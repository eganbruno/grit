# grit — notes for Claude

A Rust CLI for driving many git and dolt repositories by alias. Read
[CONTRIBUTING.md](CONTRIBUTING.md) first — it has the layout, the recipe for
adding a command, and the recipe for adding a VCS backend. This file only
records what is easy to get wrong.

## Commands

```bash
cargo test                                   # 285 tests, under a minute
cargo clippy --all-targets -- -D warnings    # CI gate
cargo fmt
GRIT_CONFIG=/tmp/scratch.toml cargo run -- status
```

Always use `GRIT_CONFIG` when running grit manually. Without it you are
mutating the user's real `~/.config/grit/config.toml`. `GRIT_CACHE` does the
same for the status cache.

## Invariants worth not breaking

- **Commands do not spawn processes or touch the filesystem.** They go through
  `Registry` and `trait Vcs`. Reaching for `std::process::Command` inside
  `commands/` means the abstraction is being bypassed.
- **`RESERVED_ALIASES` in `src/registry/mod.rs` must list every subcommand and
  alias.** `cli::tests::every_subcommand_is_reserved` enforces it. It also
  reserves two names — `completions` and `run` — that are not subcommands yet,
  so nobody's alias can shadow them later; the test only runs one way.
- **`-k` is not a `global` flag, and `--color` is.** clap offers a global flag
  under every subcommand's help, so making `-k` global had `grit show --help`
  advertising "keep going after a repo fails" on a command with no fan-out,
  and `grit show -k` exiting 0 having done nothing. Only `run::fan_out` reads
  it. `--color` is global because every command paints.
- **`grit help` and `grit version` are answered in `commands::dispatch`.**
  `disable_help_subcommand` and `--version`-as-a-flag mean both bare words
  reach the external subcommand, where they used to come back as "no repo
  registered under alias `help`". They are intercepted before the passthrough,
  and both names are in `RESERVED_ALIASES`, so no alias is shadowed by it.
- **`grit <command> --help` is where the worked examples live.** The README
  points at it rather than repeating it, so a command added without an
  `after_help` block is a documentation regression, and
  `cli::tests::every_visible_command_carries_examples` fails on one. Where the
  help quotes a constant — the block `shell enable` writes, the environment
  variables it lists — a test compares the two, because a copy is precisely the
  thing that drifts. Anything in there is shipped documentation: run the
  examples before changing them, which is how the `grit show --json` ones came
  to say `.repos[]` (that command answers with an object; `grit status --json`
  is the one that answers with an array).
- **`normalize_args` in `src/cli.rs` reads the flag list off the clap grammar**
  rather than hardcoding it, so `-r` keeps working when a global flag is added.
  Do not replace it with a hardcoded list.
- **Passthrough inherits stdio.** Capturing it would break the caller's pager
  (`delta`, `less`), colour detection and `$EDITOR`. `vcs/git.rs::exec` is
  explicit about this on purpose.
  The corollary is that **the fan-out cannot know whether a child printed
  anything**, which is why every repo in `run::fan_out` gets a receipt (`✓ ok`)
  and not just the silent ones. The divider goes out before the child and so
  promises output that `grit @tag add .` never produces; the receipt goes out
  after, from the exit code, which grit holds either way. Three tempting
  alternatives, all worse: a whitelist of "quiet commands" is wrong on the rows
  that matter (a `git add` that warns, an alias, dolt differing from git); a
  pipe buys detection at the cost of the pager, the colour and `$EDITOR`; and
  writing a placeholder then `\r`-ing so the child overwrites it strands the
  placeholder's tail on the end of any shorter first line, and garbles when
  stdout is a file.
- **`git status --porcelain=v2` is the only status format we parse.** v1 and
  the human-readable format are not stable enough to parse. Two things about
  reading the *entries* rather than counting them, which `grit detail` does:
  the invocation carries `-c core.quotePath=false`, because git's default
  C-quotes any non-ASCII path and the list would show the characters
  `"caf\303\251.txt"`; and a rename entry (`2 R.`) separates the new path from
  the old with a **tab**, not a space. `parse_status` returns counts and files
  from one walk so the header and the list cannot disagree.
- **`grit detail` takes a live reading; it never touches the cache.** The cache
  exists so a dashboard of twenty repos can appear instantly. This is one repo,
  asked for by name, and a card drawn from a stale cache would say "clean"
  about a repo you had just edited.
- **An error's first line has to carry the cause.** `grit status` renders only
  that line, so anything a backend failure needs to say belongs there — put a
  newline or a pasted SQL query in front of it and the row says nothing.
  `Error::CommandFailed` takes a *summary* of the invocation, and stderr goes
  through `vcs::one_line`, which flattens and caps it.
- **Dolt is not git with another name, and `vcs/dolt.rs` is not `vcs/git.rs`
  with the binary swapped.** Dolt has no `rev-parse` and no porcelain status, so
  every reading comes from the `dolt_*` system tables via `dolt sql -r json`.
  Three things there are easy to break:
  - ages are measured against `utc_timestamp()`, because `dolt_log.date` is UTC
    while `now()` is local — `now()` skews every age by the machine's offset
  - detection requires `.dolt/repo_state.json` or `.dolt/noms`, not `.dolt`
    alone: `~/.dolt` is dolt's *global config*, and a bare check registers a
    home directory as a repo
  - branch names reach SQL through `sql_literal`, because dolt allows a `'` in
    one
  - **`dolt_log.message` is the whole commit message, not its subject.** git's
    `%s` is the subject alone, so `subject_line` trims dolt's to match. Skip it
    and a commit body goes into the dashboard: its lines push the next repo into
    the wrong columns, and the preview leaves rows stranded above the prompt
  - **every number may arrive as a string.** With a `dolt sql-server` running
    against the database, `dolt sql` becomes a MySQL client and the wire
    protocol stringifies everything: `"0"`, not `0` or `false`. Both shapes are
    ordinary dolt, which is what `Wire`/`wire_u32` in `vcs/dolt.rs` exist for.
    Plain `u32` fields there compile and then fail on any developer who happens
    to be running a server

## The preview at the prompt

`grit shell init zsh` emits a ZLE integration that draws the dashboard under the
line you are typing. Four things hold it up, and each is easy to undo:

- **The preview must not run git.** It renders `src/cache.rs`, which every
  unfiltered `grit status` refills on its way out. A preview that took a live
  reading would block the line editor for as long as the slowest repo.
- **A cache that is subtly wrong is worse than no cache.** `StatusCache::matches`
  compares the cached rows against the live registry and rejects the whole file
  on any difference. Loosening that to "close enough" puts a row for a repo you
  removed on screen. `--max-age` is the same rule in the time dimension: a
  reading old enough to mislead is withheld and a refresh is started instead.
- **The preview carries no ANSI.** zsh is holding the table in `POSTDISPLAY` and
  renders it literally, so an escape shows up as the characters `ESC [ 3 6 m`.
  Colour goes across as `region_highlight` ranges instead — *character* offsets,
  not bytes and not display columns — which is what `Table::render_highlighted`
  and `theme::zsh_style` exist for. `tests/cli_shell.rs` slices the text with
  the offsets it was given, because an off-by-one is invisible in the output.
- **The timer is a descriptor watched with `zle -F`** — not `TMOUT`, and no
  longer `zsh/sched`. `src/shell/grit.zsh` says why at length; the short version
  is that `sched` cannot express a delay under a second (`sched +0.5` is a parse
  error) and the default is 0.5. Three things about the replacement bite:
  - **a watch is removed with `zle -F <fd>`, handler omitted.** `zle -F -<fd>`
    is a parse error, not a negation, and `2>/dev/null` makes it look like
    success. The descriptor then gets closed under a watch that is still
    installed, and one dead descriptor in ZLE's select set starves *every* watch
    in it — which is how this shipped a bug that left powerlevel10k's git
    segment stuck on "loading" for the life of the shell. `tests/shell/
    preview.py` asserts both that no grit watch survives a teardown and that a
    neighbour's watch still fires afterwards.
  - **the sleeper is one ticking subshell, but only by choice.** A handler
    re-arming itself does work; one fork per line of typing simply beats one per
    half second. Stopping is therefore grit's job, not the sleeper's.
  - **the handler must read its tick.** An unread descriptor stays readable and
    ZLE calls straight back, with no pause at all.
  - **the two `zle -N`s on the hook widgets are not redundant.**
    `_grit_preview_on_redraw` and `_grit_preview_on_finish` are reached only
    through `add-zle-hook-widget`, which makes registering them look like
    ceremony. It is not: `add-zle-hook-widget` takes a widget, so a name that
    is only a function is rejected and left out of the hook's list without a
    word. That costs the whole idle preview, and `add-zle-hook-widget -L
    line-pre-redraw` is the only thing that shows it.
  - **a redirection on a bare `exec` belongs to the shell.** `exec {fd}<&-
    2>/dev/null` closes the descriptor and points the *interactive shell's*
    stderr at /dev/null for the life of the shell, so every error message
    anything prints afterwards is discarded — `grit @nosuchtag branch` reads as
    doing nothing at all. Wrap it: `{ exec {fd}<&- } 2>/dev/null`.
    `tests/shell/preview.py` prints to stderr after an arm and a disarm.
  - **`_grit_preview_release` owns both the descriptor and the armed flag.** The
    two can disagree, TRAPINT can land in the window where they do, and an arm
    that overwrites a live descriptor number orphans its ticker for the life of
    the shell. Anything that closes the descriptor goes through that one
    function.

  grit still never touches TMOUT or TRAPALRM, so it cannot disable anyone's
  auto-logout, and nothing is watched at all except in the half second after the
  trigger is typed. `tests/shell/preview.py` asserts that with `zle -FL`; an
  empty `$(sched)` proves nothing now and `jobs` never lists a process
  substitution.

`grit shell enable` writes the loading line into a startup file and
`disable` takes back exactly the block it wrote, matched by markers. Two rules
there: grit edits a startup file **only** when asked — being run is not asking,
and installing grit changes nothing — and a line grit did not write is reported
rather than edited, because guessing where someone's own line ends is how a tool
eats a config. `src/shell/mod.rs` keeps the file handling, so `commands/` stays
out of the filesystem; the add/remove halves are pure string functions with the
tests to match.

Only `grit status` with no alias and no `--tag` writes the cache. A filtered run
cached as if it were everything leaves repos silently absent from the preview,
which reads as "clean" rather than as "not looked at".

The zsh script's comments are load-bearing. Nearly every line with one attached
is there because the obvious version was measured and found to be wrong.

## Rendering

The intended aesthetic is fzf-like: aligned columns, colour for meaning, no
box-drawing. Before changing the table:

- widths are measured with `unicode-width`, not `str::len`
- cells hold styled *spans*, and styling is applied at render time — that is
  what makes truncation safe, and it is also the only reason the shell preview
  can exist. Never bake ANSI into a cell's text.
- `Table::lines` is the single layout path; `render` and `render_highlighted`
  are two consumers of it. Adding a third renderer means another consumer, not
  another copy of the alignment code.
- colour must be off when stdout is not a terminal or `NO_COLOR` is set; the
  integration tests assert this.
- **the README's dashboard is generated, not drawn.** GitHub cannot colour a
  fenced block, so the headline example is an SVG — and an SVG is a second copy
  of the palette. `tests/readme_svg.rs` renders it through the real
  `build_table` and the real `Theme`, and asserts the committed
  `assets/status-*.svg` match a fresh render, so repainting `Theme` fails
  `cargo test` rather than quietly making the README lie. The fix it asks for:

  ```bash
  cargo test --test readme_svg -- --ignored
  ```

  Two things there are easy to get wrong. `render::svg` is a third *renderer*
  but not a third *layout* — it consumes `render_highlighted`'s text-plus-ranges
  like the shell preview does, and reaching for `Table::lines` again would be
  the copy the table module warns about. And the palette is not a transcription
  of xterm: the numbered greys (`8`, `238`) are resolved per medium, because
  the terminal's 238 against a web page is a rule nobody can see. Named hues
  come from `zsh_style`'s table via `theme::ink`, so there is still exactly one
  list of what grit's colours mean.

- **no cell may hold a control character.** `Cell` flattens newlines and tabs to
  spaces on the way in, because cells are filled from strings read out of
  repositories and a renderer that comes apart on its input is the wrong place
  to be trusting. The backend that produced the newline should still fix it at
  source — this is the net, not the answer.

## Testing

Parsers in `vcs/git.rs` and `vcs/dolt.rs` are pure `&str -> value` functions
with a fixture table — add a case there before reaching for a real repository.
The dolt fixtures are captured verbatim from `dolt sql -r json`, including its
habit of omitting null columns from the output altogether; keep them that way. `TestEnv` in
`tests/common/mod.rs` is for behaviour that genuinely needs git: it neutralises
`GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` so the developer's own gitconfig cannot
change what the tests see.

## The repo picker

`^G^G` opens fzf over `grit shell rows`, with `grit detail {1}` as the preview
command. grit spawns nothing: the fzf invocation lives in the shell scripts, so
`commands/` stays out of the process business, and the widget runs in the
user's own shell — which is the only reason its `ctrl-d` binding can change
that shell's directory.

- **`^G` is a bare prefix, bound to nothing.** The picker is `^G^G`, the
  inline preview `^G^P`. Every line editor resolves an ambiguous prefix by
  waiting for the next key, and charges that wait to the *shorter* binding:
  measured at 404ms (zsh 5.9, `KEYTIMEOUT`) and 504ms (readline,
  `keyseq-timeout`) on every press of it. So nothing sits on `^G` alone.
- **Not every key can be the tail of a chord.** `^G^D` is the obvious mnemonic
  for a dashboard and is unusable: on an empty command line the `^D` is read
  as end-of-input, so it never fires the widget and closes the shell instead.
  Measured in zsh and bash both. `^P ^A ^V ^K ^N ^Y ^G` are all fine, and
  `^W ^U ^V ^O ^S ^Q ^Z` never reach a line editor at all.
  `tests/shell/preview.py` presses each chord on an empty line for this.
- **The letters avoid fzf-git.sh's `^G^{f,b,t,r,h,s,l,e,w}`.** It is widely
  installed, it claims `^G` as a prefix too, and it binds both `^Gx` and
  `^G^x` for each. An earlier default of `^G^R` went straight into its
  `Remotes` picker on the first machine it met. `tests/shell/preview.py`
  asserts the whole family stays clear.
- **grit binds what it is asked for and does not rearrange it.** An earlier
  version moved a binding out of the way when it was a prefix of the other.
  That was one more rule to explain, and it became wrong as soon as there were
  three chords: doubling `^G` lands on `^G^G`, which is the picker. Setting
  `GRIT_PREVIEW_KEY='^G'` now does exactly that, pause and all.
- **Nothing in the scripts may be `typeset -r`.** A read-only constant makes
  the *second* sourcing of the script a fatal error at that line, so the eval
  aborts and every widget, hook and binding below it is silently never
  installed — the shell looks like it loaded and does nothing. Re-sourcing is
  what anyone does to pick up a new grit without a new terminal.
  `tests/shell/preview.py` sources twice and then checks a binding from the
  bottom of the file.
- **`grit shell rows` prints flush left.** fzf splits a row into fields the way
  awk does but counts a leading run of spaces as field one, so with the table's
  usual two-space margin `{1}` comes back empty and every preview is of
  nothing. `Table::no_indent` exists for that, and `no_header` for the same
  family of reason — a header row handed to a picker is a row you can select.
- **A hidden header reserves no width.** `no_header` had to stop the header
  text feeding `natural_widths`, or `CODE` above a two-character code holds the
  column four wide and pushes the path column out.
- **A distance nobody measured is `None`, never zero.** `Branch::distance` is
  an `Option` because `(0, 0)` renders as `✓ in sync`, and there are two ways
  to have no number that are not that: git still names the upstream of a
  branch whose upstream ref has been *deleted* (`%(upstream:track)` is
  `[gone]`), and dolt's per-branch counts come from a second invocation that
  can fail outright. Both used to paint a tick. `dolt sql` also stops at the
  first statement that errors and exits non-zero, so one never-fetched
  upstream took every other branch's count down with it — hence the
  batch-then-one-at-a-time fallback in `fill_distances`.
- **`grit shell path` exists so the shell does not parse JSON.** `show --json`
  carries the same fact, but reading it from a key binding meant either a jq
  dependency or a grep-and-sed that is wrong on a path with a quote in it.

## Not yet built

Interactive TUI (the picker is fzf, not a TUI grit draws), shell completions,
`grit clone`. For dolt, per-branch ahead/behind costs one query per branch —
batched into a single invocation, but still a second round trip — and
`dolt_stashes` records no timestamp at all, so a stash there has no age.

**`PROMPT_COMMAND` is not grit's variable.** `src/shell/grit.bash` hooks it so
a table that has scrolled into real output stops being ours to erase. Since
bash 5.1 it may be an *array*, which prompt frameworks use. Assigning a string
to one does not lose the other elements — bash writes element 0 — so the damage
is quieter than it looks: it rewrites a line its owner put there, and the
string check then only ever reads element 0, so a marker in element 1 reads as
absent and gets added again. grit prepends its own element instead. `${VAR@a}`
is how you tell, it needs bash 4.4, and it sits behind the version test rather
than beside it because on bash 3.2 it is a runtime "bad substitution" that lazy
`&&` never reaches. `tests/shell/preview.py` covers both shapes and skips the
array half below 5.1.

bash and fish get `^G` rather than the idle preview: neither fires a hook while
you sit at the prompt. Not quite a dead end for fish — a self-armed background
timer sending `SIGUSR1` to an `--on-signal` handler does run while its reader is
blocked — but that is a spawned sleeper process per prompt to reach what zsh
does with a builtin, and it is not built.

`tests/shell/preview.py` drives all three in a pty and is not part of `cargo
test`; it is timing-dependent. Run it after touching `src/shell/`.

`tests/cli_dolt.rs` needs the real `dolt` binary and skips itself when it is
absent, so a green local run does not necessarily mean those ran — CI installs
dolt. Check the skip notices if a dolt change looks suspiciously fine.
