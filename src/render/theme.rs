//! Colours and symbols.
//!
//! Every colour grit prints is named here, so the palette can be judged (and
//! changed) in one place instead of being scattered through the commands.
//!
//! The vocabulary is deliberately small and borrowed from git's own defaults —
//! yellow hashes, cyan refs — so output looks at home next to `git log`.

use owo_colors::{Style, XtermColors};

/// Whether to emit ANSI escapes.
///
/// `Auto` means: colour when stdout is a terminal and `NO_COLOR` is unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[value(rename_all = "lower")]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    /// Resolve to a yes/no for the given stream.
    ///
    /// `NO_COLOR` is honoured per <https://no-color.org>: set, to anything
    /// non-empty, means no colour.
    pub fn enabled_for(self, stream_is_terminal: bool) -> bool {
        match self {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => stream_is_terminal && !no_color_env(),
        }
    }
}

fn no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Symbols used in the status column. Single-width so columns stay aligned.
pub mod symbol {
    pub const SYNCED: &str = "✓";
    pub const AHEAD: &str = "↑";
    pub const BEHIND: &str = "↓";
    pub const STAGED: &str = "●";
    pub const UNSTAGED: &str = "○";
    pub const UNTRACKED: &str = "?";
    pub const CONFLICT: &str = "!";
    pub const STASH: &str = "⚑";
    pub const NONE: &str = "·";
    pub const ELLIPSIS: &str = "…";
    pub const RULE: &str = "─";
    pub const BULLET: &str = "·";
}

/// The named styles. Grouped by what they mean, not by colour, so a repaint is
/// a one-line change here.
pub struct Theme;

impl Theme {
    pub fn header() -> Style {
        Style::new().dimmed()
    }
    pub fn rule() -> Style {
        Style::new().color(XtermColors::from(238))
    }
    pub fn alias() -> Style {
        Style::new().bold()
    }
    pub fn branch() -> Style {
        Style::new().cyan()
    }
    pub fn detached() -> Style {
        Style::new().magenta()
    }
    pub fn sha() -> Style {
        Style::new().yellow()
    }
    pub fn subject() -> Style {
        Style::new()
    }
    pub fn muted() -> Style {
        Style::new().dimmed()
    }
    pub fn path() -> Style {
        Style::new().dimmed()
    }
    pub fn tag() -> Style {
        Style::new().magenta()
    }
    pub fn kind() -> Style {
        Style::new().blue()
    }

    pub fn ok() -> Style {
        Style::new().green()
    }
    pub fn ahead() -> Style {
        Style::new().yellow()
    }
    pub fn behind() -> Style {
        Style::new().red()
    }
    pub fn staged() -> Style {
        Style::new().green()
    }
    pub fn unstaged() -> Style {
        Style::new().yellow()
    }
    pub fn untracked() -> Style {
        Style::new().blue()
    }
    pub fn conflict() -> Style {
        Style::new().red().bold()
    }
    pub fn stash() -> Style {
        Style::new().magenta()
    }
    pub fn warn() -> Style {
        Style::new().red()
    }
    pub fn banner() -> Style {
        Style::new().bold().cyan()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_and_never_ignore_the_terminal() {
        assert!(ColorChoice::Always.enabled_for(false));
        assert!(!ColorChoice::Never.enabled_for(true));
    }

    #[test]
    fn auto_requires_a_terminal() {
        // NO_COLOR is process-wide state; only assert the half that does not
        // depend on it, so this test cannot flake under a coloured CI shell.
        assert!(!ColorChoice::Auto.enabled_for(false));
    }
}
