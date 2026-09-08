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
//! is what makes truncation safe: we never cut through an ANSI escape. It also
//! means layout can be shared by two renderers — [`Table::render`], which
//! paints, and [`Table::render_highlighted`], which hands the caller plain text
//! plus the ranges that were going to be coloured. The second exists for shells
//! that colour a region of their own buffer rather than reading escapes.

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

/// Text as one row of a table can hold it: a single line, no control characters.
///
/// A newline in a cell puts the rest of that row on the next screen line and
/// everything after it in the wrong column; in the shell preview, where zsh is
/// counting rows so it can erase them again, it also leaves debris above the
/// prompt. A tab is the same problem with a variable width.
///
/// Not a hypothetical: cells are filled from strings read out of repositories,
/// and dolt's `dolt_log.message` is the whole commit message rather than its
/// subject. The backend trims that itself — see `vcs::dolt::subject_line` — but
/// a renderer that comes apart on data it was handed is the wrong place to be
/// trusting, so every cell is flattened on the way in.
///
/// Collapsed to spaces rather than cut short, so nothing vanishes without the
/// column's own `…` to account for it. Borrowed back untouched in the ordinary
/// case, which is every cell that has no control character in it.
fn single_line(text: String) -> String {
    if !text.contains(|c: char| c.is_control()) {
        return text;
    }
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
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
                text: single_line(text.into()),
                style,
            }],
        }
    }

    /// Append a span. Returns `self` so cells can be built in one expression.
    pub fn push(mut self, text: impl Into<String>, style: Style) -> Self {
        let text = single_line(text.into());
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

    /// The spans that survive truncation to `max` display columns.
    ///
    /// Truncation walks spans and then characters, so it stops on a character
    /// boundary and leaves room for the ellipsis.
    fn visible_spans(&self, max: Option<usize>) -> Vec<Span> {
        let Some(max) = max.filter(|&m| self.width() > m) else {
            return self.spans.clone();
        };

        if max == 0 {
            return Vec::new();
        }

        let budget = max.saturating_sub(symbol::ELLIPSIS.width());
        let mut out = Vec::new();
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
                out.push(Span {
                    text: taken,
                    style: span.style,
                });
            }
        }

        out.push(Span {
            text: symbol::ELLIPSIS.to_string(),
            style: Theme::muted(),
        });
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
        let mut out = String::new();
        for line in self.lines() {
            for span in &line {
                out.push_str(&paint(&span.text, span.style, self.color));
            }
            out.push('\n');
        }
        out
    }

    /// The same table as plain text, plus the ranges that carry a style.
    ///
    /// Offsets are counted in *characters* — not bytes, and not display
    /// columns — because the one consumer is zsh's `region_highlight`, which
    /// indexes the line editor's buffer that way. Nothing is painted: the shell
    /// is holding the text in a variable and does its own colouring, which is
    /// the only way to tint something the terminal has not printed yet.
    ///
    /// Ranges are half-open (`start..end`) and never overlap. Unstyled runs —
    /// padding, gaps, the indent — produce no range at all.
    pub fn render_highlighted(&self) -> (String, Vec<Highlight>) {
        let mut text = String::new();
        let mut ranges: Vec<Highlight> = Vec::new();
        let mut at = 0;

        for line in self.lines() {
            for span in line {
                let start = at;
                at += span.text.chars().count();
                text.push_str(&span.text);

                if span.style == Style::new() || span.text.is_empty() {
                    continue;
                }
                // Adjacent runs of one style are one range, which keeps the
                // list short enough for a shell to loop over.
                match ranges.last_mut() {
                    Some(last) if last.end == start && last.style == span.style => {
                        last.end = at;
                    }
                    _ => ranges.push(Highlight {
                        start,
                        end: at,
                        style: span.style,
                    }),
                }
            }
            text.push('\n');
            at += 1;
        }

        (text, ranges)
    }

    /// Every line of the table, as the styled spans it is laid out from.
    ///
    /// The single layout path. Both renderers consume this, so a change to
    /// alignment or truncation cannot make them disagree.
    fn lines(&self) -> Vec<Vec<Span>> {
        let widths = self.fitted_widths();
        let flex_idx = self.columns.iter().position(|c| c.flex);
        let cap = |i: usize| {
            if Some(i) == flex_idx {
                Some(widths[i])
            } else {
                None
            }
        };

        let mut out = Vec::new();

        // Header row, in the same column geometry as the body.
        let header_cells: Vec<Cell> = self
            .columns
            .iter()
            .map(|c| Cell::styled(c.header.to_uppercase(), Theme::header()))
            .collect();
        out.push(self.line_spans(&header_cells, &widths, &cap));

        if self.rule {
            let span: usize = widths.iter().sum::<usize>() + GAP * widths.len().saturating_sub(1);
            out.push(vec![
                unstyled(INDENT),
                Span {
                    text: symbol::RULE.repeat(span),
                    style: Theme::rule(),
                },
            ]);
        }

        for row in &self.rows {
            out.push(self.line_spans(row, &widths, &cap));
        }

        out
    }

    fn line_spans(
        &self,
        cells: &[Cell],
        widths: &[usize],
        cap: &impl Fn(usize) -> Option<usize>,
    ) -> Vec<Span> {
        let mut line = vec![unstyled(INDENT)];

        for (i, col) in self.columns.iter().enumerate() {
            let empty = Cell::empty();
            let cell = cells.get(i).unwrap_or(&empty);
            let max = cap(i);
            let pad = " ".repeat(widths[i].saturating_sub(cell.rendered_width(max)));

            match col.align {
                Align::Left => {
                    line.extend(cell.visible_spans(max));
                    if i + 1 < self.columns.len() {
                        line.push(unstyled(&pad));
                    }
                }
                Align::Right => {
                    line.push(unstyled(&pad));
                    line.extend(cell.visible_spans(max));
                }
            }

            if i + 1 < self.columns.len() {
                line.push(unstyled(&" ".repeat(GAP)));
            }
        }

        // Left-aligned final columns pad to their width; nothing follows, so
        // trim it back off rather than shipping trailing whitespace.
        while let Some(last) = line.last_mut() {
            while last.text.ends_with(' ') {
                last.text.pop();
            }
            if last.text.is_empty() {
                line.pop();
            } else {
                break;
            }
        }

        line
    }
}

/// A run of the rendered table that carries a style, as character offsets into
/// the text [`Table::render_highlighted`] returned alongside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Highlight {
    pub start: usize,
    /// One past the last character, so `end - start` is the length.
    pub end: usize,
    pub style: Style,
}

fn unstyled(text: &str) -> Span {
    Span {
        text: text.to_string(),
        style: Style::new(),
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

    #[test]
    fn a_newline_in_a_cell_cannot_break_the_row() {
        let cell = Cell::plain("subject\n\nand a body that would land in the next column");
        assert!(!cell.text().contains('\n'), "{:?}", cell.text());
        assert!(
            cell.text().starts_with("subject  and a body"),
            "{:?}",
            cell.text()
        );
    }

    #[test]
    fn tabs_and_carriage_returns_go_the_same_way() {
        let cell = Cell::plain("a\tb\rc");
        assert_eq!(cell.text(), "a b c");
    }

    #[test]
    fn a_pushed_span_is_flattened_too() {
        let cell = Cell::plain("first").push("second\nthird", Style::new());
        assert!(!cell.text().contains('\n'), "{:?}", cell.text());
    }

    #[test]
    fn ordinary_text_is_left_exactly_as_it_was() {
        let text = "✨ ship the 2.0 rewrite ✨ (#420) — with a, comma";
        assert_eq!(Cell::plain(text).text(), text);
    }

    /// What one cell contributes to a line, truncated to `max` columns.
    fn truncated(cell: &Cell, max: usize) -> String {
        cell.visible_spans(Some(max))
            .iter()
            .map(|s| s.text.as_str())
            .collect()
    }

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
        let rendered = truncated(&Cell::plain("日本語です"), 5);
        // 5 columns: two glyphs (4 cols) plus the ellipsis.
        assert_eq!(rendered, "日本…");
        assert!(rendered.width() <= 5);
    }

    #[test]
    fn truncation_spans_style_boundaries() {
        let cell = Cell::plain("abc").push("defghi", Style::new());
        assert_eq!(truncated(&cell, 5), "abcd…");
    }

    /// The characters a highlight covers, which is the only thing that makes
    /// an offset bug visible.
    fn covered(text: &str, h: &Highlight) -> String {
        text.chars().skip(h.start).take(h.end - h.start).collect()
    }

    #[test]
    fn the_highlighted_text_is_what_the_uncoloured_table_renders() {
        let (text, _) = table().render_highlighted();
        assert_eq!(text, table().color(false).render());
    }

    #[test]
    fn offsets_are_characters_not_bytes() {
        // `日本語` is 3 characters, 9 bytes and 6 display columns; a renderer
        // that reported any of the other two would send zsh's region_highlight
        // to the wrong place.
        let mut t = Table::new(["branch"]).no_rule();
        t.push_row(vec![Cell::styled("日本語", Theme::branch())]);

        let (text, ranges) = t.render_highlighted();
        let branch = ranges
            .iter()
            .find(|h| h.style == Theme::branch())
            .expect("the branch cell is styled");

        assert_eq!(covered(&text, branch), "日本語");
        assert_eq!(branch.end - branch.start, 3);
    }

    #[test]
    fn a_highlight_lands_on_its_own_cell_and_no_further() {
        let mut t = Table::new(["alias", "branch"]).no_rule();
        t.push_row(vec![
            Cell::styled("dashboard", Theme::alias()),
            Cell::styled("main", Theme::branch()),
        ]);

        let (text, ranges) = t.render_highlighted();
        for want in ["dashboard", "main"] {
            let style = if want == "main" {
                Theme::branch()
            } else {
                Theme::alias()
            };
            let found = ranges.iter().find(|h| h.style == style).unwrap();
            assert_eq!(covered(&text, found), want);
        }
    }

    #[test]
    fn padding_and_gaps_carry_no_highlight() {
        let mut t = Table::new(["alias", "branch"]).no_rule();
        t.push_row(vec![Cell::plain("api"), Cell::plain("main")]);

        let (_, ranges) = t.render_highlighted();
        // Only the header is styled; the two plain cells and every space
        // between them are left alone.
        assert!(
            ranges.iter().all(|h| h.style == Theme::header()),
            "unstyled text produced {ranges:?}"
        );
    }

    #[test]
    fn adjacent_runs_of_one_style_become_a_single_range() {
        // `↑2 ↓5` is three spans — two styled, one plain space — so it stays
        // three ranges; `↑2 ↑5` sharing a style would still not merge, because
        // the space between them breaks the run. Two touching spans do merge.
        let cell = Cell::styled("ab", Theme::sha()).push("cd", Theme::sha());
        let mut t = Table::new(["x"]).no_rule();
        t.push_row(vec![cell]);

        let (text, ranges) = t.render_highlighted();
        let sha: Vec<_> = ranges.iter().filter(|h| h.style == Theme::sha()).collect();
        assert_eq!(sha.len(), 1);
        assert_eq!(covered(&text, sha[0]), "abcd");
    }

    #[test]
    fn every_line_ends_with_a_newline_the_offsets_account_for() {
        let (text, ranges) = table().render_highlighted();
        assert!(text.ends_with('\n'));
        for h in &ranges {
            assert!(
                !covered(&text, h).contains('\n'),
                "{h:?} spans a line break"
            );
        }
    }

    #[test]
    fn a_truncated_cell_highlights_only_what_survived() {
        let mut t = Table::new(["alias", "subject"])
            .flex(1)
            .no_rule()
            .terminal_width(Some(20));
        t.push_row(vec![
            Cell::plain("api"),
            Cell::styled("a rather long commit subject", Theme::subject().bold()),
        ]);

        let (text, ranges) = t.render_highlighted();
        let subject = ranges
            .iter()
            .find(|h| h.style == Theme::subject().bold())
            .unwrap();
        assert!(text.contains(&covered(&text, subject)));
        assert!(!covered(&text, subject).contains(symbol::ELLIPSIS));
    }

    #[test]
    fn colour_is_off_by_default_and_adds_escapes_when_on() {
        let mut t = Table::new(["branch"]).no_rule();
        t.push_row(vec![Cell::styled("main", Theme::branch())]);

        let plain = t.clone().color(false).render();
        let fancy = t.color(true).render();
        assert!(plain.contains("main") && !plain.contains('\u{1b}'));
        assert!(
            fancy.contains('\u{1b}'),
            "expected ANSI escapes in {fancy:?}"
        );
        assert!(fancy.contains("main"));
    }
}
