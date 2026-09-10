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

/// A deeper reading of one repository, for the detail view.
///
/// Separate from [`Snapshot`] because it costs several more invocations of the
/// backend and is only ever wanted one repository at a time — the one under the
/// cursor — where the dashboard wants a cheap reading of every repository at
/// once. Nothing here is cached: it is read on demand and thrown away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Detail {
    pub snapshot: Snapshot,
    /// Paths that differ from HEAD; tables, on a backend whose unit is a table.
    pub files: Vec<FileChange>,
    /// Most recent first, capped by the backend at [`LOG_LIMIT`].
    pub commits: Vec<Commit>,
    pub stashes: Vec<Stash>,
    pub branches: Vec<Branch>,
}

/// How many commits a detail reading walks back.
///
/// Enough to recognise where you were up to, few enough that the reading stays
/// one cheap call on a repository with a hundred thousand commits.
pub const LOG_LIMIT: usize = 10;

/// What happened to one path, on one side of the index.
///
/// Deliberately not git's raw `XY` pair: dolt has no index codes at all, and a
/// backend that had to invent them would be describing itself in another tool's
/// vocabulary. Each side is simply "nothing" or one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl Change {
    /// One character, in git's spelling, for the two-column code the detail
    /// view prints. Single-width so the column cannot come out ragged.
    pub const fn code(self) -> char {
        match self {
            Change::Added => 'A',
            Change::Modified => 'M',
            Change::Deleted => 'D',
            Change::Renamed => 'R',
            Change::Copied => 'C',
            Change::TypeChanged => 'T',
            Change::Untracked => '?',
            Change::Conflicted => 'U',
        }
    }
}

/// One path that differs from HEAD.
///
/// Both sides can be set at once: a file staged and then modified again is
/// `staged: Some(Modified), unstaged: Some(Modified)`, which is what git's `MM`
/// means and what the dashboard counts twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    /// Where a rename or a copy came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// What is staged for commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<Change>,
    /// What has changed since the index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unstaged: Option<Change>,
}

/// One entry on the stash stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stash {
    /// `stash@{0}`, as both backends spell it — so it can be pasted straight
    /// into `grit <alias> stash show <id>`.
    pub id: String,
    pub message: String,
    /// Compacted relative age, in the same spelling as [`Commit::age`].
    pub age: String,
}

/// How far a branch stands from its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Distance {
    pub ahead: u32,
    pub behind: u32,
}

impl Distance {
    pub fn is_level(self) -> bool {
        self.ahead == 0 && self.behind == 0
    }
}

/// One branch, and how it stands against its upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// How far from the upstream, when that could be measured.
    ///
    /// `None` is not `Some(Distance::default())`. A branch whose upstream has
    /// been deleted, or one on a backend that could not run the count, has no
    /// distance to report — and rendering that as zero would put a ✓ against
    /// a branch nobody has compared with anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<Distance>,
    /// True for the branch that is checked out.
    pub head: bool,
    /// The commit at the tip. `None` on a branch with no commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<Commit>,
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

    /// Read the repository in depth: changed paths, recent commits, stashes
    /// and branches, as well as everything [`Self::snapshot`] reads.
    ///
    /// Required rather than defaulted. A default returning an empty [`Detail`]
    /// would give a new backend a detail view that renders as a repository with
    /// nothing in it, which reads as a fact rather than as a gap.
    fn detail(&self, path: &Path) -> Result<Detail>;

    /// Run the backend's CLI in `path` with the caller's arguments.
    ///
    /// Implementations must inherit stdio so pagers, colour detection and
    /// `$EDITOR` behave exactly as they would in an interactive shell.
    fn exec(&self, path: &Path, args: &[OsString]) -> Result<ExitStatus>;
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
