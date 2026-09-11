# Changelog

## [0.3.0] - 2026-09-11

This release makes it easier to find a repository, see what you were working
on, and pick up where you left off.

### Added

- **Browse your repositories from the terminal.** Press **Ctrl+G, then Ctrl+G**
  to open a searchable list of your registered repositories, with details of
  the selected repository alongside it. Press **Enter** to put `grit <alias> `
  on the command line, ready for you to add a command; **Ctrl+D** to move into
  its directory; or **Ctrl+R** to refresh the list. Available with grit's shell
  integration in zsh, bash and fish. Requires
  [fzf](https://github.com/junegunn/fzf), a terminal search tool.

- **See one repository in detail with `grit detail <alias>`.** Get a fresh
  overview of its current branch, uncommitted changes, recent commits, local
  branches and stashes (changes saved for later). Works with Git and Dolt.
  Use `--json` to read the results from a script.

- **View branches across repositories with `grit branch`.** See local branches
  in one table, including their latest commit and how they compare with the
  remote branch they track, when available. Choose repositories by alias, or
  use `--tag <tag>` to show a group. Works with Git and Dolt and supports
  `--json`. For Dolt, ahead/behind counts are shown only for the current branch.

### Changed

- **The dashboard shortcut is now Ctrl+G, then Ctrl+P**, replacing Ctrl+G
  alone. Ctrl+G, then Ctrl+G opens the new repository picker. If you set a
  custom shortcut with `GRIT_PREVIEW_KEY`, that setting still applies.
- Long commit titles are shortened to keep the existing status dashboard
  readable, including when its output is sent to another command or a file.

### Fixed

- Reloading grit's zsh integration now works without opening a new terminal.
- In zsh, the dashboard shortcut now also works on an empty command line.

---

Releases before 0.3.0 predate this file; see the
[releases page](https://github.com/eganbruno/grit/releases) for those.
