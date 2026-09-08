//! grit — work across many git and dolt repositories from anywhere, by alias.
//!
//! The binary in `main.rs` is a thin wrapper; everything of substance lives
//! here so it can be unit-tested, and so a future front end (a TUI, say) can
//! reuse it without shelling out to grit itself.
//!
//! ## Layers
//!
//! | module | responsibility |
//! |---|---|
//! | [`cache`] | the last dashboard read, so one can be shown instantly |
//! | [`cli`] | the command-line grammar, and nothing else |
//! | [`commands`] | one file per command; the only layer that prints |
//! | [`context`] | `Ctx` — the registry, colour choice and terminal width |
//! | [`error`] | [`Error`], the typed error every layer returns |
//! | [`registry`] | the set of registered repos, and its file on disk |
//! | [`vcs`] | the [`vcs::Vcs`] trait and its backends |
//! | [`shell`] | the prompt integration, and installing it |
//! | [`render`] | tables, colours and symbols |
//!
//! Dependencies point downwards only: `commands` uses `registry`, `vcs` and
//! `render`; none of those three know about each other or about `commands`.

pub mod cache;
pub mod cli;
pub mod commands;
pub mod context;
pub mod error;
pub mod registry;
pub mod render;
pub mod shell;
pub mod vcs;

pub use error::{Error, Result};
