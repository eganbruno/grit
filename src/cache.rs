//! The last dashboard grit read, kept so something can paint one without
//! waiting for git.
//!
//! `grit status` takes as long as the slowest repository it has to open. That
//! is fine when you asked for it and much too slow for a table that is supposed
//! to appear on its own while you are still typing, so every unfiltered `grit
//! status` leaves its readings here and `grit status --cached` renders those
//! instead of running anything at all.
//!
//! A cache that can be subtly *wrong* is worse than no cache — a row for a repo
//! you removed ten minutes ago is a lie, not a stale truth. So this one refuses
//! to answer rather than guess. [`StatusCache::matches`] compares the cached
//! rows against the live registry and any difference at all — an added alias, a
//! moved path, a backend change, a different config file entirely — makes the
//! whole cache unusable. Refusing is cheap: the next refresh writes a good one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::registry::{Repo, VcsKind};
use crate::vcs::{Snapshot, compact_age};

/// Environment variable that overrides where the cache is written, the way
/// `GRIT_CONFIG` does for the registry. The tests rely on it; so does anyone
/// who keeps more than one registry.
pub const CACHE_ENV: &str = "GRIT_CACHE";

/// Bump when the shape below changes. An unrecognised version is treated as no
/// cache at all, which is the whole upgrade story a cache needs.
const CACHE_VERSION: u32 = 1;

pub fn cache_path() -> Result<PathBuf> {
    if let Some(raw) = std::env::var_os(CACHE_ENV) {
        if !raw.is_empty() {
            return Ok(PathBuf::from(raw));
        }
    }

    use etcetera::BaseStrategy as _;
    let strategy = etcetera::choose_base_strategy()
        .map_err(|e| Error::NoConfigHome(format!("no home directory found ({e})")))?;

    Ok(strategy.cache_dir().join("grit").join("status.json"))
}

/// One repo's last reading, or the message explaining why there wasn't one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRow {
    pub alias: String,
    pub path: PathBuf,
    pub kind: VcsKind,
    /// Carried so that `--cached --json` is the same document `--json` is,
    /// rather than a lookalike missing a field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusCache {
    pub version: u32,
    /// The registry these readings came from. Two registries — a scratch
    /// `GRIT_CONFIG` and the real one — would otherwise take turns overwriting
    /// each other's cache and each read the other's rows as its own.
    pub config: PathBuf,
    /// When the readings were taken, RFC 3339 in UTC.
    pub captured_at: jiff::Timestamp,
    pub rows: Vec<CachedRow>,
}

impl StatusCache {
    pub fn new(config: impl Into<PathBuf>, rows: Vec<CachedRow>) -> Self {
        Self {
            version: CACHE_VERSION,
            config: config.into(),
            captured_at: jiff::Timestamp::now(),
            rows,
        }
    }

    /// Read the cache, or `None` if there isn't a usable one.
    ///
    /// Every failure — missing file, unreadable file, half-written JSON, a
    /// version this build predates — is the same answer, because the caller has
    /// the same recourse in every case: run a refresh.
    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let cache: Self = serde_json::from_str(&raw).ok()?;
        (cache.version == CACHE_VERSION).then_some(cache)
    }

    /// Whether these readings still describe `repos`.
    ///
    /// Order matters and is not a limitation: the registry is a `BTreeMap`, so
    /// both sides are alphabetical by alias and a pairwise walk is exact.
    pub fn matches(&self, config: &Path, repos: &[Repo]) -> bool {
        self.config == config
            && self.rows.len() == repos.len()
            && self.rows.iter().zip(repos).all(|(row, repo)| {
                row.alias == repo.alias && row.path == repo.path() && row.kind == repo.kind()
            })
    }

    /// How long ago these readings were taken.
    ///
    /// Negative if the clock moved backwards, which the callers both handle:
    /// [`Self::age`] clamps it and a freshness check reads it as "very fresh".
    pub fn age_seconds(&self) -> i64 {
        jiff::Timestamp::now().as_second() - self.captured_at.as_second()
    }

    /// How long ago, in the same compact spelling the `age` column uses: `3s`,
    /// `2m`, `4h`.
    pub fn age(&self) -> String {
        compact_age(self.age_seconds())
    }

    /// Write the cache to `path`, creating its directory.
    ///
    /// Through a temporary file in the same directory and a rename, so a reader
    /// arriving mid-write sees the old cache rather than half of the new one.
    /// A background refresh racing an interactive `grit status` is the normal
    /// case here, not an exotic one.
    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)
            .map_err(Error::io("could not create the cache directory", dir))?;

        let json = serde_json::to_string(self).expect("a cache always serialises");

        let mut file = tempfile::NamedTempFile::new_in(dir)
            .map_err(Error::io("could not write to the cache directory", dir))?;
        {
            use std::io::Write as _;
            file.write_all(json.as_bytes())
                .map_err(Error::io("could not write the cache", path))?;
        }
        file.persist(path).map_err(|e| Error::Io {
            context: "could not replace the cache",
            path: path.to_path_buf(),
            source: e.error,
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RepoEntry;

    fn repo(alias: &str) -> Repo {
        Repo {
            alias: alias.to_string(),
            entry: RepoEntry {
                path: PathBuf::from(format!("/code/{alias}")),
                kind: VcsKind::Git,
                tags: vec![],
                added_at: None,
            },
        }
    }

    fn row(alias: &str) -> CachedRow {
        CachedRow {
            alias: alias.to_string(),
            path: PathBuf::from(format!("/code/{alias}")),
            kind: VcsKind::Git,
            tags: vec![],
            snapshot: Some(Snapshot {
                branch: Some("main".into()),
                ..Snapshot::default()
            }),
            error: None,
        }
    }

    fn cache(aliases: &[&str]) -> StatusCache {
        StatusCache::new(
            "/cfg/config.toml",
            aliases.iter().copied().map(row).collect(),
        )
    }

    #[test]
    fn a_saved_cache_loads_back_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("status.json");

        cache(&["api", "docs"]).save(&path).unwrap();
        let loaded = StatusCache::load(&path).expect("just written");

        assert_eq!(loaded.rows.len(), 2);
        assert_eq!(loaded.rows[0].alias, "api");
        assert_eq!(
            loaded.rows[0].snapshot.as_ref().unwrap().branch.as_deref(),
            Some("main")
        );
    }

    #[test]
    fn a_missing_or_corrupt_cache_is_simply_no_cache() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("status.json");
        assert!(StatusCache::load(&missing).is_none());

        std::fs::write(&missing, "{ not json").unwrap();
        assert!(StatusCache::load(&missing).is_none());

        std::fs::write(
            &missing,
            r#"{"version":99,"config":"/x","captured_at":"2026-01-01T00:00:00Z","rows":[]}"#,
        )
        .unwrap();
        assert!(StatusCache::load(&missing).is_none());
    }

    #[test]
    fn a_cache_matches_the_registry_it_was_taken_from() {
        let config = Path::new("/cfg/config.toml");
        assert!(cache(&["api", "docs"]).matches(config, &[repo("api"), repo("docs")]));
    }

    #[test]
    fn a_registered_or_removed_repo_invalidates_the_cache() {
        let config = Path::new("/cfg/config.toml");
        let cache = cache(&["api", "docs"]);

        assert!(!cache.matches(config, &[repo("api")]));
        assert!(!cache.matches(config, &[repo("api"), repo("docs"), repo("web")]));
        assert!(!cache.matches(config, &[repo("api"), repo("dokks")]));
    }

    #[test]
    fn a_moved_repo_invalidates_the_cache() {
        let config = Path::new("/cfg/config.toml");
        let mut moved = repo("api");
        moved.entry.path = PathBuf::from("/elsewhere/api");

        assert!(!cache(&["api"]).matches(config, &[moved]));
    }

    #[test]
    fn a_cache_from_another_registry_is_never_used() {
        // Two registries share one cache file unless GRIT_CACHE says otherwise,
        // so the file has to know which one it came from.
        assert!(!cache(&["api"]).matches(Path::new("/other/config.toml"), &[repo("api")]));
    }

    #[test]
    fn a_fresh_cache_reports_a_fresh_age() {
        assert_eq!(cache(&[]).age(), "0s");
    }
}
