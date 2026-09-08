//! The shell integration: the scripts, and putting them in a startup file.
//!
//! `grit shell init <shell>` only prints a script; something has to `eval` it
//! from an rc file before any of it runs. That step is the whole difference
//! between the feature working and the feature appearing to do nothing, so
//! `enable` and `disable` do it rather than leaving it to a line someone has to
//! copy correctly.
//!
//! Nothing here happens on its own. grit edits a startup file when asked to and
//! at no other time: a tool that writes to `.zshrc` because it was merely run
//! is a tool people are right to distrust.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The shells with an integration. A domain type rather than a CLI one — see
/// [`crate::render::ColorChoice`] for the same arrangement — so this module
/// does not have to reach up into `cli` for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

impl Shell {
    pub const fn as_str(self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
            Shell::Fish => "fish",
        }
    }

    /// The shell `$SHELL` names, if it is one grit has a script for.
    ///
    /// Matched on the basename so `/opt/homebrew/bin/zsh` and `/bin/zsh` are
    /// both zsh, and so a version suffix does not have to be anticipated.
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var_os("SHELL")?;
        let path = PathBuf::from(raw);
        let name = path.file_name()?.to_str()?;

        [Shell::Zsh, Shell::Bash, Shell::Fish]
            .into_iter()
            .find(|shell| name.starts_with(shell.as_str()))
    }
}

impl std::fmt::Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The integration for a shell, embedded at build time.
///
/// Kept as real files under `src/shell/` rather than string literals so a shell
/// syntax checker, an editor and a reviewer all see them as the shell code they
/// are. `tests/cli_shell.rs` runs `zsh -n` over the output for that reason.
pub fn script(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => include_str!("grit.zsh"),
        Shell::Bash => include_str!("grit.bash"),
        Shell::Fish => include_str!("grit.fish"),
    }
}

/// The line that loads the integration.
pub fn eval_line(shell: Shell) -> String {
    match shell {
        // fish has no `eval "$(...)"`; a pipe into `source` is the idiom.
        Shell::Fish => "grit shell init fish | source".to_string(),
        other => format!("eval \"$(grit shell init {other})\""),
    }
}

/// Markers around what grit wrote, so `disable` can take back exactly that and
/// nothing a person put next to it.
pub(crate) const BEGIN: &str = "# >>> grit shell integration >>>";
pub(crate) const END: &str = "# <<< grit shell integration <<<";

/// What grit found, and what it did about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    AlreadyEnabled,
    Removed,
    NotEnabled,
    /// Loaded by a line grit did not write. Reported rather than edited: a
    /// hand-written line may be doing more than this one does, and guessing
    /// where it ends is how a tool eats somebody's config.
    Managed,
}

/// The block grit writes, markers and all.
fn block(shell: Shell) -> String {
    format!("{BEGIN}\n{}\n{END}\n", eval_line(shell))
}

/// True when the integration is loaded, however it got there.
fn is_enabled(contents: &str) -> bool {
    contents.contains(BEGIN) || mentions_init(contents)
}

/// A line that loads the integration without grit's markers around it.
fn mentions_init(contents: &str) -> bool {
    contents
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| line.contains("grit shell init"))
}

/// `contents` with the block appended, or `None` if it is already loaded.
///
/// Pure, so the interesting half is tested without a startup file to write.
fn with_block(contents: &str, shell: Shell) -> Option<String> {
    if is_enabled(contents) {
        return None;
    }

    let mut out = contents.to_string();
    // A file that does not end in a newline would otherwise get the marker
    // welded onto its last line.
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&block(shell));
    Some(out)
}

/// `contents` with grit's block taken out, or `None` if there is none to take.
///
/// Only ever removes between the markers, and leaves the surrounding text as it
/// was apart from the blank line it inserted in front of them.
fn without_block(contents: &str) -> Option<String> {
    let start = contents.find(BEGIN)?;
    let end = contents[start..].find(END).map(|i| start + i + END.len())?;

    // Taken off the seam, not the end of the file: the block is not always the
    // last thing in there, and trimming trailing newlines instead would eat
    // blank lines somebody meant to keep.
    //
    // Exactly one newline, because `with_block` adds exactly one.
    let mut out = contents[..start].to_string();
    if out.ends_with("\n\n") {
        out.pop();
    }

    let tail = &contents[end..];
    out.push_str(tail.strip_prefix('\n').unwrap_or(tail));

    Some(out)
}

/// Where a shell reads its interactive startup from.
pub fn rc_path(shell: Shell) -> Result<PathBuf> {
    let home = home_dir()?;
    Ok(match shell {
        // Respected by zsh itself, and by anyone who keeps their dotfiles out
        // of `$HOME`.
        Shell::Zsh => match std::env::var_os("ZDOTDIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(".zshrc"),
            _ => home.join(".zshrc"),
        },
        Shell::Bash => home.join(".bashrc"),
        Shell::Fish => match std::env::var_os("XDG_CONFIG_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("fish").join("config.fish"),
            _ => home.join(".config").join("fish").join("config.fish"),
        },
    })
}

fn home_dir() -> Result<PathBuf> {
    etcetera::home_dir().map_err(|e| Error::NoConfigHome(format!("no home directory found ({e})")))
}

/// Add the integration to `path`, creating the file if it is not there.
pub fn enable(path: &Path, shell: Shell) -> Result<Change> {
    let contents = read_rc(path)?;

    if mentions_init(&contents) && !contents.contains(BEGIN) {
        return Ok(Change::Managed);
    }
    let Some(updated) = with_block(&contents, shell) else {
        return Ok(Change::AlreadyEnabled);
    };

    write_rc(path, &updated)?;
    Ok(Change::Added)
}

/// Take the integration back out of `path`.
pub fn disable(path: &Path) -> Result<Change> {
    let contents = read_rc(path)?;

    let Some(updated) = without_block(&contents) else {
        return Ok(if mentions_init(&contents) {
            Change::Managed
        } else {
            Change::NotEnabled
        });
    };

    write_rc(path, &updated)?;
    Ok(Change::Removed)
}

/// A startup file that is not there yet reads as empty: `enable` on a machine
/// with no `.zshrc` should write one, not refuse.
fn read_rc(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(Error::Io {
            context: "could not read the shell startup file",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_rc(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::io(
            "could not create the directory for the shell startup file",
            parent,
        ))?;
    }
    std::fs::write(path, contents)
        .map_err(Error::io("could not write the shell startup file", path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_added_with_a_blank_line_in_front_of_it() {
        let out = with_block("export PATH=/usr/bin\n", Shell::Zsh).unwrap();
        assert!(out.starts_with("export PATH=/usr/bin\n\n"), "{out:?}");
        assert!(out.contains(r#"eval "$(grit shell init zsh)""#), "{out:?}");
        assert!(out.ends_with("\n"), "{out:?}");
    }

    #[test]
    fn a_file_not_ending_in_a_newline_does_not_get_the_marker_welded_on() {
        let out = with_block("setopt nomatch", Shell::Zsh).unwrap();
        assert!(out.starts_with("setopt nomatch\n"), "{out:?}");
        assert!(out.lines().any(|l| l == BEGIN), "{out:?}");
    }

    #[test]
    fn an_empty_file_gets_the_block_and_no_leading_blank_line() {
        let out = with_block("", Shell::Zsh).unwrap();
        assert!(out.starts_with(BEGIN), "{out:?}");
    }

    #[test]
    fn enabling_twice_adds_nothing() {
        let once = with_block("", Shell::Zsh).unwrap();
        assert_eq!(with_block(&once, Shell::Zsh), None);
    }

    #[test]
    fn fish_is_sourced_rather_than_evalled() {
        assert_eq!(eval_line(Shell::Fish), "grit shell init fish | source");
        assert!(eval_line(Shell::Bash).starts_with("eval "));
    }

    #[test]
    fn disabling_restores_the_file_it_started_as() {
        let original = "export PATH=/usr/bin\nsetopt nomatch\n";
        let enabled = with_block(original, Shell::Zsh).unwrap();
        assert_eq!(without_block(&enabled).as_deref(), Some(original));
    }

    #[test]
    fn a_full_cycle_leaves_no_growing_pile_of_blank_lines() {
        let original = "setopt nomatch\n";
        let mut text = original.to_string();
        for _ in 0..5 {
            text = with_block(&text, Shell::Zsh).unwrap();
            text = without_block(&text).unwrap();
        }
        assert_eq!(text, original);
    }

    #[test]
    fn what_is_written_around_the_block_survives_disabling() {
        let text = format!("before\n\n{BEGIN}\neval \"$(grit shell init zsh)\"\n{END}\nafter\n");
        assert_eq!(
            without_block(&text).as_deref(),
            Some("before\nafter\n"),
            "{text:?}"
        );
    }

    #[test]
    fn blank_lines_the_user_put_there_survive_a_cycle() {
        // The seam is trimmed by exactly what was added, so a file that ended
        // in its own blank line still does.
        let original = "setopt nomatch\n\n";
        let enabled = with_block(original, Shell::Zsh).unwrap();
        assert_eq!(without_block(&enabled).as_deref(), Some(original));
    }

    #[test]
    fn disabling_a_file_without_the_block_changes_nothing() {
        assert_eq!(without_block("setopt nomatch\n"), None);
    }

    #[test]
    fn a_hand_written_eval_counts_as_enabled() {
        // Someone who followed the README rather than running `enable`.
        assert!(is_enabled("eval \"$(grit shell init zsh)\"\n"));
        assert_eq!(
            with_block("eval \"$(grit shell init zsh)\"\n", Shell::Zsh),
            None
        );
    }

    #[test]
    fn the_documentation_comment_in_the_script_is_not_mistaken_for_a_live_line() {
        // `grit shell init zsh` prints its own usage as a comment, and someone
        // may well have pasted the whole thing in.
        assert!(!mentions_init("#   eval \"$(grit shell init zsh)\"\n"));
        assert!(!is_enabled("# eval \"$(grit shell init zsh)\"\n"));
    }

    #[test]
    fn every_shell_has_a_script_that_calls_back_into_grit() {
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
            let text = script(shell);
            assert!(!text.is_empty(), "{shell} has an empty script");
            assert!(text.contains("grit shell"), "{shell} never calls back in");
        }
    }
}
