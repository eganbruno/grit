//! The dashboard as an SVG, for places that render pictures but not colour.
//!
//! GitHub's markdown gives no way to colour text: a fenced block is plain by
//! construction, and `style` attributes and inline `<svg>` are both stripped.
//! So the README's headline example — the one whose whole point is that colour
//! carries meaning — can only be painted as a linked image.
//!
//! The risk in that is drift. A screenshot is a second copy of the palette, and
//! it goes stale the first time [`Theme`] is repainted, quietly, in the one
//! place a newcomer looks first. Hence this: not a picture of the dashboard but
//! the dashboard itself, rendered through the same [`Table`] the terminal gets
//! and coloured from the same [`Theme`]. `cargo test` compares the committed
//! files against a fresh render, so a repaint fails the build instead.
//!
//! A third renderer, but not a third layout: like `render_highlighted`, this
//! consumes plain text plus styled ranges, so alignment cannot disagree with
//! what the terminal prints.

use owo_colors::Style;

use crate::render::table::{Highlight, Table};
use crate::render::theme::ink;

/// Monospace geometry, in CSS pixels.
///
/// Every run is positioned absolutely at its own column, so a viewer whose
/// monospace font advances slightly differently from `ADVANCE` shifts glyphs
/// *within* a run rather than pulling the columns apart. That is the whole
/// reason for placing runs individually instead of letting one `<text>` flow.
const FONT_SIZE: f64 = 14.0;
const ADVANCE: f64 = 8.4;
const LINE_HEIGHT: f64 = 20.0;
const PAD_X: f64 = 16.0;
const PAD_Y: f64 = 16.0;

/// What a colour slot means in one medium.
///
/// Not a transcription of the xterm palette. The terminal's grey 238 sits on
/// whatever background the reader chose, and against a page it lands somewhere
/// between subtle and invisible — a rule nobody can see is not the subtle
/// separator the theme was after, it is a missing one. The numbered slots are
/// therefore resolved per medium, and only the named hues stay recognisably
/// themselves.
///
/// Tokens are the zsh colour names [`ink`] hands back; anything not listed
/// falls back to `foreground`, so a new colour in [`Theme`] renders legibly
/// while looking slightly wrong, rather than vanishing.
pub struct Palette {
    pub name: &'static str,
    pub background: &'static str,
    pub foreground: &'static str,
    pub colors: &'static [(&'static str, &'static str)],
}

impl Palette {
    fn resolve(&self, token: &str) -> &str {
        self.colors
            .iter()
            .find(|(name, _)| *name == token)
            .map(|(_, hex)| *hex)
            .unwrap_or(self.foreground)
    }
}

/// Primer's dark palette, so the image sits in a GitHub dark page rather than
/// glowing out of it.
pub const DARK: Palette = Palette {
    name: "dark",
    background: "#0d1117",
    foreground: "#c9d1d9",
    colors: &[
        ("green", "#3fb950"),
        ("yellow", "#d29922"),
        ("red", "#f85149"),
        ("blue", "#58a6ff"),
        ("magenta", "#bc8cff"),
        ("cyan", "#39c5cf"),
        ("8", "#6e7681"),
        ("238", "#3d444d"),
    ],
};

/// Primer's light palette. The same hues, darkened to keep contrast on white.
pub const LIGHT: Palette = Palette {
    name: "light",
    background: "#ffffff",
    foreground: "#24292f",
    colors: &[
        ("green", "#1a7f37"),
        ("yellow", "#9a6700"),
        ("red", "#cf222e"),
        ("blue", "#0969da"),
        ("magenta", "#8250df"),
        ("cyan", "#1b7c83"),
        ("8", "#6e7781"),
        ("238", "#afb8c1"),
    ],
};

/// Lines of styled text, accumulated the way a terminal session reads.
///
/// Holds exactly what `Table::render_highlighted` produces — text plus ranges
/// over it — so a table, a shell prompt line and a footer can share one
/// coordinate space.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    text: String,
    highlights: Vec<Highlight>,
    /// Characters written so far, which is where the next range starts.
    at: usize,
}

impl Screen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one line, all of it in `style`.
    pub fn line(&mut self, text: &str, style: Style) {
        let start = self.at;
        self.at += text.chars().count();
        self.text.push_str(text);

        if style != Style::new() && !text.is_empty() {
            self.highlights.push(Highlight {
                start,
                end: self.at,
                style,
            });
        }

        self.text.push('\n');
        self.at += 1;
    }

    pub fn blank(&mut self) {
        self.line("", Style::new());
    }

    /// Append a whole table, keeping its highlight ranges aligned.
    pub fn table(&mut self, table: &Table) {
        let (text, highlights) = table.render_highlighted();
        let offset = self.at;

        self.highlights
            .extend(highlights.into_iter().map(|h| Highlight {
                start: h.start + offset,
                end: h.end + offset,
                style: h.style,
            }));

        self.at += text.chars().count();
        self.text.push_str(&text);
    }

    /// The plain text, exactly as a terminal without colour would show it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Render to a standalone SVG document.
    pub fn to_svg(&self, palette: &Palette) -> String {
        let chars: Vec<char> = self.text.chars().collect();

        // A style per character, so runs can be recovered by grouping equal
        // neighbours. The alternative — walking highlights and inferring the
        // unstyled gaps between them — is the same work with off-by-one bugs.
        let mut style_at = vec![Style::new(); chars.len()];
        for h in &self.highlights {
            for slot in style_at
                .iter_mut()
                .take(h.end.min(chars.len()))
                .skip(h.start)
            {
                *slot = h.style;
            }
        }

        let lines = self.runs(&chars, &style_at);
        let columns = lines
            .iter()
            .flat_map(|runs| runs.iter().map(|(col, text, _)| col + text.chars().count()))
            .max()
            .unwrap_or(0);

        let width = PAD_X * 2.0 + columns as f64 * ADVANCE;
        let height = PAD_Y * 2.0 + lines.len() as f64 * LINE_HEIGHT;

        let mut out = String::new();
        out.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {:.0} {:.0}\" \
             width=\"{:.0}\" height=\"{:.0}\" font-family=\"ui-monospace, SFMono-Regular, \
             Menlo, Consolas, monospace\" font-size=\"{FONT_SIZE}\" role=\"img\">\n",
            width, height, width, height
        ));

        // The plain text, for a screen reader and for anyone who wants to read
        // the file rather than look at it — some of what a fenced block gave.
        out.push_str("  <title>grit status</title>\n  <desc>\n");
        for line in self.text.lines() {
            out.push_str(&format!("{}\n", escape(line)));
        }
        out.push_str("  </desc>\n");

        out.push_str("  <style>\n");
        out.push_str(&format!("    .fg {{ fill: {} }}\n", palette.foreground));
        out.push_str("    .b { font-weight: 600 }\n");
        for (token, _) in palette.colors {
            out.push_str(&format!(
                "    .c-{} {{ fill: {} }}\n",
                token,
                palette.resolve(token)
            ));
        }
        out.push_str("  </style>\n");

        out.push_str(&format!(
            "  <rect width=\"100%\" height=\"100%\" rx=\"6\" fill=\"{}\"/>\n",
            palette.background
        ));

        for (row, runs) in lines.iter().enumerate() {
            if runs.is_empty() {
                continue;
            }
            // Baseline: most of the line box above it, a little below.
            let y = PAD_Y + row as f64 * LINE_HEIGHT + FONT_SIZE;
            out.push_str(&format!("  <text y=\"{y:.1}\" xml:space=\"preserve\">"));
            for (col, text, style) in runs {
                let x = PAD_X + *col as f64 * ADVANCE;
                out.push_str(&format!(
                    "<tspan x=\"{:.1}\" class=\"{}\">{}</tspan>",
                    x,
                    class_of(*style),
                    escape(text)
                ));
            }
            out.push_str("</text>\n");
        }

        out.push_str("</svg>\n");
        out
    }

    /// Group each line into runs of one style, trimmed to their ink.
    ///
    /// Padding is dropped rather than carried: a run is placed at its own
    /// column, so leading spaces inside one would position it twice — once by
    /// `x` and once by however wide the viewer's font makes a space. Trimming
    /// and advancing the column instead is what keeps the columns square in a
    /// font whose advance is not exactly [`ADVANCE`]. Runs that were only
    /// padding disappear entirely, which is also most of the file's size.
    #[allow(clippy::type_complexity)]
    fn runs(&self, chars: &[char], style_at: &[Style]) -> Vec<Vec<(usize, String, Style)>> {
        let mut lines = Vec::new();
        let mut runs: Vec<(usize, String, Style)> = Vec::new();
        let mut col = 0;

        for (i, &ch) in chars.iter().enumerate() {
            if ch == '\n' {
                lines.push(trimmed(std::mem::take(&mut runs)));
                col = 0;
                continue;
            }

            match runs.last_mut() {
                Some((_, text, style)) if *style == style_at[i] => text.push(ch),
                _ => runs.push((col, ch.to_string(), style_at[i])),
            }
            col += 1;
        }

        if !runs.is_empty() {
            lines.push(trimmed(runs));
        }

        lines
    }
}

/// Strip each run back to its non-blank text, moving its column to match, and
/// drop the ones that held nothing else.
fn trimmed(runs: Vec<(usize, String, Style)>) -> Vec<(usize, String, Style)> {
    runs.into_iter()
        .filter_map(|(col, text, style)| {
            let lead = text.chars().take_while(|c| *c == ' ').count();
            let text = text.trim().to_string();
            (!text.is_empty()).then_some((col + lead, text, style))
        })
        .collect()
}

/// The CSS classes a style asks for.
fn class_of(style: Style) -> String {
    let ink = ink(style);
    let mut classes = match ink.color {
        Some(token) => format!("c-{token}"),
        None => "fg".to_string(),
    };
    if ink.bold {
        classes.push_str(" b");
    }
    classes
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::table::Cell;
    use crate::render::theme::Theme;

    fn screen() -> Screen {
        let mut table = Table::new(["alias", "sync"]).no_rule();
        table.push_row(vec![
            Cell::styled("api", Theme::alias()),
            Cell::styled("✓", Theme::ok()),
        ]);

        let mut screen = Screen::new();
        screen.line("$ grit status", Theme::muted());
        screen.table(&table);
        screen
    }

    #[test]
    fn the_plain_text_is_what_an_uncoloured_terminal_would_print() {
        let mut table = Table::new(["alias"]).color(false).no_rule();
        table.push_row(vec![Cell::styled("api", Theme::alias())]);

        let mut screen = Screen::new();
        screen.table(&table);
        assert_eq!(screen.text(), table.render());
    }

    #[test]
    fn a_tables_highlights_are_shifted_by_the_lines_above_it() {
        let svg = screen().to_svg(&DARK);
        // `api` is bold; if the offset were not shifted past the prompt line,
        // the range would land on `$ grit status` instead.
        assert!(svg.contains(">api</tspan>"), "{svg}");
        let bold = svg
            .lines()
            .find(|l| l.contains(">api</tspan>"))
            .expect("a run for the alias");
        assert!(bold.contains("class=\"fg b\""), "{bold}");
    }

    #[test]
    fn every_colour_in_the_theme_reaches_the_stylesheet() {
        let svg = screen().to_svg(&DARK);
        for (token, hex) in DARK.colors {
            assert!(
                svg.contains(&format!(".c-{token} {{ fill: {hex} }}")),
                "{token}"
            );
        }
    }

    #[test]
    fn the_two_palettes_paint_the_same_geometry() {
        let (dark, light) = (screen().to_svg(&DARK), screen().to_svg(&LIGHT));
        let strip = |s: &str| {
            s.lines()
                .filter(|l| !l.contains("fill:") && !l.contains("fill=\""))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(strip(&dark), strip(&light));
    }

    #[test]
    fn padding_produces_no_runs_of_its_own() {
        let mut table = Table::new(["alias", "branch"]).no_rule();
        table.push_row(vec![Cell::plain("a"), Cell::plain("main")]);
        let mut screen = Screen::new();
        screen.table(&table);

        let svg = screen.to_svg(&DARK);
        assert!(!svg.contains("> </tspan>"), "{svg}");
        assert!(!svg.contains("></tspan>"), "{svg}");
    }

    #[test]
    fn markup_in_a_commit_subject_cannot_escape_the_document() {
        let mut table = Table::new(["subject"]).no_rule();
        table.push_row(vec![Cell::plain("fix <script> & \"quotes\"")]);
        let mut screen = Screen::new();
        screen.table(&table);

        let svg = screen.to_svg(&DARK);
        assert!(svg.contains("fix &lt;script&gt; &amp; \"quotes\""), "{svg}");
        assert!(!svg.contains("<script>"), "{svg}");
    }

    #[test]
    fn the_document_is_self_contained_and_well_formed_enough_to_embed() {
        let svg = screen().to_svg(&LIGHT);
        assert!(svg.starts_with("<svg xmlns="));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches("<text").count(), svg.matches("</text>").count());
    }

    #[test]
    fn the_description_carries_the_whole_table_as_text() {
        let svg = screen().to_svg(&DARK);
        for line in screen().text().lines().filter(|l| !l.trim().is_empty()) {
            assert!(svg.contains(line.trim_end()), "missing {line:?}");
        }
    }
}
