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
  cache.rs        the last dashboard read, so one can be shown instantly
  commands/       one file per command; the only layer that prints
  registry/       the set of registered repos and its file on disk
  vcs/            the Vcs trait and its backends
  render/         tables, colours, symbols
  shell/          the integration scripts, and installing them into an rc file
tests/
  common/mod.rs   TestEnv — a temp registry and throwaway git repos
  cli_*.rs        one file per command, plus one per VCS backend
  shell/          a pty harness for the integrations; run by hand
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

## Working on the shell integration

`grit shell init <shell>` prints a script from `src/shell/`, embedded with
`include_str!`. They are kept as real `.zsh`/`.bash`/`.fish` files so an editor,
a linter and a reviewer all see shell code, and so `tests/cli_shell.rs` can run
`zsh -n` over what actually ships. That test skips when the shell is missing, so
check the skip notices before believing a green run.

The zsh script draws the dashboard *below the line you are typing*, which is a
different problem from printing it:

- The text lives in zsh's `POSTDISPLAY`, which is rendered literally. **ANSI in
  there appears on screen as `ESC [ 3 6 m`.** Colour has to go across as
  `region_highlight` ranges instead, which is why `grit shell preview` emits
  plain text and a list of offsets rather than a painted table.
- Those offsets are **characters**. Not bytes — a branch name with an accent in
  it would shift everything after it — and not display columns, which a CJK
  commit subject would break. `Table::render_highlighted` counts `chars()`, and
  the tests slice the text with the offsets to prove it.
- The preview has to be instant, so it renders `cache.rs` and never opens a
  repository. The integration starts a throttled `grit shell refresh` behind it.
- The idle timer is `zsh/sched`. `TMOUT` looks like the obvious answer and is
  not: it can only be armed when a line read *begins*, so it cannot be turned on
  when the buffer becomes the trigger, and merely defining a `TRAPALRM` to go
  with it silently disables a user's auto-logout.

If you change anything under `src/shell/`, run it against a real interactive
shell rather than reading it and believing yourself:

```bash
cargo build && python3 tests/shell/preview.py
```

That types into zsh, bash and fish on a pseudo-terminal and asserts against the
bytes they write back — the table appears, the colours land on the right cells,
a keystroke erases it, Ctrl-C strands nothing, and a user who already has
`TMOUT` set keeps their auto-logout. It is not part of `cargo test` because it
waits on a one-second timer several times and would flake on a loaded runner;
that is a reason to run it by hand, not a reason to skip it. Nearly every bug
this feature had was of the kind where the script reads correctly and the
screen is wrong, and nothing but a terminal will show you those.

## Adding a VCS backend

Two exist: `vcs/git.rs` and `vcs/dolt.rs`. Say you want `jj`.

1. Add the variant to `VcsKind` in `src/registry/model.rs`, and its `as_str()`
   arm.
2. Add `src/vcs/jj.rs` implementing `Vcs` — four methods: `kind`, `discover`,
   `snapshot`, `exec`.
3. List it in `provider_for` and `all_providers` in `src/vcs/mod.rs`.
   `all_providers` is the order registration tries when detecting what a path
   is, so put the more specific backend first: dolt precedes git so that a
   database inside a git working tree is registered as the database.
4. Add `tests/cli_jj.rs`. Backends need the real binary, which not every
   machine has — follow `cli_dolt.rs` and skip when it is missing, then install
   it in `ci.yml` so the tests still run somewhere.

Nothing in `commands/`, `render/` or `registry/` should need to change. If it
does, the abstraction is in the wrong place — say so in the PR.

### What dolt taught us

The trap is assuming a git-like tool is git with a different binary name. Dolt
borrows git's vocabulary but almost none of its plumbing: no `rev-parse`, no
porcelain format for `status`, and a `.dolt` directory holding a database rather
than refs and marker files. A backend written by copying `git.rs` and renaming
the binary compiles and is wrong at every call site.

So before writing one, find that backend's *stable, machine-readable*
interface and check each reading against the real tool. For dolt that is SQL —
the `dolt_*` system tables — which is also why `Error::BadOutput` exists.
Specifically worth knowing, if only as a flavour of what to look for:

- **Not every column has a source.** Dolt has no detached HEAD and no rebase or
  bisect state, so some `RepoState` variants are simply unreachable there. Leave
  them unreachable and say why; do not invent a signal.
- **Detection needs a positive marker.** `~/.dolt` is dolt's *global config*, so
  testing for `.dolt` alone reports a home directory as a repository.
- **Time zones.** `dolt_log.date` is UTC while its `now()` is local; the obvious
  query makes every age wrong by the machine's offset.
- **Names are data.** Dolt permits a `'` in a branch name, so anything
  interpolated into a query has to be escaped.
- **The same query can answer in two shapes.** When a `dolt sql-server` holds
  the database, `dolt sql` stops opening it directly and becomes a MySQL client,
  and the wire protocol renders every value as a string — `"0"` instead of `0`
  or `false`. Nothing about the command changes, so this is invisible until you
  test against a repo someone is serving. It is why `vcs/dolt.rs` reads numbers
  through `wire_u32` rather than as plain `u32`.

The moral of the last one is worth stating plainly: probe a *real* repository,
not only a freshly created one. Every trap above came from doing that, and the
server case came from a repository the author was actually using.

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

Releases are built by [`dist`](https://github.com/axodotdev/cargo-dist).

`.github/workflows/release.yml` is **generated** from
`[workspace.metadata.dist]` in `Cargo.toml`. Treat it like a lockfile: it is
committed, but do not hand-edit it. Change the config and regenerate:

```bash
cargo install cargo-dist --locked   # once
dist init --yes                     # rewrites release.yml
dist plan                           # shows what a release would produce
```

The same applies after upgrading dist, since the workflow is pinned to the
`cargo-dist-version` recorded in the config.

### One-time setup

Both steps are already done for this repository; they are recorded for anyone
forking it.

1. Create the tap repository named in the `tap` key — `eganbruno/homebrew-tap`.

   It needs **at least one commit on the default branch**. A repository with no
   commits has no `main` for the publish job to check out, and the job fails
   with `fatal: couldn't find remote ref refs/heads/main` — after the binaries
   have already been built and the release published. Adding a README is
   enough.
2. Add a `HOMEBREW_TAP_TOKEN` secret to *this* repository: a fine-grained
   personal access token scoped to the tap repo only, with
   **Contents: read and write**.

   The token is needed because the automatic `GITHUB_TOKEN` is scoped to this
   repository alone, and the publish job has to push a formula to another one.

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
