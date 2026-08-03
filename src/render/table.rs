//! A small aligned-column printer.
//!
//! Not a general table library — deliberately. It does exactly what grit's two
//! listing commands need:
//!
//! - columns sized to their content, measured in *display* width so CJK and
//!   emoji do not knock the alignment out
//! - cells built from several differently-coloured pieces (`●3 ○1 ⚑1`)
//! - one column may flex: when the table is wider than the terminal, that
//!   column is the one that shrinks, and its text is truncated with an ellipsis
//!
//! Styling is applied at render time rather than baked into the strings, which
//! is what makes truncation safe: we never cut through an ANSI escape.

use owo_colors::Style;
use unicode_width::UnicodeWidthStr;

use crate::render::theme::{Theme, symbol};

/// Spaces between columns.
const GAP: usize = 2;
/// Left margin, so output does not sit flush against the terminal edge.
const INDENT: &str = "  ";
/// A flex column never shrinks below this, even on a very narrow terminal.
const MIN_FLEX_WIDTH: usize = 8;

/// A run of text sharing one style.
#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// One table cell: zero or more styled spans laid end to end.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    spans: Vec<Span>,
}

impl Cell {
    pub fn empty() -> Self {
        Self::default()
    }

    /// A cell with no styling of its own.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::styled(text, Style::new())
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            spans: vec![Span {
                text: text.into(),
                style,
            }],
        }
    }

    /// Append a span. Returns `self` so cells can be built in one expression.
    pub fn push(mut self, text: impl Into<String>, style: Style) -> Self {
        let text = text.into();
        if !text.is_empty() {
            self.spans.push(Span { text, style });
        }
        self
    }

    /// Append a span preceded by a space, unless the cell is still empty.
    pub fn push_spaced(self, text: impl Into<String>, style: Style) -> Self {
        let text = text.into();
        if text.is_empty() {
            return self;
        }
        if self.is_empty() {
            return self.push(text, style);
        }
        self.push(" ", Style::new()).push(text, style)
    }

    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text.is_empty())
    }

    /// Total display width, ignoring styling.
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.width()).sum()
    }

    /// The unstyled text, for tests and `--json`-adjacent uses.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Render, optionally truncated to `max` display columns.
    ///
    /// Truncation walks spans and then characters, so it stops on a character
    /// boundary and leaves room for the ellipsis.
    fn render(&self, color: bool, max: Option<usize>) -> String {
        let Some(max) = max.filter(|&m| self.width() > m) else {
            return self
                .spans
                .iter()
                .map(|s| paint(&s.text, s.style, color))
                .collect();
        };

        if max == 0 {
            return String::new();
        }

        let budget = max.saturating_sub(symbol::ELLIPSIS.width());
        let mut out = String::new();
        let mut used = 0;

        for span in &self.spans {
            if used >= budget {
                break;
            }
            let mut taken = String::new();
            for ch in span.text.chars() {
                let w = ch.to_string().width();
                if used + w > budget {
                    break;
                }
                taken.push(ch);
                used += w;
            }
            if !taken.is_empty() {
                out.push_str(&paint(&taken, span.style, color));
            }
        }

        out.push_str(&paint(symbol::ELLIPSIS, Theme::muted(), color));
        out
    }

    /// Display width after truncation to `max`.
    fn rendered_width(&self, max: Option<usize>) -> usize {
        match max {
            Some(m) if self.width() > m => m.min(self.width()),
            _ => self.width(),
        }
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell::plain(s)
    }
}

impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell::plain(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone)]
struct Column {
    header: String,
    align: Align,
    /// The column that absorbs the shrinking when space runs out.
    flex: bool,
}

#[derive(Debug, Clone)]
pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
    color: bool,
    /// Available terminal width, if known.
    width: Option<usize>,
    rule: bool,
}

impl Table {
    /// Start a table with the given column headers, all left-aligned.
    pub fn new<const N: usize>(headers: [&str; N]) -> Self {
        Self {
            columns: headers
                .into_iter()
                .map(|h| Column {
                    header: h.to_string(),
                    align: Align::Left,
                    flex: false,
                })
                .collect(),
            rows: Vec::new(),
            color: false,
            width: None,
            rule: true,
        }
    }

    pub fn color(mut self, on: bool) -> Self {
        self.color = on;
        self
    }

    /// Constrain the table to `width` columns. Without this it never truncates.
    pub fn terminal_width(mut self, width: Option<usize>) -> Self {
        self.width = width;
        self
    }

    /// Mark the column that should shrink first. Only one may be set; a later
    /// call replaces an earlier one.
    pub fn flex(mut self, index: usize) -> Self {
        for (i, col) in self.columns.iter_mut().enumerate() {
            col.flex = i == index;
        }
        self
    }

    pub fn align(mut self, index: usize, align: Align) -> Self {
        if let Some(col) = self.columns.get_mut(index) {
            col.align = align;
        }
        self
    }

    pub fn no_rule(mut self) -> Self {
        self.rule = false;
        self
    }

    pub fn push_row(&mut self, cells: Vec<Cell>) {
        debug_assert_eq!(
            cells.len(),
            self.columns.len(),
            "row has {} cells but the table has {} columns",
            cells.len(),
            self.columns.len()
        );
        self.rows.push(cells);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Natural width of each column: the widest of its header and its cells.
    fn natural_widths(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let cells = self
                    .rows
                    .iter()
                    .filter_map(|r| r.get(i))
                    .map(Cell::width)
                    .max()
                    .unwrap_or(0);
                cells.max(col.header.width())
            })
            .collect()
    }

    /// Column widths after fitting to the terminal.
    fn fitted_widths(&self) -> Vec<usize> {
        let mut widths = self.natural_widths();

        let (Some(available), Some(flex_idx)) =
            (self.width, self.columns.iter().position(|c| c.flex))
        else {
            return widths;
        };

        let total: usize = widths.iter().sum::<usize>()
            + GAP * self.columns.len().saturating_sub(1)
            + INDENT.width();

        if total <= available {
            return widths;
        }

        let overflow = total - available;
        widths[flex_idx] = widths[flex_idx]
            .saturating_sub(overflow)
            .max(MIN_FLEX_WIDTH);
        widths
    }

    /// Render the whole table, including a trailing newline per line.
    pub fn render(&self) -> String {
        let widths = self.fitted_widths();
        let flex_idx = self.columns.iter().position(|c| c.flex);
        let cap = |i: usize| {
            if Some(i) == flex_idx {
                Some(widths[i])
            } else {
                None
            }
        };

        let mut out = String::new();

        // Header row, in the same column geometry as the body.
        let header_cells: Vec<Cell> = self
            .columns
            .iter()
            .map(|c| Cell::styled(c.header.to_uppercase(), Theme::header()))
            .collect();
        out.push_str(&self.render_line(&header_cells, &widths, cap));

        if self.rule {
            let span: usize = widths.iter().sum::<usize>() + GAP * widths.len().saturating_sub(1);
            let rule = symbol::RULE.repeat(span);
            out.push_str(INDENT);
            out.push_str(&paint(&rule, Theme::rule(), self.color));
            out.push('\n');
        }

        for row in &self.rows {
            out.push_str(&self.render_line(row, &widths, cap));
        }

        out
    }

    fn render_line(
        &self,
        cells: &[Cell],
        widths: &[usize],
        cap: impl Fn(usize) -> Option<usize>,
    ) -> String {
        let mut line = String::from(INDENT);

        for (i, col) in self.columns.iter().enumerate() {
            let empty = Cell::empty();
            let cell = cells.get(i).unwrap_or(&empty);
            let max = cap(i);
            let body = cell.render(self.color, max);
            let pad = widths[i].saturating_sub(cell.rendered_width(max));

            match col.align {
                Align::Left => {
                    line.push_str(&body);
                    if i + 1 < self.columns.len() {
                        line.push_str(&" ".repeat(pad));
                    }
                }
                Align::Right => {
                    line.push_str(&" ".repeat(pad));
                    line.push_str(&body);
                }
            }

            if i + 1 < self.columns.len() {
                line.push_str(&" ".repeat(GAP));
            }
        }

        // Left-aligned final columns pad to their width; nothing follows, so
        // trim it back off rather than shipping trailing whitespace.
        while line.ends_with(' ') {
            line.pop();
        }
        line.push('\n');
        line
    }
}

/// Apply a style, or don't.
pub fn paint(text: &str, style: Style, color: bool) -> String {
    if color && !text.is_empty() {
        style.style(text).to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        let mut t = Table::new(["alias", "branch"]).color(false);
        t.push_row(vec![Cell::plain("api"), Cell::plain("main")]);
        t.push_row(vec![
            Cell::plain("dashboard"),
            Cell::plain("fix/login-redirect"),
        ]);
        t
    }

    /// Column widths for [`table`]: `dashboard` and `fix/login-redirect` are
    /// the widest entries, and both beat their headers.
    const ALIAS_W: usize = "dashboard".len();
    const BRANCH_W: usize = "fix/login-redirect".len();

    #[test]
    fn columns_are_padded_to_the_widest_entry() {
        let out = table().render();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "  ALIAS      BRANCH");
        assert_eq!(lines[2], "  api        main");
        assert_eq!(lines[3], "  dashboard  fix/login-redirect");
    }

    #[test]
    fn the_rule_spans_the_table_but_not_the_indent() {
        let out = table().render();
        let rule = out.lines().nth(1).unwrap();
        assert!(rule.starts_with("  ─"));
        assert_eq!(rule.trim_start().chars().count(), ALIAS_W + BRANCH_W + GAP);
    }

    #[test]
    fn a_header_wider_than_its_data_sets_the_column_width() {
        let mut t = Table::new(["alias", "description"]).color(false);
        t.push_row(vec![Cell::plain("a"), Cell::plain("x")]);
        assert_eq!(t.render().lines().nth(2).unwrap(), "  a      x");
    }

    #[test]
    fn no_line_carries_trailing_whitespace() {
        for line in table().render().lines() {
            assert!(!line.ends_with(' '), "trailing space in {line:?}");
        }
    }

    #[test]
    fn right_aligned_columns_pad_on_the_left() {
        let mut t = Table::new(["n", "count"])
            .color(false)
            .align(1, Align::Right)
            .no_rule();
        t.push_row(vec![Cell::plain("a"), Cell::plain("7")]);
        t.push_row(vec![Cell::plain("b"), Cell::plain("1234")]);
        let out = t.render();
        let lines: Vec<&str> = out.lines().collect();
        // The `count` header is 5 wide, so that is the column width.
        assert_eq!(lines[0], "  N  COUNT");
        assert_eq!(lines[1], "  a      7");
        assert_eq!(lines[2], "  b   1234");
    }

    #[test]
    fn width_is_measured_in_display_columns_not_bytes() {
        // Each CJK glyph occupies two columns.
        let cell = Cell::plain("日本語");
        assert_eq!(cell.width(), 6);
        assert_eq!(cell.text().len(), 9);
    }

    #[test]
    fn multi_span_cells_sum_their_widths() {
        let cell = Cell::plain("●3")
            .push_spaced("○1", Style::new())
            .push_spaced("⚑1", Style::new());
        assert_eq!(cell.text(), "●3 ○1 ⚑1");
        assert_eq!(cell.width(), 8);
    }

    #[test]
    fn push_spaced_does_not_lead_with_a_space() {
        let cell = Cell::empty().push_spaced("first", Style::new());
        assert_eq!(cell.text(), "first");
    }

    #[test]
    fn the_flex_column_shrinks_when_the_terminal_is_narrow() {
        let mut t = Table::new(["alias", "subject"])
            .color(false)
            .flex(1)
            .no_rule()
            .terminal_width(Some(24));
        t.push_row(vec![
            Cell::plain("api"),
            Cell::plain("a rather long commit subject"),
        ]);
        let line = t.render().lines().nth(1).unwrap().to_string();
        assert!(line.width() <= 24, "{line:?} is {} wide", line.width());
        assert!(line.ends_with(symbol::ELLIPSIS));
    }

    #[test]
    fn a_wide_terminal_leaves_the_flex_column_alone() {
        let mut t = Table::new(["alias", "subject"])
            .color(false)
            .flex(1)
            .no_rule()
            .terminal_width(Some(200));
        t.push_row(vec![
            Cell::plain("api"),
            Cell::plain("add rate limit headers"),
        ]);
        assert_eq!(
            t.render().lines().nth(1).unwrap(),
            "  api    add rate limit headers"
        );
    }

    #[test]
    fn truncation_never_splits_a_wide_character() {
        let cell = Cell::plain("日本語です");
        let rendered = cell.render(false, Some(5));
        // 5 columns: two glyphs (4 cols) plus the ellipsis.
        assert_eq!(rendered, "日本…");
        assert!(rendered.width() <= 5);
    }

    #[test]
    fn truncation_spans_style_boundaries() {
        let cell = Cell::plain("abc").push("defghi", Style::new());
        assert_eq!(cell.render(false, Some(5)), "abcd…");
    }

    #[test]
    fn colour_is_off_by_default_and_adds_escapes_when_on() {
        let plain = Cell::styled("main", Theme::branch()).render(false, None);
        let fancy = Cell::styled("main", Theme::branch()).render(true, None);
        assert_eq!(plain, "main");
        assert!(
            fancy.contains('\u{1b}'),
            "expected ANSI escapes in {fancy:?}"
        );
        assert!(fancy.contains("main"));
    }
}
