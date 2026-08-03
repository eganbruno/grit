# grit

Work across many git repositories from anywhere, by alias.

Preparing a release that spans four repos means four `cd`s to answer one
question and four more to act on the answer. `grit` gives each repo a short
name and lets you drive it from wherever you happen to be.

```
$ grit status
  ALIAS      BRANCH               SYNC  STATE     COMMIT   SUBJECT                          AGE
  ─────────────────────────────────────────────────────────────────────────────────────────────
  api        feature/rate-limits  ↑2    ●3 ○1     15beeba  add rate limit headers           20m
  dashboard  main                 ✓     clean     57a90bc  bump chart library to 4.2         2d
  docs       main                 ↓1    clean     430125f  document the webhooks endpoint    3d
  webapp     fix/login-redirect   ✓     ○2 ?1 ⚑1  a36d467  restore scroll position on back   6h

  4 repos · 2 dirty · 1 ahead · 1 behind · 1 stash
```

At a glance: `api` has two unpushed commits plus three staged and one modified
file, `docs` has an upstream commit waiting, `webapp` has uncommitted work, an
untracked file and a stash, and `dashboard` is clean and up to date.

## Install

Homebrew, on macOS or Linux:

```bash
brew install eganbruno/tap/grit
```

Or a prebuilt binary, no Rust needed:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/eganbruno/grit/releases/latest/download/grit-installer.sh | sh
```

From source, if you have a Rust toolchain:

```bash
cargo install --git https://github.com/eganbruno/grit --locked
```

`--locked` builds against the committed `Cargo.lock`. Without it Cargo resolves
dependencies afresh, so a bad upstream release can break your install when a
lockfile build would have been fine.

Prebuilt binaries are published for macOS and Linux on both Apple Silicon/ARM
and Intel/x86.

## Quick start

Register the repos you work across. Any path inside a repo works — grit stores
the root — and `--tag` puts repos into groups.

```bash
grit -r api       ~/code/api       --tag release,backend
grit -r dashboard ~/code/dashboard --tag release,frontend
grit -r docs      ~/code/docs      --tag release
grit -r webapp    ~/code/webapp    --tag frontend
```

`grit show` lists what you have registered:

```
$ grit show
  ALIAS      KIND  TAGS               PATH
  ────────────────────────────────────────────────────
  api        git   backend, release   ~/code/api
  dashboard  git   frontend, release  ~/code/dashboard
  docs       git   release            ~/code/docs
  webapp     git   frontend           ~/code/webapp

  4 repos · tags: backend, frontend, release · ~/.config/grit/config.toml
```

Then, from anywhere:

```bash
grit status                     # dashboard across every repo
grit status --tag release       # just the release group
grit status api docs            # just these two

grit docs commit -am "changelog"  # run git in the docs repo
grit api log --oneline -n 10      # any git command, flags and all
grit @release fetch               # run it in every repo tagged `release`
```

## Commands

| Command | What it does |
| --- | --- |
| `grit -r <alias> [path]` | Register a repo. `path` defaults to `.`. Long form: `grit register`. |
| `grit rm <alias>...` | Forget an alias. The repository itself is untouched. |
| `grit show` | Every registered repo: alias, kind, tags, path. |
| `grit status` | Branch, sync state and working-tree state for each repo. |
| `grit <alias> <args...>` | Run git in that repo with those arguments. |
| `grit @<tag> <args...>` | Run it in every repo carrying that tag. |

`show` and `status` both take `--tag <TAG>` and `--json`. `status` also takes a
list of aliases: `grit status api docs`.

### Reading the dashboard

| Column | Meaning |
| --- | --- |
| `SYNC` | `✓` in sync · `↑n` ahead · `↓n` behind · `·` no upstream |
| `STATE` | `●n` staged · `○n` unstaged · `?n` untracked · `!n` conflicts · `⚑n` stashes |

An in-progress merge, rebase, cherry-pick, revert or bisect is named in `STATE`.
A registered path that has been deleted shows up as `missing` rather than
breaking the rest of the dashboard.

### Passthrough

`grit <alias> <args...>` runs git with its stdio inherited, so it behaves
exactly as it would if you had `cd`'d there first — your pager pages, colour
stays on, `$EDITOR` opens for a commit message — and git's exit code becomes
grit's exit code.

The child runs *in the repo root*, so relative paths in arguments resolve
against the repo, not against your current directory.

A group fan-out runs sequentially and stops at the first repo that fails. Pass
`-k` before the target to keep going:

```bash
grit -k @release fetch
```

## Configuration

`~/.config/grit/config.toml`, or wherever `$GRIT_CONFIG` points. It is plain
TOML and meant to be edited by hand or checked into your dotfiles:

```toml
version = 1

[repos.api]
path = "/home/you/code/api"
kind = "git"
tags = ["backend", "release"]
added_at = "2026-08-03T11:49:22Z"
```

Colour follows [`NO_COLOR`](https://no-color.org) and switches off when stdout
is not a terminal. `--color always|never|auto` overrides both.

## How it works

grit shells out to the real `git` binary rather than linking a git library.
That keeps the dependency tree pure Rust, guarantees your own git config is
honoured, and means the dashboard and the passthrough use one mechanism instead
of two.

Everything grit does to a repository goes through one trait, `vcs::Vcs`.
Supporting dolt — or any other git-like system — is one new implementation of
four methods; no command, renderer or registry code changes.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the layout, the recipe for adding a
command, and how to run the tests.

## License

MIT or Apache-2.0, at your option.
