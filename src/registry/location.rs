//! Where the registry lives on disk.
//!
//! Resolution order:
//!
//! 1. `$GRIT_CONFIG` — a full path to the file. This is what makes the
//!    integration tests hermetic, and it lets you keep a project-specific
//!    registry if you want one.
//! 2. `$XDG_CONFIG_HOME/grit/config.toml`, falling back to
//!    `~/.config/grit/config.toml`.
//!
//! Note we deliberately use the XDG-style location on macOS too. `~/Library/
//! Application Support` is right for GUI apps, but CLI users expect their tools
//! under `~/.config` where they can be dotfile-managed.

use std::path::{Path, PathBuf};

use etcetera::BaseStrategy as _;

use crate::error::{Error, Result};

/// Environment variable that overrides the config file path entirely.
pub const CONFIG_ENV: &str = "GRIT_CONFIG";

pub fn config_path() -> Result<PathBuf> {
    if let Some(raw) = std::env::var_os(CONFIG_ENV) {
        if !raw.is_empty() {
            return Ok(PathBuf::from(raw));
        }
    }

    let strategy = etcetera::choose_base_strategy()
        .map_err(|e| Error::NoConfigHome(format!("no home directory found ({e})")))?;

    Ok(strategy.config_dir().join("grit").join("config.toml"))
}

/// Render a path with `$HOME` collapsed to `~`, for display only.
pub fn abbreviate_home(path: &Path) -> String {
    let Ok(strategy) = etcetera::choose_base_strategy() else {
        return path.display().to_string();
    };
    let home = strategy.home_dir().to_path_buf();

    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviate_leaves_unrelated_paths_alone() {
        let p = Path::new("/opt/homebrew/bin/git");
        assert_eq!(abbreviate_home(p), "/opt/homebrew/bin/git");
    }

    #[test]
    fn abbreviate_collapses_the_home_prefix() {
        let Ok(strategy) = etcetera::choose_base_strategy() else {
            return; // no home dir in this environment; nothing to assert
        };
        let p = strategy.home_dir().join("Work").join("repo");
        assert_eq!(abbreviate_home(&p), "~/Work/repo");
    }
}
