# grit — notes for Claude

A Rust CLI for driving many git repositories by alias. Read
[CONTRIBUTING.md](CONTRIBUTING.md) first — it has the layout, the recipe for
adding a command, and the recipe for adding a VCS backend. This file only
records what is easy to get wrong.

## Commands

```bash
cargo test                                   # 141 tests, ~2s
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

## Rendering

The intended aesthetic is fzf-like: aligned columns, colour for meaning, no
box-drawing. Before changing the table:

- widths are measured with `unicode-width`, not `str::len`
- cells hold styled *spans*, and styling is applied at render time — that is
  what makes truncation safe. Never bake ANSI into a cell's text.
- colour must be off when stdout is not a terminal or `NO_COLOR` is set; the
  integration tests assert this.

## Testing

Parsers in `vcs/git.rs` are pure `&str -> value` functions with a fixture table
— add a case there before reaching for a real repository. `TestEnv` in
`tests/common/mod.rs` is for behaviour that genuinely needs git: it neutralises
`GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` so the developer's own gitconfig cannot
change what the tests see.

## Not yet built

Dolt backend (the trait is shaped for it), interactive TUI, shell completions,
`grit clone`.
