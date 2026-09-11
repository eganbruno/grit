//! Everything a command needs that is not its own arguments.
//!
//! Commands take `&mut Ctx` rather than reading globals, which is what lets a
//! test point grit at a throwaway registry and force colour off.

use std::io::IsTerminal as _;

use crate::error::Result;
use crate::registry::Registry;
use crate::render::ColorChoice;

pub struct Ctx {
    pub registry: Registry,
    /// Already resolved: no command should have to think about `NO_COLOR`.
    pub color: bool,
    /// Terminal width in columns, if stdout is a terminal.
    pub width: Option<usize>,
}

impl Ctx {
    pub fn new(color: ColorChoice) -> Result<Self> {
        Ok(Self {
            registry: Registry::load()?,
            color: color.enabled_for(std::io::stdout().is_terminal()),
            width: terminal_width(),
        })
    }

    /// A context over an explicit registry, with colour and width fixed.
    /// Used by tests and by anything that needs deterministic output.
    pub fn with_registry(registry: Registry, color: bool, width: Option<usize>) -> Self {
        Self {
            registry,
            color,
            width,
        }
    }
}

/// The width to lay out to.
///
/// The precedence is not the obvious one: the *most specific* name wins, and
/// `$COLUMNS` — the one everybody knows — is the one that arrives wrong.
///
/// Inside an fzf preview the pane is the whole terminal as far as the card is
/// concerned, and only fzf knows how wide it is. It does export `$COLUMNS`,
/// but the preview command runs through `$SHELL -c`, and zsh re-derives
/// `COLUMNS` from the tty as it starts — the tty being the *whole* terminal.
/// So the export is overwritten before grit is reached, and a card laid out
/// to 200 columns is drawn into a 110-column pane, where fzf wraps every
/// branch row onto a second line and the columns stop lining up. fzf's manual
/// says to prefer the prefixed name for exactly this reason.
/// `FZF_PREVIEW_COLUMNS` is set only inside a preview, so where it is set at
/// all it is the right answer.
///
/// `$COLUMNS` still beats the measurement, which is what lets a test or a
/// script pin the geometry.
fn terminal_width() -> Option<usize> {
    env_width("FZF_PREVIEW_COLUMNS")
        .or_else(|| env_width("COLUMNS"))
        .or_else(|| terminal_size::terminal_size().map(|(w, _)| w.0 as usize))
}

/// A width from the environment, if it is set and means anything.
///
/// Zero is rejected rather than believed: a table fitted to no columns at all
/// is every cell cut back to its ellipsis, which is worse than the unfitted
/// table these variables exist to avoid.
fn env_width(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|&w| w > 0)
}
