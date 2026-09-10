//! Load, query and persist the set of registered repositories.
//!
//! Commands never touch the config file directly — they go through [`Registry`].
//! That keeps every command testable against an in-memory registry and means a
//! change to the storage format touches exactly one module.

pub mod location;
pub mod model;

use std::io::Write as _;
use std::path::{Path, PathBuf};

pub use location::{CONFIG_ENV, abbreviate_home, config_path};
pub use model::{FORMAT_VERSION, RegistryFile, Repo, RepoEntry, VcsKind};

use crate::error::{Error, Result};

/// Names that can never be aliases, because `grit <name>` already means
/// something else.
///
/// `cli::tests::every_subcommand_is_reserved` fails if a new command is added
/// without being listed here, so this cannot silently drift.
pub const RESERVED_ALIASES: &[&str] = &[
    "add",
    "branch",
    "clone",
    "completions",
    "help",
    "register",
    "remove",
    "rm",
    "run",
    "shell",
    "show",
    "status",
    "version",
];

/// The sigil that turns a name into a tag lookup: `grit @release fetch`.
pub const GROUP_SIGIL: char = '@';

#[derive(Debug)]
pub struct Registry {
    file: RegistryFile,
    path: PathBuf,
}

impl Registry {
    /// Read the registry from `path`.
    ///
    /// A missing file is not an error — it is simply an empty registry, so a
    /// fresh install works without a setup step.
    pub fn load_from(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();

        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    file: RegistryFile::default(),
                    path,
                });
            }
            Err(source) => {
                return Err(Error::Io {
                    context: "could not read the config file",
                    path,
                    source,
                });
            }
        };

        let file: RegistryFile = toml::from_str(&raw).map_err(|source| Error::ConfigParse {
            path: path.clone(),
            source,
        })?;

        if file.version > FORMAT_VERSION {
            return Err(Error::ConfigVersion {
                path,
                found: file.version,
                supported: FORMAT_VERSION,
            });
        }

        Ok(Self { file, path })
    }

    /// Read the registry from wherever [`config_path`] points.
    pub fn load() -> Result<Self> {
        Self::load_from(config_path()?)
    }

    /// An empty registry that would be saved to `path`. Test helper.
    pub fn empty_at(path: impl Into<PathBuf>) -> Self {
        Self {
            file: RegistryFile::default(),
            path: path.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_empty(&self) -> bool {
        self.file.repos.is_empty()
    }

    /// Every repo, alphabetically by alias.
    pub fn all(&self) -> Vec<Repo> {
        self.file
            .repos
            .iter()
            .map(|(alias, entry)| Repo {
                alias: alias.clone(),
                entry: entry.clone(),
            })
            .collect()
    }

    /// Every distinct tag in use, alphabetically.
    pub fn tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self
            .file
            .repos
            .values()
            .flat_map(|e| e.tags.iter().cloned())
            .collect();
        tags.sort();
        tags.dedup();
        tags
    }

    /// Look up a single repo by alias.
    pub fn get(&self, alias: &str) -> Result<Repo> {
        self.file
            .repos
            .get(alias)
            .map(|entry| Repo {
                alias: alias.to_string(),
                entry: entry.clone(),
            })
            .ok_or_else(|| Error::UnknownAlias(alias.to_string()))
    }

    /// Every repo carrying `tag`. Errors if the tag matches nothing, so a typo
    /// in `grit @relase fetch` fails loudly instead of quietly doing nothing.
    pub fn by_tag(&self, tag: &str) -> Result<Vec<Repo>> {
        let matched: Vec<Repo> = self
            .all()
            .into_iter()
            .filter(|r| r.entry.tags.iter().any(|t| t == tag))
            .collect();

        if matched.is_empty() {
            return Err(Error::UnknownTag {
                tag: tag.to_string(),
                known: self.tags(),
            });
        }
        Ok(matched)
    }

    /// Resolve a target that may be either an alias or an `@tag` group.
    ///
    /// This is what `grit <target> <args...>` dispatches on.
    pub fn resolve_target(&self, target: &str) -> Result<Vec<Repo>> {
        match target.strip_prefix(GROUP_SIGIL) {
            Some(tag) => self.by_tag(tag),
            None => Ok(vec![self.get(target)?]),
        }
    }

    /// Narrow the registry down for listing commands.
    ///
    /// With no aliases and no tag, everything is returned. Both filters may be
    /// combined, in which case a repo has to satisfy both.
    pub fn select(&self, aliases: &[String], tag: Option<&str>) -> Result<Vec<Repo>> {
        let mut repos = if aliases.is_empty() {
            self.all()
        } else {
            aliases
                .iter()
                .map(|a| self.get(a))
                .collect::<Result<Vec<_>>>()?
        };

        if let Some(tag) = tag {
            // Check the tag exists at all before filtering, so `--tag typo`
            // reports the typo rather than an empty dashboard.
            self.by_tag(tag)?;
            repos.retain(|r| r.entry.tags.iter().any(|t| t == tag));
        }

        Ok(repos)
    }

    /// Add or replace an entry. Returns the previous entry if one was replaced.
    ///
    /// Validation of the alias and of the path having a repo in it happens in
    /// [`validate_alias`] and the caller respectively; this is the raw insert.
    pub fn insert(&mut self, alias: String, entry: RepoEntry) -> Option<RepoEntry> {
        self.file.repos.insert(alias, entry)
    }

    pub fn remove(&mut self, alias: &str) -> Result<RepoEntry> {
        self.file
            .repos
            .remove(alias)
            .ok_or_else(|| Error::UnknownAlias(alias.to_string()))
    }

    /// Error if `alias` is taken, unless the caller passed `--force`.
    pub fn check_available(&self, alias: &str, force: bool) -> Result<()> {
        if force {
            return Ok(());
        }
        if let Some(existing) = self.file.repos.get(alias) {
            return Err(Error::DuplicateAlias {
                alias: alias.to_string(),
                existing: existing.path.clone(),
            });
        }
        Ok(())
    }

    /// Write the registry back to disk.
    ///
    /// Writes to a temporary file in the same directory and renames it into
    /// place, so an interrupted save can never leave a half-written registry.
    pub fn save(&self) -> Result<()> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)
            .map_err(Error::io("could not create the config directory", dir))?;

        let body = render_toml(&self.file);

        let mut tmp = tempfile::NamedTempFile::new_in(dir)
            .map_err(Error::io("could not create a temporary config file", dir))?;
        tmp.write_all(body.as_bytes())
            .map_err(Error::io("could not write the config file", &self.path))?;
        tmp.as_file()
            .sync_all()
            .map_err(Error::io("could not flush the config file", &self.path))?;
        tmp.persist(&self.path).map_err(|e| Error::Io {
            context: "could not replace the config file",
            path: self.path.clone(),
            source: e.error,
        })?;

        Ok(())
    }
}

/// Serialise with a short header explaining that the file is fair game to edit.
///
/// `to_string` rather than `to_string_pretty`: the pretty writer explodes
/// `tags` across four lines, and this file is meant to be read and edited by a
/// person, not diffed by a machine.
fn render_toml(file: &RegistryFile) -> String {
    let body = toml::to_string(file).expect("registry is always serialisable");
    format!(
        "# grit registry — safe to edit by hand.\n\
         # `grit show` lists what is in here; `grit -r <alias> <path>` adds to it.\n\
         \n{body}"
    )
}

/// Reject aliases that would be ambiguous, unusable, or would shadow a command.
pub fn validate_alias(alias: &str) -> Result<()> {
    let invalid = |reason: &str| Error::InvalidAlias {
        alias: alias.to_string(),
        reason: reason.to_string(),
    };

    if alias.is_empty() {
        return Err(invalid("it is empty"));
    }
    if alias.starts_with(GROUP_SIGIL) {
        return Err(invalid("`@` is reserved for tag groups, as in `@release`"));
    }
    if alias.starts_with('-') {
        return Err(invalid("it would be parsed as a flag"));
    }
    if let Some(bad) = alias
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        return Err(invalid(&format!("`{bad}` is not allowed")));
    }
    if RESERVED_ALIASES.contains(&alias) {
        return Err(Error::ReservedAlias(alias.to_string()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, tags: &[&str]) -> RepoEntry {
        RepoEntry {
            path: PathBuf::from(path),
            kind: VcsKind::Git,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            added_at: None,
        }
    }

    fn fixture() -> Registry {
        let mut r = Registry::empty_at("/tmp/does-not-matter.toml");
        r.insert("docs".into(), entry("/code/docs", &["release"]));
        r.insert("api".into(), entry("/code/api", &["release", "core"]));
        r.insert("solo".into(), entry("/code/solo", &[]));
        r
    }

    #[test]
    fn all_is_alphabetical_regardless_of_insertion_order() {
        let aliases: Vec<_> = fixture().all().into_iter().map(|r| r.alias).collect();
        assert_eq!(aliases, ["api", "docs", "solo"]);
    }

    #[test]
    fn get_reports_the_alias_it_could_not_find() {
        let err = fixture().get("nope").unwrap_err();
        assert!(matches!(err, Error::UnknownAlias(a) if a == "nope"));
    }

    #[test]
    fn by_tag_matches_every_repo_carrying_it() {
        let aliases: Vec<_> = fixture()
            .by_tag("release")
            .unwrap()
            .into_iter()
            .map(|r| r.alias)
            .collect();
        assert_eq!(aliases, ["api", "docs"]);
    }

    #[test]
    fn an_unknown_tag_is_an_error_not_an_empty_list() {
        let err = fixture().by_tag("nope").unwrap_err();
        assert!(matches!(err, Error::UnknownTag { .. }));
        // The tags that do exist are named in the error, so the reader does not
        // have to go and run `grit show` to find the one they meant.
        let msg = err.to_string();
        assert!(msg.starts_with("no repos tagged `nope`"), "{msg}");
        assert!(msg.contains("tags in use: core, release"), "{msg}");
    }

    #[test]
    fn an_unknown_tag_with_no_tags_at_all_says_how_to_make_one() {
        let mut registry = Registry::empty_at("/tmp/grit-test.toml");
        registry.insert("docs".to_string(), entry("/code/docs", &[]));
        let msg = registry.by_tag("nope").unwrap_err().to_string();
        assert!(msg.contains("no tags are in use"), "{msg}");
    }

    #[test]
    fn resolve_target_distinguishes_aliases_from_groups() {
        let reg = fixture();
        assert_eq!(reg.resolve_target("docs").unwrap().len(), 1);
        assert_eq!(reg.resolve_target("@release").unwrap().len(), 2);
    }

    #[test]
    fn select_with_no_filters_returns_everything() {
        assert_eq!(fixture().select(&[], None).unwrap().len(), 3);
    }

    #[test]
    fn select_combines_alias_and_tag_filters() {
        let reg = fixture();
        let picked = reg
            .select(&["api".into(), "solo".into()], Some("release"))
            .unwrap();
        let aliases: Vec<_> = picked.into_iter().map(|r| r.alias).collect();
        assert_eq!(aliases, ["api"]);
    }

    #[test]
    fn tags_are_deduplicated_and_sorted() {
        assert_eq!(fixture().tags(), ["core", "release"]);
    }

    #[test]
    fn check_available_refuses_a_taken_alias_unless_forced() {
        let reg = fixture();
        assert!(reg.check_available("docs", false).is_err());
        assert!(reg.check_available("docs", true).is_ok());
        assert!(reg.check_available("fresh", false).is_ok());
    }

    #[test]
    fn valid_aliases_are_accepted() {
        for alias in ["docs", "api", "a_b", "v1.2", "x9"] {
            assert!(validate_alias(alias).is_ok(), "{alias} should be valid");
        }
    }

    #[test]
    fn invalid_aliases_are_rejected_with_a_reason() {
        assert!(matches!(
            validate_alias("status").unwrap_err(),
            Error::ReservedAlias(_)
        ));
        for alias in ["", "@release", "-r", "has space", "sla/sh"] {
            assert!(
                validate_alias(alias).is_err(),
                "{alias:?} should be rejected"
            );
        }
    }

    #[test]
    fn round_trips_through_toml() {
        let original = fixture();
        let text = render_toml(&original.file);
        let parsed: RegistryFile = toml::from_str(&text).unwrap();
        assert_eq!(parsed.version, FORMAT_VERSION);
        assert_eq!(parsed.repos, original.file.repos);
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::load_from(dir.path().join("nothing-here.toml")).unwrap();
        assert!(reg.is_empty());
    }

    #[test]
    fn save_then_load_preserves_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");

        let mut reg = Registry::empty_at(&path);
        reg.insert("docs".into(), entry("/code/docs", &["release"]));
        reg.save().unwrap();

        let reloaded = Registry::load_from(&path).unwrap();
        assert_eq!(reloaded.get("docs").unwrap().entry.tags, ["release"]);
    }

    #[test]
    fn a_config_from_the_future_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "version = 999\n[repos]\n").unwrap();

        assert!(matches!(
            Registry::load_from(&path).unwrap_err(),
            Error::ConfigVersion { found: 999, .. }
        ));
    }

    #[test]
    fn malformed_toml_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();

        assert!(matches!(
            Registry::load_from(&path).unwrap_err(),
            Error::ConfigParse { .. }
        ));
    }
}
