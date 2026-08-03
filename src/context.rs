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

fn terminal_width() -> Option<usize> {
    // `$COLUMNS` wins when set, so output can be pinned in tests and scripts.
    if let Some(cols) = std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()) {
        return Some(cols);
    }
    terminal_size::terminal_size().map(|(w, _)| w.0 as usize)
}
