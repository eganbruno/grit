# Changelog

## [0.3.0] - 2026-09-11

Two new read-only commands and a way to move between repositories without
leaving the prompt.

### Added

- **`grit branch`** — every branch of every repository, one line each: branch,
  upstream, sync, subject and age. `grit @tag branch -vv` could not be made to
  work, because passthrough hands the child inherited stdio and both git and
  dolt print a commit *message* where a table wants a subject, so one merge
  commit with a rationale in its body turned a fifteen-branch listing into
  pages. The listing is grit's own, read through `trait Vcs` and cut while the
  data is still structured.

  A distance nobody measured renders as an empty `SYNC` rather than a `✓` it
  has not earned. Counting for every branch of a dolt database costs a query
  each, so only the checked-out branch is measured.

- **`grit detail <alias>`** — one repository as a card: the header the dashboard
  would have shown, then a section each for the changed paths, the recent
  commits, the branches and the stash stack. Empty sections are left out rather
  than headed and empty. `--json` carries the same reading. It always takes a
  live reading; a card drawn from the cache would say "clean" about a repository
  you had just edited.

- **A repository picker on `^G^G`** — every registered repo on the left, that
  card in the preview beside it. `enter` puts `grit <alias> ` on the line,
  `ctrl-d` cd's there, `ctrl-r` takes a fresh reading. fzf does the navigating
  and grit spawns nothing: the widget runs in your own shell, which is what
  lets `ctrl-d` change its directory. Needs fzf; `^G^P` still draws the inline
  table, and neither is bound to a bare `^G`.

- `grit shell rows` and `grit shell path`, which supply the picker its list and
  its answer without the shell having to parse JSON.

### Changed

- The dashboard's commit subject is capped at 60 columns — git's conventional
  50-column subject with a ticket reference after it — whatever the terminal is
  doing, including when stdout is a pipe and there is no width to fit to. A
  subject has no length anyone agreed to: merge subjects carry a remote URL, a
  squashed pull request arrives with its title and number, and one talkative
  repository used to set the geometry for every other row.

### Fixed

- The detail card inside an fzf preview was laid out to the width of the whole
  terminal rather than of the pane, so every branch row was folded onto a second
  line and the columns stopped lining up. fzf exports the pane's width as
  `$COLUMNS`, but runs the preview through `$SHELL -c`, where zsh re-derives
  `COLUMNS` from the tty before grit is reached. `$FZF_PREVIEW_COLUMNS` is now
  read first.

- The picker's `ctrl-d` changed the shell's directory and then left the *old*
  one on the prompt until the next Enter, so it read as having done nothing.
  The cd was always happening; the prompt was not being rebuilt. Prompts
  assembled in `precmd` — powerlevel10k, starship, oh-my-posh — are the ones
  affected, and bash has the same hole through `PROMPT_COMMAND`.

- `typeset -gir` in the zsh integration made the *second* sourcing of the script
  a fatal error at that line, so the eval aborted and every widget, hook and
  binding below it was silently never installed. Re-sourcing is what anyone does
  to pick up a new grit without a new terminal.

- `GRIT_PREVIEW_TRIGGERS=( ${BUFFER} )` was unquoted, and an unquoted empty
  expansion builds an array of no elements — so the on-demand preview key never
  worked on an empty command line.

- A branch distance that had not been measured counted as zero, and zero
  rendered as `✓ in sync`. git still names the upstream of a branch whose
  upstream was deleted, and dolt's counts come from an invocation that can fail,
  so an unmeasured distance is now modelled as one.

---

Releases before 0.3.0 predate this file; see the
[releases page](https://github.com/eganbruno/grit/releases) for those.
