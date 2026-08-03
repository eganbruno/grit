//! Shared scaffolding for the integration tests.
//!
//! Every test gets its own temporary directory holding both its registry and
//! its repositories, so tests never see each other's state and never touch the
//! developer's real `~/.config/grit`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;

pub struct TestEnv {
    /// Kept alive for the lifetime of the test; dropping it deletes everything.
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
}

impl TestEnv {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        // Resolve symlinks up front (/var -> /private/var on macOS) so paths
        // grit canonicalises compare equal to the ones we hand it.
        let root = dir.path().canonicalize().expect("canonicalize temp dir");
        let config = root.join("config.toml");
        Self {
            _dir: dir,
            root,
            config,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &Path {
        &self.config
    }

    pub fn config_contents(&self) -> String {
        std::fs::read_to_string(&self.config).unwrap_or_default()
    }

    pub fn config_exists(&self) -> bool {
        self.config.exists()
    }

    /// A `grit` invocation pointed at this environment's registry.
    ///
    /// `COLUMNS` is pinned so column widths do not depend on the terminal the
    /// suite happens to run in, and the git config files are neutralised so a
    /// developer's own `~/.gitconfig` cannot change what git prints.
    pub fn grit(&self) -> Command {
        let mut cmd = Command::cargo_bin("grit").expect("built binary");
        cmd.env("GRIT_CONFIG", &self.config)
            .env("COLUMNS", "200")
            .env("NO_COLOR", "1")
            .current_dir(&self.root);
        neutralise_git_config(&mut cmd);
        cmd
    }

    /// Create a git repository with one commit, and return its path.
    pub fn repo(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).expect("create repo dir");

        git(&path, &["init", "--quiet", "--initial-branch=main"]);
        self.commit(&path, "README.md", &format!("# {name}\n"), "initial commit");
        path
    }

    /// Write a file, stage it, and commit.
    pub fn commit(&self, repo: &Path, file: &str, contents: &str, message: &str) {
        std::fs::write(repo.join(file), contents).expect("write file");
        git(repo, &["add", file]);
        git(repo, &["commit", "--quiet", "-m", message]);
    }

    /// Write a file without staging it.
    pub fn write(&self, repo: &Path, file: &str, contents: &str) {
        std::fs::write(repo.join(file), contents).expect("write file");
    }

    /// Clone `origin` so the clone has a real upstream to be ahead of.
    pub fn clone_of(&self, origin: &Path, name: &str) -> PathBuf {
        let path = self.root.join(name);
        git(
            &self.root,
            &[
                "clone",
                "--quiet",
                origin.to_str().unwrap(),
                path.to_str().unwrap(),
            ],
        );
        path
    }

    /// Register `path` under `alias`, asserting the command succeeded.
    pub fn register(&self, alias: &str, path: &Path, tags: &[&str]) {
        let mut cmd = self.grit();
        cmd.args(["-r", alias]).arg(path);
        for tag in tags {
            cmd.args(["--tag", tag]);
        }
        cmd.assert().success();
    }
}

impl Default for TestEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Run git in `dir`, panicking with its stderr if it fails.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    neutralise_git_config(&mut cmd);

    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}:\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run git in `dir` and hand back its output whether or not it succeeded.
///
/// For commands that are *expected* to fail — a merge staged to conflict, say.
/// Use this rather than a hand-rolled `Command`: it applies the same full
/// environment as [`git`]. Supplying only part of that environment makes git
/// fail for the wrong reason on some platforms and not others — notably, git on
/// Linux derives a fallback identity from `/etc/passwd` and the hostname, and
/// aborts before doing any work when that lookup fails, where macOS succeeds.
pub fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    neutralise_git_config(&mut cmd);
    cmd.output().expect("run git")
}

/// Render a command's outcome for a panic message.
pub fn describe(out: &std::process::Output) -> String {
    format!(
        "{}\n  stdout: {}\n  stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout).trim(),
        String::from_utf8_lossy(&out.stderr).trim(),
    )
}

/// Make git ignore the machine's configuration and use a fixed identity.
///
/// Without this a developer's `core.pager = delta` or a missing `user.email`
/// would make the suite behave differently on different machines.
fn neutralise_git_config(cmd: &mut Command) {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "grit tests")
        .env("GIT_AUTHOR_EMAIL", "tests@grit.invalid")
        .env("GIT_COMMITTER_NAME", "grit tests")
        .env("GIT_COMMITTER_EMAIL", "tests@grit.invalid")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0");
}

/// The cell values of a rendered table row, by leading alias.
///
/// Splits on runs of two or more spaces, which is exactly the gap the renderer
/// uses between columns, so a single space inside a commit subject stays put.
pub fn row(output: &str, alias: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim_end)
        .find(|line| line.split_whitespace().next() == Some(alias))
        .unwrap_or_else(|| panic!("no row for `{alias}` in:\n{output}"))
        .trim()
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
