//! The version-control abstraction.
//!
//! Everything grit knows how to do to a repository goes through [`Vcs`]. Adding
//! support for a new git-like system (jj, hg, ...) means writing one
//! implementation of this trait and adding a [`VcsKind`] variant — no command,
//! renderer or registry code has to change.
//!
//! Backends shell out to the real CLI rather than linking a library. That keeps
//! the dependency tree pure-Rust, guarantees we honour the user's own config
//! (pagers, credential helpers, hooks), and means the passthrough path and the
//! status path use one mechanism instead of two.

pub mod dolt;
pub mod git;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::registry::VcsKind;

/// A point-in-time reading of one repository, everything the dashboard needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Snapshot {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// The configured upstream, e.g. `origin/main`.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicts: u32,
    pub stashes: u32,
    /// `None` on a repo with no commits yet.
    pub head: Option<Commit>,
    pub state: RepoState,
}

impl Snapshot {
    /// True when nothing is staged, modified, untracked or conflicted.
    pub fn is_clean(&self) -> bool {
        self.staged == 0 && self.unstaged == 0 && self.untracked == 0 && self.conflicts == 0
    }

    /// True when the branch matches its upstream (or has none to differ from).
    pub fn is_synced(&self) -> bool {
        self.ahead == 0 && self.behind == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    /// Abbreviated hash, as git chose to abbreviate it.
    pub short_id: String,
    pub subject: String,
    /// Compacted relative age, e.g. `28h`. See [`git::compact_relative_time`]
    /// and [`dolt::compact_age`].
    pub age: String,
}

/// What the repository is in the middle of, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RepoState {
    #[default]
    Normal,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
    Bisecting,
    /// The registered path is gone. Rendered as a row rather than an error, so
    /// one stale entry cannot take down the whole dashboard.
    Missing,
}

impl RepoState {
    pub const fn label(self) -> Option<&'static str> {
        match self {
            RepoState::Normal => None,
            RepoState::Merging => Some("merging"),
            RepoState::Rebasing => Some("rebasing"),
            RepoState::CherryPicking => Some("cherry-picking"),
            RepoState::Reverting => Some("reverting"),
            RepoState::Bisecting => Some("bisecting"),
            RepoState::Missing => Some("missing"),
        }
    }
}

pub trait Vcs: Send + Sync {
    fn kind(&self) -> VcsKind;

    /// The root of the repository containing `path`, if there is one.
    ///
    /// Returning the root (rather than a bare bool) means `grit -r api .` from
    /// a subdirectory registers the repository itself.
    fn discover(&self, path: &Path) -> Option<PathBuf>;

    /// Read the repository's current state.
    fn snapshot(&self, path: &Path) -> Result<Snapshot>;

    /// Run the backend's CLI in `path` with the caller's arguments.
    ///
    /// Implementations must inherit stdio so pagers, colour detection and
    /// `$EDITOR` behave exactly as they would in an interactive shell.
    fn exec(&self, path: &Path, args: &[OsString]) -> Result<ExitStatus>;

    fn detect(&self, path: &Path) -> bool {
        self.discover(path).is_some()
    }
}

/// Seconds-to-`4h`, which is not really dolt's despite living there: the
/// backend that had to do the arithmetic itself is simply where it was written.
/// Anything holding a timestamp rather than a phrase wants it.
pub use dolt::compact_age;

/// The most of a backend's complaint worth carrying in an error message.
///
/// Long enough for a sentence of explanation, short enough that one bad row
/// cannot stretch the dashboard's columns past the terminal.
const MAX_STDERR: usize = 200;

/// Squeeze a backend's multi-line complaint onto one bounded line.
///
/// `grit status` renders only the first line of an error, so a message that
/// puts the useful part on line two shows the user nothing at all.
pub fn one_line(text: &str) -> String {
    let joined = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");

    match joined.char_indices().nth(MAX_STDERR) {
        None => joined,
        Some((cut, _)) => format!("{}…", joined[..cut].trim_end()),
    }
}

/// The backend for a given kind.
///
/// Returns `&'static dyn Vcs` because providers are stateless — they hold no
/// configuration, so one shared instance serves every repo and every thread.
pub fn provider_for(kind: VcsKind) -> &'static dyn Vcs {
    match kind {
        VcsKind::Git => &git::GitVcs,
        VcsKind::Dolt => &dolt::DoltVcs,
    }
}

/// Every backend, in the order registration should try them when the user does
/// not say which kind a path is.
///
/// Dolt comes first because it is the more specific answer: a dolt database
/// checked into a git working tree is detected as dolt, which is what someone
/// registering that directory meant. The reverse order would never see it.
pub fn all_providers() -> &'static [&'static dyn Vcs] {
    &[&dolt::DoltVcs, &git::GitVcs]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_complaint_becomes_one_line() {
        assert_eq!(one_line("boom\nhint: try again"), "boom; hint: try again");
    }

    #[test]
    fn blank_lines_are_dropped_rather_than_joined() {
        assert_eq!(one_line("boom\n\n\nhint"), "boom; hint");
    }

    #[test]
    fn a_single_line_is_left_alone() {
        assert_eq!(one_line("  no database selected  "), "no database selected");
    }

    #[test]
    fn an_enormous_complaint_is_capped() {
        // One unreadable repo must not stretch the dashboard past the terminal.
        let flattened = one_line(&"x".repeat(1_000));
        assert_eq!(flattened.chars().count(), MAX_STDERR + 1);
        assert!(flattened.ends_with('…'));
    }

    #[test]
    fn capping_does_not_split_a_multibyte_character() {
        // Slicing on a byte index inside a `—` would panic.
        let flattened = one_line(&"—".repeat(1_000));
        assert!(flattened.ends_with('…'));
    }
}
