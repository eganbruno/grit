//! The on-disk shape of grit's registry.
//!
//! This is the file the user can hand-edit, so the types here are chosen for
//! how they *read* in TOML rather than for internal convenience.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The config format version we write, and the newest one we can read.
///
/// Bump this only for breaking layout changes; additive fields should use
/// `#[serde(default)]` instead so older files keep loading.
pub const FORMAT_VERSION: u32 = 1;

/// Which version control system backs a repo.
///
/// Adding a new backend starts here: add a variant, then add the matching
/// [`crate::vcs::Vcs`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VcsKind {
    #[default]
    Git,
    Dolt,
}

impl VcsKind {
    /// The name of the CLI binary this backend drives.
    pub const fn program(self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Dolt => "dolt",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Dolt => "dolt",
        }
    }
}

impl std::fmt::Display for VcsKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One registered repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Absolute, canonicalised path to the working tree.
    pub path: PathBuf,

    #[serde(default)]
    pub kind: VcsKind,

    /// Free-form group labels. `grit status --tag release` and `grit @release`
    /// both resolve through these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// RFC 3339, UTC. Informational only — grit never compares these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
}

/// The whole registry, as stored in `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryFile {
    #[serde(default = "default_version")]
    pub version: u32,

    /// Keyed by alias. A `BTreeMap` so both the file and every listing come out
    /// in the same alphabetical order regardless of insertion order.
    #[serde(default)]
    pub repos: BTreeMap<String, RepoEntry>,
}

const fn default_version() -> u32 {
    FORMAT_VERSION
}

impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            repos: BTreeMap::new(),
        }
    }
}

/// A registry entry paired with the alias it is filed under.
///
/// Commands work with these rather than raw `(String, RepoEntry)` pairs so the
/// alias always travels with the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub alias: String,
    pub entry: RepoEntry,
}

impl Repo {
    pub fn path(&self) -> &std::path::Path {
        &self.entry.path
    }

    pub fn kind(&self) -> VcsKind {
        self.entry.kind
    }
}
