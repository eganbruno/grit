# Contributing to grit

## Getting set up

```bash
cargo test                                      # unit + integration
cargo clippy --all-targets -- -D warnings       # what CI runs
cargo fmt
cargo run -- status                             # against your real registry
```

To try things without touching your real registry, point `GRIT_CONFIG` at a
scratch file:

```bash
GRIT_CONFIG=/tmp/grit-scratch.toml cargo run -- -r test .
```

## Layout

Dependencies point one way only. `commands` uses `registry`, `vcs` and
`render`; none of those three know about each other, or about `commands`.

```
src/
  main.rs         thin: parse, dispatch, turn errors into an exit code
  lib.rs          the module tree
  cli.rs          clap types, and nothing else
  context.rs      Ctx — the registry, colour and terminal width
  error.rs        Error, the library's typed error
  commands/       one file per command; the only layer that prints
  registry/       the set of registered repos and its file on disk
  vcs/            the Vcs trait and its backends
  render/         tables, colours, symbols
tests/
  common/mod.rs   TestEnv — a temp registry and throwaway git repos
  cli_*.rs        one file per command
```

Two rules keep this honest:

- **Commands never spawn processes or touch the filesystem directly.** They go
  through `Registry` and `trait Vcs`. This is what makes them testable, and
  what lets a new VCS backend work without touching any command.
- **Parsers are pure functions.** `vcs/git.rs` separates "run the command" from
  "parse its output" so the parsing — where the bugs are — is tested against
  fixture strings, with no repository on disk.

## Adding a command

Say you want `grit fetch-all`.

**1. Add the variant** in `src/cli.rs`:

```rust
#[derive(Debug, Subcommand)]
pub enum Command {
    // ...
    /// Fetch every registered repo.
    FetchAll(FetchAllArgs),
}

#[derive(Debug, Args)]
pub struct FetchAllArgs {
    /// Only repos carrying this tag.
    #[arg(short, long, value_name = "TAG")]
    pub tag: Option<String>,
}
```

**2. Reserve the name** in `RESERVED_ALIASES` in `src/registry/mod.rs`, so
nobody's alias can shadow it. The test
`cli::tests::every_subcommand_is_reserved` fails if you forget.

**3. Write the command** in `src/commands/fetch_all.rs`:

```rust
use anyhow::Result;

use crate::cli::FetchAllArgs;
use crate::context::Ctx;

pub fn run(args: &FetchAllArgs, ctx: &mut Ctx) -> Result<()> {
    let repos = ctx.registry.select(&[], args.tag.as_deref())?;
    // ...
    Ok(())
}
```

**4. Wire it up** in `src/commands/mod.rs` — declare the module and add the
match arm. The compiler will insist on the arm; it will not remind you about
the `pub mod`.

```rust
pub mod fetch_all;
// ...
Some(Command::FetchAll(args)) => fetch_all::run(args, ctx).map(|_| 0),
```

**5. Test it.** Pure logic gets a `#[cfg(test)]` block in the same file; the
command as a whole gets `tests/cli_fetch_all.rs`:

```rust
mod common;

use common::TestEnv;
use assert_cmd::prelude::*;

#[test]
fn fetches_every_registered_repo() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    env.grit().arg("fetch-all").assert().success();
}
```

`TestEnv` gives each test its own temporary registry and real git repositories,
with the machine's git config neutralised so results do not depend on whose
laptop is running them. See `tests/common/mod.rs` for what it offers.

## Adding a VCS backend

Dolt is the intended next one, and the shape is already there.

1. Add the variant to `VcsKind` in `src/registry/model.rs`, and its `program()`
   and `as_str()` arms.
2. Add `src/vcs/dolt.rs` implementing `Vcs` — four methods: `kind`, `discover`,
   `snapshot`, `exec`.
3. List it in `provider_for` and `all_providers` in `src/vcs/mod.rs`.
   `all_providers` is the order registration tries when detecting what a path
   is, so put the more specific backend first.

Nothing in `commands/`, `render/` or `registry/` should need to change. If it
does, the abstraction is in the wrong place — say so in the PR.

## Style

- Comments explain *why*, not *what*. If a line needs a comment to say what it
  does, rename something instead.
- Test names are sentences: `a_branch_with_no_upstream_has_no_ab_line`, not
  `test_parse_3`. The test list should read as a specification.
- Errors are for humans. Say what went wrong and, on the next line, what to do
  about it — `error.rs` has the pattern.
- British or American spelling, just be consistent within a file.

## Before opening a PR

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs exactly these on Linux and macOS, plus a `cargo check` against the
`rust-version` in `Cargo.toml`.

## Releasing

Releases are built by [`dist`](https://github.com/axodotdev/cargo-dist). The
configuration lives in `[workspace.metadata.dist]` in `Cargo.toml`; the
workflow that consumes it is *generated*, not hand-written, so it is
regenerated rather than edited.

### One-time setup

1. Create the tap repository named in the `tap` key — `eganbruno/homebrew-tap`.
   It can be empty.
2. Add a `HOMEBREW_TAP_TOKEN` secret to this repository: a fine-grained
   personal access token with **Contents: read and write** on the tap repo
   only.
3. Install dist and generate the workflow:

   ```bash
   cargo install cargo-dist --locked
   dist init --yes
   ```

   This writes `.github/workflows/release.yml`. Commit it.

### Cutting a release

```bash
# bump `version` in Cargo.toml, then:
git commit -am "release 0.2.0"
git tag v0.2.0
git push --follow-tags
```

The workflow builds macOS and Linux binaries, publishes a GitHub release with a
`curl | sh` installer, and opens the formula update on the tap. After the first
release, `brew install eganbruno/tap/grit` works.

Re-run `dist init --yes` after changing anything under
`[workspace.metadata.dist]`, or after upgrading dist itself — the generated
workflow is pinned to the `cargo-dist-version` recorded there.
