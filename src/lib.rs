//! grit — work across many git repositories from anywhere, by alias.
//!
//! Everything of substance lives here rather than in `main.rs`, so it can be
//! unit-tested and reused.

pub mod error;
pub mod registry;
pub mod render;
pub mod vcs;

pub use error::{Error, Result};
