# grit — notes for Claude

A Rust CLI for driving many git and dolt repositories by alias. Read
[CONTRIBUTING.md](CONTRIBUTING.md) first — it has the layout, the recipe for
adding a command, and the recipe for adding a VCS backend. This file only
records what is easy to get wrong.

## Commands

```bash
cargo test                                   # 258 tests, ~17s
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
  alias.** `cli::tests::every_subcommand_is_reserved` enforces it.
- **`normalize_args` in `src/cli.rs` reads the flag list off the clap grammar**
  rather than hardcoding it, so `-r` keeps working when a global flag is added.
  Do not replace it with a hardcoded list.
- **Passthrough inherits stdio.** Capturing it would break the caller's pager
  (`delta`, `less`), colour detection and `$EDITOR`. `vcs/git.rs::exec` is
  explicit about this on purpose.
- **`git status --porcelain=v2` is the only status format we parse.** v1 and
  the human-readable format are not stable enough to parse.
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

## Not yet built

Interactive TUI, shell completions, `grit clone`.

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
