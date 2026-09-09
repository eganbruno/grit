//! Turning data into terminal output.
//!
//! Commands build [`table::Table`]s and hand them a [`crate::context::Ctx`]'s
//! colour and width settings; nothing in here knows about the registry or git.

pub mod svg;
pub mod table;
pub mod theme;

pub use svg::{Palette, Screen};
pub use table::{Align, Cell, Highlight, Table, paint};
pub use theme::{ColorChoice, Theme, symbol, zsh_style};

use owo_colors::Style;

/// The one-line summary printed under a table, e.g.
/// `4 repos · 1 dirty · 1 ahead`.
pub fn footer(parts: &[String], color: bool) -> String {
    if parts.is_empty() {
        return String::new();
    }
    let joined = parts.join(&format!(" {} ", symbol::BULLET));
    format!("  {}\n", paint(&joined, Theme::muted(), color))
}

/// `1 repo` / `3 repos`, because "1 repos" looks careless.
pub fn plural(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// The one-line receipt printed under a repo's output during a fan-out, e.g.
/// `  ✓ ok`.
///
/// Indented to sit under the divider rather than level with it, so it reads as
/// belonging to the block above it.
pub fn outcome(note: &str, style: Style, color: bool) -> String {
    format!("  {}\n", paint(note, style, color))
}

/// A dim `── alias ─────` divider, used between repos during a group fan-out.
pub fn divider(label: &str, width: Option<usize>, color: bool) -> String {
    use unicode_width::UnicodeWidthStr;

    // Capped well short of a wide terminal: a divider is a separator, not a
    // banner, and 100 columns of rule drowns out the output it introduces.
    const MAX: usize = 72;

    let lead = symbol::RULE.repeat(2);
    let head = format!("{lead} {label} ");
    let total = width.unwrap_or(MAX).min(MAX);
    let tail = symbol::RULE.repeat(total.saturating_sub(head.width()));

    format!(
        "{}{}{}\n",
        paint(&lead, Theme::rule(), color),
        paint(&format!(" {label} "), Style::new().bold(), color),
        paint(&tail, Theme::rule(), color),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_agrees_with_the_count() {
        assert_eq!(plural(0, "repo", "repos"), "0 repos");
        assert_eq!(plural(1, "repo", "repos"), "1 repo");
        assert_eq!(plural(4, "repo", "repos"), "4 repos");
    }

    #[test]
    fn footer_joins_parts_with_a_bullet() {
        let parts = vec!["4 repos".to_string(), "1 dirty".to_string()];
        assert_eq!(footer(&parts, false), "  4 repos · 1 dirty\n");
    }

    #[test]
    fn footer_of_nothing_is_nothing() {
        assert_eq!(footer(&[], false), "");
    }

    #[test]
    fn divider_labels_and_fills_to_width() {
        use unicode_width::UnicodeWidthStr;
        let d = divider("docs", Some(20), false);
        assert!(d.starts_with("── docs ─"));
        assert_eq!(d.trim_end_matches('\n').width(), 20);
    }
}
