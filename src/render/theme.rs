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

/// How zsh should paint text grit styled this way, if it can.
///
/// The shell integration hands zsh the dashboard as plain text plus a list of
/// ranges, and zsh colours them itself through `region_highlight`. That takes
/// a highlight spec rather than ANSI, and the spec vocabulary is smaller than
/// what ANSI can say: `fg=`, `bg=`, `bold`, `underline`, `standout`, and no dim
/// attribute at all. Dimmed text therefore becomes grey 8 — the colour
/// zsh-autosuggestions uses for exactly this job.
///
/// A style with no entry simply goes unpainted, so adding one to [`Theme`] and
/// forgetting it here costs colour, not correctness.
pub fn zsh_style(style: Style) -> Option<&'static str> {
    if style == Style::new() {
        return None;
    }
    ZSH_STYLES
        .iter()
        .find(|(named, _)| named() == style)
        .map(|(_, spec)| *spec)
}

/// Every named style paired with its zsh spelling.
///
/// Function pointers because [`Style`]'s constructors are not `const`. Matching
/// is by equality, so styles that happen to be identical — `header` and `muted`
/// are both dimmed — resolve to whichever comes first. That is harmless: they
/// are the same instruction to zsh as well.
type ZshStyle = (fn() -> Style, &'static str);

static ZSH_STYLES: &[ZshStyle] = &[
    (Theme::header, "fg=8"),
    (Theme::rule, "fg=238"),
    (Theme::alias, "bold"),
    (Theme::branch, "fg=cyan"),
    (Theme::detached, "fg=magenta"),
    (Theme::sha, "fg=yellow"),
    (Theme::kind, "fg=blue"),
    (Theme::ok, "fg=green"),
    (Theme::behind, "fg=red"),
    (Theme::untracked, "fg=blue"),
    (Theme::conflict, "fg=red,bold"),
    (Theme::stash, "fg=magenta"),
    (Theme::banner, "fg=cyan,bold"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_and_never_ignore_the_terminal() {
        assert!(ColorChoice::Always.enabled_for(false));
        assert!(!ColorChoice::Never.enabled_for(true));
    }

    #[test]
    fn every_named_style_has_a_zsh_spelling() {
        for (named, spec) in ZSH_STYLES {
            assert_eq!(zsh_style(named()), Some(*spec));
        }
    }

    #[test]
    fn styles_that_are_the_same_colour_share_a_spelling() {
        // Not an accident worth guarding against — several columns are yellow
        // on purpose — but the lookup must not depend on which one asked.
        for style in [Theme::sha(), Theme::ahead(), Theme::unstaged()] {
            assert_eq!(zsh_style(style), Some("fg=yellow"));
        }
        for style in [Theme::header(), Theme::muted(), Theme::path()] {
            assert_eq!(zsh_style(style), Some("fg=8"));
        }
    }

    #[test]
    fn unstyled_text_asks_zsh_for_nothing() {
        assert_eq!(zsh_style(Theme::subject()), None);
        assert_eq!(zsh_style(Style::new()), None);
    }

    #[test]
    fn auto_requires_a_terminal() {
        // NO_COLOR is process-wide state; only assert the half that does not
        // depend on it, so this test cannot flake under a coloured CI shell.
        assert!(!ColorChoice::Auto.enabled_for(false));
    }
}
