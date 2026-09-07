# grit — notes for Claude

A Rust CLI for driving many git and dolt repositories by alias. Read
[CONTRIBUTING.md](CONTRIBUTING.md) first — it has the layout, the recipe for
adding a command, and the recipe for adding a VCS backend. This file only
records what is easy to get wrong.

## Commands

```bash
cargo test                                   # 186 tests, ~15s
cargo clippy --all-targets -- -D warnings    # CI gate
cargo fmt
GRIT_CONFIG=/tmp/scratch.toml cargo run -- status
```

Always use `GRIT_CONFIG` when running grit manually. Without it you are
mutating the user's real `~/.config/grit/config.toml`.

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
  - **every number may arrive as a string.** With a `dolt sql-server` running
    against the database, `dolt sql` becomes a MySQL client and the wire
    protocol stringifies everything: `"0"`, not `0` or `false`. Both shapes are
    ordinary dolt, which is what `Wire`/`wire_u32` in `vcs/dolt.rs` exist for.
    Plain `u32` fields there compile and then fail on any developer who happens
    to be running a server

## Rendering

The intended aesthetic is fzf-like: aligned columns, colour for meaning, no
box-drawing. Before changing the table:

- widths are measured with `unicode-width`, not `str::len`
- cells hold styled *spans*, and styling is applied at render time — that is
  what makes truncation safe. Never bake ANSI into a cell's text.
- colour must be off when stdout is not a terminal or `NO_COLOR` is set; the
  integration tests assert this.

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

`tests/cli_dolt.rs` needs the real `dolt` binary and skips itself when it is
absent, so a green local run does not necessarily mean those ran — CI installs
dolt. Check the skip notices if a dolt change looks suspiciously fine.
