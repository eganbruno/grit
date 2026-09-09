#!/usr/bin/env python3
"""Drive the emitted shell integrations in a real terminal.

    cargo build && python3 tests/shell/preview.py

`cargo test` covers the protocol `grit shell preview` speaks. It cannot cover
the half that matters most — whether a line editor, handed that protocol, puts
the right characters on a screen. The failure mode there is a script that reads
correctly and behaves wrongly, and the only way to see it is to run a shell on a
pseudo-terminal and look at the bytes it writes.

So this types into zsh, bash and fish for real and asserts against their raw
output: that the table appears, that the colours landed on the cells they were
meant for, that a keystroke erases it, that Ctrl-C leaves nothing stranded, and
that someone who already uses `TMOUT` for an auto-logout keeps it.

Deliberately not part of `cargo test`. It waits on a one-second timer several
times over, which is fine on a laptop and a coin flip on a loaded CI runner; a
flaky gate would teach people to ignore it. Run it by hand when you change
anything under `src/shell/`. A shell that is not installed is skipped, and so is
the zsh-autosuggestions check unless `$ZSH_AUTOSUGGESTIONS` points at a copy of
`zsh-autosuggestions.zsh`.
"""

import fcntl
import json
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time

COLS, ROWS = 100, 30

# fish 4 asks the terminal what it can do and blocks until answered. A pty is
# not a terminal, so the harness has to answer for one.
CAPABILITY_REPLIES = [
    (re.compile(rb"\x1bP\+q[0-9A-Fa-f;]*(?:\x1b\\|\x07)"), b"\x1bP0+r\x1b\\"),
    (re.compile(rb"\x1b\[>0q"), b"\x1bP>|grit-tests\x1b\\"),
    (re.compile(rb"\x1b\[>?[0-9;]*c"), b"\x1b[?1;2c"),
    (re.compile(rb"\x1b\[6n"), b"\x1b[1;1R"),
    (re.compile(rb"\x1b\]1[01];\?"), b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
]

# Escape sequences to drop when asking "is this string on the screen".
NOISE = [
    re.compile(r"\x1b\][^\x07\x1b]*(\x07|\x1b\\)"),  # OSC
    re.compile(r"\x1bP[^\x1b]*\x1b\\"),  # DCS
    re.compile(r"\x1b\[[0-9;?>]*[a-zA-Z]"),  # CSI
    re.compile(r"\x1b[()][AB0]"),  # charset selection
]


def flatten(raw: bytes) -> str:
    text = raw.decode("utf-8", "replace")
    for pattern in NOISE:
        text = pattern.sub("", text)
    return text.replace("\r", "")


def sgr_runs(raw: bytes, needle: str) -> list:
    """The SGR parameters in force wherever `needle` appears.

    Proving a colour *arrived* means finding the escape that set it in front of
    the text it was meant for. Anything less passes when the range is off.
    """
    text = raw.decode("utf-8", "replace")
    runs, active, i = [], "", 0
    while i < len(text):
        m = re.match(r"\x1b\[([0-9;]*)m", text[i:])
        if m:
            active = m.group(1)
            i += m.end()
        elif text.startswith(needle, i):
            runs.append(active)
            i += len(needle)
        else:
            i += 1
    return runs


class Shell:
    """An interactive shell on a pseudo-terminal."""

    def __init__(self, argv, env, rows=ROWS, cols=COLS):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ.clear()
            os.environ.update(env)
            os.execvp(argv[0], argv)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.read(1.0)

    def read(self, seconds=1.0) -> bytes:
        end, seen = time.time() + seconds, b""
        while time.time() < end:
            ready, _, _ = select.select([self.fd], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            seen += chunk
            for pattern, reply in CAPABILITY_REPLIES:
                if pattern.search(chunk):
                    os.write(self.fd, reply)
        return seen

    def send(self, text: str):
        os.write(self.fd, text.encode())

    def run(self, line: str, wait=1.0) -> str:
        self.send(line + "\r")
        return flatten(self.read(wait))

    def close(self):
        try:
            self.send("\x15exit\r")
            self.read(0.3)
            os.close(self.fd)
        except OSError:
            pass


class Report:
    def __init__(self):
        self.passed, self.failed, self.skipped = 0, [], []

    def check(self, name, ok, detail=""):
        if ok:
            self.passed += 1
            print(f"  \033[32mpass\033[0m  {name}")
        else:
            self.failed.append(name)
            print(f"  \033[31mFAIL\033[0m  {name}")
            if detail:
                print(f"        {detail[:600]}")

    def skip(self, name, why):
        self.skipped.append(name)
        print(f"  \033[2mskip\033[0m  {name} ({why})")


def git(args, cwd):
    subprocess.run(
        ["git"] + args,
        cwd=cwd,
        check=True,
        capture_output=True,
        env=dict(
            os.environ,
            GIT_CONFIG_GLOBAL="/dev/null",
            GIT_CONFIG_SYSTEM="/dev/null",
            GIT_AUTHOR_NAME="grit tests",
            GIT_AUTHOR_EMAIL="tests@grit.invalid",
            GIT_COMMITTER_NAME="grit tests",
            GIT_COMMITTER_EMAIL="tests@grit.invalid",
        ),
    )


class Fixture:
    """A registry of throwaway repos, with grit's cache already warm."""

    def __init__(self, grit, aliases=("api", "dashboard")):
        self.grit = os.path.abspath(grit)
        self.root = tempfile.mkdtemp(prefix="grit-preview-")
        self.config = os.path.join(self.root, "config.toml")
        self.cache = os.path.join(self.root, "status-cache.json")

        for alias in aliases:
            repo = os.path.join(self.root, alias)
            os.makedirs(repo)
            git(["init", "-q", "-b", "main"], repo)
            open(os.path.join(repo, "README.md"), "w").write(f"# {alias}\n")
            git(["add", "."], repo)
            git(["commit", "-qm", f"initial commit for {alias}"], repo)
            self.grit_run(["-r", alias, repo])
        self.grit_run(["shell", "refresh"])

    def grit_run(self, args):
        subprocess.run([self.grit] + args, check=True, capture_output=True, env=self.env())

    def env(self):
        return dict(
            os.environ,
            GRIT_CONFIG=self.config,
            GRIT_CACHE=self.cache,
            HOME=self.root,
            TERM="xterm-256color",
            LANG="en_US.UTF-8",
            LC_ALL="en_US.UTF-8",
            # The scripts call a bare `grit`, the way a real install would.
            PATH=os.path.dirname(self.grit) + ":" + os.environ["PATH"],
        )

    def rc(self, name, body):
        path = os.path.join(self.root, name)
        open(path, "w").write(body)
        return path

    def clean(self):
        shutil.rmtree(self.root, ignore_errors=True)


def have(program):
    return shutil.which(program) is not None


def zsh_suite(grit, report):
    print("\nzsh — the idle preview")
    if not have("zsh"):
        return report.skip("zsh", "not installed")

    fx = Fixture(grit)
    rc = fx.rc("rc.zsh", f"PS1='%% '\neval \"$({fx.grit} shell init zsh)\"\n")
    sh = Shell(["zsh", "-f", "-i"], fx.env())
    sh.run(f"source {rc}", 1.2)

    sh.send("grit")
    report.check("nothing appears while you are still typing",
                 "ALIAS" not in flatten(sh.read(0.35)))

    drawn = sh.read(3.0)
    text = flatten(drawn)
    report.check("the table appears after the pause", "ALIAS" in text and "api" in text,
                 repr(text))
    report.check("every repo is listed", "api" in text and "dashboard" in text)
    report.check("the footer says how old the reading is", " ago" in text, repr(text))
    report.check("no escape sequence leaked through as text",
                 "\\x1b" not in text and "[36m" not in text)

    # POSTDISPLAY renders literally, so these colours can only have come from
    # region_highlight ranges landing on the right characters.
    report.check("the alias is bold",
                 any("1" in run.split(";") for run in sgr_runs(drawn, "api")),
                 f"runs around 'api': {sgr_runs(drawn, 'api')}")
    report.check("the branch is cyan",
                 any("36" in run.split(";") for run in sgr_runs(drawn, "main")),
                 f"runs around 'main': {sgr_runs(drawn, 'main')}")

    sh.send("x")
    erased = sh.read(1.0)
    report.check("a keystroke erases it", b"\x1b[" in erased and b"K" in erased)
    report.check("and it stays erased", "ALIAS" not in flatten(sh.read(2.0)))

    sh.send("\x7f")
    report.check("deleting back to the trigger brings it back",
                 "ALIAS" in flatten(sh.read(2.5)))

    # Ctrl-C reaches the shell as a signal; ZLE runs no hook for it at all.
    sh.send("\x03")
    sh.read(1.0)
    report.check("ctrl-c leaves nothing stranded",
                 "ALIAS" not in sh.run("echo done", 1.5))

    sh.send("grit")
    sh.read(2.5)
    ran = sh.run("", 3.0)
    report.check("enter still runs the command", "Work across many" in ran or "Usage:" in ran,
                 repr(ran))
    report.check("and leaves no second copy of the table", ran.count("ALIAS") <= 1,
                 f"ALIAS appears {ran.count('ALIAS')} times")

    sh.close()
    fx.clean()


def zsh_neighbours_suite(grit, report, autosuggestions=None):
    """The preview shares a shell with other people's code.

    It watches a descriptor with `zle -F` rather than setting TMOUT, so it
    should leave a user's auto-logout and their own TRAPALRM completely alone,
    and watch nothing at all on a line that is not the trigger. ZLE's select
    set is shared as well: one watch grit forgets to remove starves every other
    watch in it. POSTDISPLAY, meanwhile, is a single slot that
    zsh-autosuggestions also wants.
    """
    print("\nzsh — sharing the shell")
    if not have("zsh"):
        return report.skip("zsh neighbours", "not installed")

    fx = Fixture(grit, aliases=("api",))

    rc = fx.rc("tmout.zsh", f"PS1='%% '\nTMOUT=600\neval \"$({fx.grit} shell init zsh)\"\n")
    sh = Shell(["zsh", "-f", "-i"], fx.env())
    sh.run(f"source {rc}", 1.2)
    state = sh.run("print -r -- TMOUT=$TMOUT ALRM=${+functions[TRAPALRM]}")
    report.check("an auto-logout TMOUT is left as it was", "TMOUT=600" in state, state.strip())
    report.check("and no TRAPALRM is defined over it", "ALRM=0" in state, state.strip())

    sh.send("grit")
    report.check("the preview still works alongside it", "ALIAS" in flatten(sh.read(3.0)))
    sh.close()


    rc = fx.rc("idle.zsh", f"PS1='%% '\neval \"$({fx.grit} shell init zsh)\"\n")

    sh = Shell(["zsh", "-f", "-i"], fx.env())
    sh.run(f"source {rc}", 1.2)
    # Quoted: `[unset]` is a glob character class to an unquoted zsh.
    #
    # `zle -FL` is the load-bearing one. The timer is a watched descriptor now,
    # so an empty `$(sched)` says nothing at all — it would pass just as well if
    # the preview had left a ticker running on every prompt. A process
    # substitution never appears in `jobs` either, which is why neither of the
    # other two can be trusted to notice.
    state = sh.run('print -r -- "TMOUT=<${TMOUT:-unset}> WATCH=<$(zle -FL 2>/dev/null)>'
                   ' SCHED=<$(sched)> JOBS=<$(jobs)>"')
    report.check("nothing is watched or scheduled at rest",
                 "TMOUT=<unset>" in state and "WATCH=<>" in state
                 and "SCHED=<>" in state and "JOBS=<>" in state,
                 state.strip())

    # An ordinary command that is not the trigger must cost nothing at all.
    sh.send("git commit -m wip")
    sh.read(0.5)          # the shell's own echo of what was typed
    quiet = sh.read(2.5)
    report.check("an unrelated line wakes the shell not at all", quiet == b"",
                 f"{len(quiet)} bytes: {quiet[:200]!r}")
    sh.send("\x15")

    # The buffer passes through `grit` on the way to `grit status`.
    for ch in "grit status":
        sh.send(ch)
        sh.read(0.12)
    report.check("typing `grit status` straight through does not flash the table",
                 "ALIAS" not in flatten(sh.read(0.4)))
    sh.close()


    # The preview is not the only thing in the shell watching a descriptor:
    # powerlevel10k's gitstatus waits on its daemon exactly this way. A watch
    # grit forgets to remove is left pointing at a descriptor it has closed, and
    # one dead descriptor in ZLE's select set stops *every* watch in it being
    # serviced — which reads, to whoever is running the prompt, as a git segment
    # stuck on "loading" that never updates again.
    rc = fx.rc("neighbour.zsh",
               f"PS1='%% '\n"
               "zmodload -F zsh/zselect b:zselect\n"
               # a stand-in for gitstatus: its own watch, counting what it gets
               "typeset -gi _nb_ticks=0 _nb_fd=0\n"
               "_nb_handler() { read -r -k1 -u $1 _j 2>/dev/null; (( _nb_ticks++ )); }\n"
               "exec {_nb_fd}< <(while :; do zselect -t 30 2>/dev/null; print -n x || break; done)\n"
               "zle -F $_nb_fd _nb_handler\n"
               f"eval \"$({fx.grit} shell init zsh)\"\n")
    sh = Shell(["zsh", "-f", "-i"], fx.env())
    sh.run(f"source {rc}", 1.2)

    # Draw and tear down through each of the three exits.
    for exit_key in ("\x7f\x7f\x7f\x7f", "\r", "\x03"):
        sh.send("grit")
        sh.read(1.4)
        sh.send(exit_key)
        sh.read(1.0)

    left = sh.run("print -r -- \"OWN=<$(zle -FL | grep _grit_preview)>\"", 1.5)
    report.check("no descriptor watch survives a teardown",
                 "OWN=<>" in left, left.strip())

    before = sh.run('print -r -- "N=$_nb_ticks"', 1.0)
    sh.read(1.5)
    after = sh.run('print -r -- "N=$_nb_ticks"', 1.0)
    def ticks(text):
        m = re.search(r"N=(\d+)", text)
        return int(m.group(1)) if m else -1
    report.check("a neighbour's watch is still being serviced afterwards",
                 ticks(after) > ticks(before) >= 0,
                 f"{ticks(before)} -> {ticks(after)}")
    sh.close()


    # A redirection on a bare `exec` belongs to the shell, not to the exec: one
    # `exec {fd}<&- 2>/dev/null` closing the ticker took the interactive shell's
    # stderr with it, permanently, and every error message anything printed
    # afterwards went to /dev/null. It reads as commands doing nothing at all.
    rc = fx.rc("stderr.zsh", f"PS1='%% '\neval \"$({fx.grit} shell init zsh)\"\n")
    sh = Shell(["zsh", "-f", "-i"], fx.env())
    sh.run(f"source {rc}", 1.2)
    sh.send("grit")
    sh.read(1.4)                      # arm the timer and let it fire
    sh.send("\x7f" * 4)
    sh.read(1.0)                      # and tear it back down
    # Split so the terminal's echo of what was typed does not itself contain
    # the marker: the only way `STDERR-LIVES` reaches the screen whole is by
    # being printed.
    seen = sh.run('print -r -- "STDERR-""LIVES" >&2', 1.5)
    report.check("the shell's own stderr survives an arm and a disarm",
                 "STDERR-LIVES" in seen,
                 seen.strip())
    sh.close()

    if autosuggestions and os.path.exists(autosuggestions):
        rc = fx.rc("suggest.zsh",
                   f"PS1='%% '\nsource {autosuggestions}\n"
                   f"eval \"$({fx.grit} shell init zsh)\"\n")
        sh = Shell(["zsh", "-f", "-i"], fx.env())
        sh.run(f"source {rc}", 1.5)
        sh.run("grit status --cached", 2.0)   # give the history a `grit ...` entry
        sh.send("grit")
        both = flatten(sh.read(3.0))
        report.check("the table appears even when a suggestion owns POSTDISPLAY",
                     "ALIAS" in both, repr(both))
        sh.send("x")
        sh.read(1.0)
        left = sh.run("print -r -- PD=[$POSTDISPLAY]", 1.0)
        report.check("and erasing it does not eat the suggestion",
                     "ALIAS" not in left, repr(left))
        sh.close()
    else:
        report.skip("zsh-autosuggestions coexistence", "plugin not available")

    fx.clean()


def zsh_staleness_suite(grit, report):
    """A reading old enough to mislead is withheld, not drawn.

    The table is read at a glance and acted on; one from this morning shown the
    same way as a live one is the failure this whole feature has to avoid. So
    the preview declines, the tick behind it starts a refresh, and it comes back
    a second later with the truth.
    """
    print("\nzsh — a reading too old to trust")
    if not have("zsh"):
        return report.skip("zsh staleness", "not installed")

    fx = Fixture(grit, aliases=("api",))
    cache = json.load(open(fx.cache))
    cache["captured_at"] = "2020-01-01T00:00:00Z"
    json.dump(cache, open(fx.cache, "w"))
    backdated = open(fx.cache).read()

    # The read windows below encode "before the second tick" and "after it", so
    # the delay is pinned rather than inherited: this suite is about staleness,
    # and halving the shipped default should not silently turn it into a test of
    # something else.
    rc = fx.rc("stale.zsh",
               f"PS1='%% '\nGRIT_PREVIEW_DELAY=1\neval \"$({fx.grit} shell init zsh)\"\n")
    sh = Shell(["zsh", "-f", "-i"], fx.env())
    sh.run(f"source {rc}", 1.2)

    sh.send("grit")
    report.check("a stale reading is withheld on the first tick",
                 "ALIAS" not in flatten(sh.read(1.6)))
    later = flatten(sh.read(4.0))
    report.check("the refreshed one arrives a tick later", "ALIAS" in later, repr(later))
    report.check("and its footer says it is current",
                 any(f"{n}s ago" in later for n in range(4)), repr(later))
    report.check("the cache on disk was rewritten", open(fx.cache).read() != backdated)

    sh.close()
    fx.clean()


def key_suite(grit, report, name, argv, rc_name, rc_body, prompt_wait=1.5):
    """bash and fish both get Ctrl-G rather than the idle trigger, and both
    print above the prompt rather than into the line, so one suite covers them."""
    print(f"\n{name} — the key binding")
    if not have(argv[0]):
        return report.skip(name, "not installed")

    fx = Fixture(grit)
    rc = fx.rc(rc_name, rc_body(fx))
    sh = Shell(argv, fx.env())
    sh.run(f"source {rc}", prompt_wait)

    sh.send("git ")
    sh.read(0.4)
    sh.send("\x07")
    drawn = flatten(sh.read(2.5))
    report.check("ctrl-g draws the table", "ALIAS" in drawn and "api" in drawn, repr(drawn))
    report.check("what was typed survives", "git" in drawn.splitlines()[-1], repr(drawn))

    sh.send("\x07")
    report.check("ctrl-g again takes it down", "ALIAS" not in flatten(sh.read(2.0)))

    sh.send("\x07")
    sh.read(2.0)
    sh.send("\x07")
    report.check("a second cycle leaves no leftover rows",
                 flatten(sh.read(2.0)).count("ALIAS") == 0)

    report.check("the shell is still usable", "still-here" in sh.run("\x15echo still-here", 1.5))
    sh.close()
    fx.clean()


def bash_prompt_command_suite(grit, report):
    """grit hooks `PROMPT_COMMAND` so a table that scrolled into real output
    stops being ours to erase. It is somebody else's variable, and bash 5.1
    let it be an array — which prompt frameworks use. Writing a string to an
    array does not drop the other elements, bash puts it in element 0, but it
    does rewrite a line its owner put there, and a marker landing anywhere but
    element 0 then reads as absent and gets added a second time."""
    print("\nbash — sharing PROMPT_COMMAND")
    if not have("bash"):
        return report.skip("bash PROMPT_COMMAND", "not installed")

    fx = Fixture(grit)
    # Sourced twice throughout: hooking once is the property under test.
    twice = f'eval "$({fx.grit} shell init bash)"\n' * 2

    rc = fx.rc("pc-string.bash",
               f'PS1="% "\nPROMPT_COMMAND="echo THEIRS"\n{twice}')
    sh = Shell(["bash", "--norc", "-i"], fx.env())
    sh.run(f"source {rc}", 1.5)
    # Never name the function in the command, or the pty echo counts as a hit.
    state = sh.run('printf "PC=<%s>\\n" "$PROMPT_COMMAND"', 1.2)
    report.check("a string PROMPT_COMMAND keeps what was already in it",
                 "echo THEIRS" in state, state.strip())
    report.check("and grit hooks it once, not once per sourcing",
                 state.count("__grit_preview_forget") == 1, state.strip())
    sh.close()

    out = subprocess.run(["bash", "-c", "echo ${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"],
                         capture_output=True, text=True).stdout.split()
    version = tuple(int(n) for n in out) if len(out) == 2 else (0, 0)
    if version < (5, 1):
        report.skip("bash array PROMPT_COMMAND",
                    f"bash {version[0]}.{version[1]} has no array form")
        fx.clean()
        return

    rc = fx.rc("pc-array.bash",
               f'PS1="% "\nPROMPT_COMMAND=( "echo ALPHA" "echo BETA" )\n{twice}')
    sh = Shell(["bash", "--norc", "-i"], fx.env())
    sh.run(f"source {rc}", 1.5)
    dump = sh.run('printf "E:%s\\n" "${PROMPT_COMMAND[@]}"', 1.2)
    elements = [line.split("E:", 1)[1] for line in dump.splitlines() if "E:" in line]

    report.check("an array PROMPT_COMMAND keeps every element it had",
                 "echo ALPHA" in dump and "echo BETA" in dump, repr(elements))
    report.check("grit arrives as its own element rather than editing theirs",
                 not any("ALPHA" in e and "__grit_preview_forget" in e for e in elements),
                 repr(elements))
    report.check("and is added once, not once per sourcing",
                 dump.count("__grit_preview_forget") == 1, repr(elements))
    # The elements run at every prompt, so their output is proof they survived.
    ran = sh.run("true", 1.2)
    report.check("their commands still run at the prompt",
                 "ALPHA" in ran and "BETA" in ran, repr(ran))
    sh.close()
    fx.clean()


def main():
    grit = sys.argv[1] if len(sys.argv) > 1 else "target/debug/grit"
    if not os.path.exists(grit):
        raise SystemExit(f"no grit binary at {grit} — run `cargo build` first")

    report = Report()
    autosuggestions = os.environ.get("ZSH_AUTOSUGGESTIONS")
    zsh_suite(grit, report)
    zsh_neighbours_suite(grit, report, autosuggestions)
    zsh_staleness_suite(grit, report)
    key_suite(grit, report, "bash", ["bash", "--norc", "-i"], "rc.bash",
              lambda fx: f'PS1="% "\nCOLUMNS={COLS}\neval "$({fx.grit} shell init bash)"\n')
    bash_prompt_command_suite(grit, report)
    key_suite(grit, report, "fish", ["fish", "-i"], "rc.fish",
              lambda fx: f'{fx.grit} shell init fish | source\n', prompt_wait=3.0)

    print(f"\n{report.passed} passed, {len(report.failed)} failed, "
          f"{len(report.skipped)} skipped")
    if report.failed:
        print("failed: " + ", ".join(report.failed))
    sys.exit(1 if report.failed else 0)


if __name__ == "__main__":
    main()
