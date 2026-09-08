//! Shared scaffolding for the integration tests.
//!
//! Every test gets its own temporary directory holding both its registry and
//! its repositories, so tests never see each other's state and never touch the
//! developer's real `~/.config/grit`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;

/// Where `TestEnv` keeps dolt's global configuration, relative to its root.
///
/// Deliberately not named `.dolt`: that would make the directory itself look
/// like a database to the backend's own detection.
const DOLT_HOME: &str = "dolt-home";

/// Give dolt a fixed identity, the way [`neutralise_git_config`] does for git.
///
/// This has to be dolt's *global* config rather than each repo's: a clone
/// starts with an empty local config, so an identity written only into the
/// original leaves `dolt commit` in the clone with no one to attribute to.
///
/// Written as a file rather than through `dolt config` so that constructing a
/// `TestEnv` needs no dolt binary — the git-only tests build one too.
fn write_dolt_identity(root: &Path) {
    let dir = root.join(DOLT_HOME).join(".dolt");
    std::fs::create_dir_all(&dir).expect("create dolt home");
    std::fs::write(
        dir.join("config_global.json"),
        r#"{"user.name":"grit tests","user.email":"tests@grit.invalid"}"#,
    )
    .expect("write dolt identity");
}

/// Whether the `dolt` binary is on `PATH`.
///
/// Dolt is not installed on every contributor's machine, so the tests that need
/// a real database skip themselves rather than failing. CI installs it, which is
/// where they are guaranteed to run.
pub fn dolt_available() -> bool {
    Command::new("dolt")
        .arg("version")
        .output()
        .is_ok_and(|out| out.status.success())
}

pub struct TestEnv {
    /// Kept alive for the lifetime of the test; dropping it deletes everything.
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    cache: PathBuf,
}

impl TestEnv {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        // Resolve symlinks up front (/var -> /private/var on macOS) so paths
        // grit canonicalises compare equal to the ones we hand it.
        let root = dir.path().canonicalize().expect("canonicalize temp dir");
        let config = root.join("config.toml");
        let cache = root.join("status-cache.json");
        write_dolt_identity(&root);
        Self {
            _dir: dir,
            root,
            config,
            cache,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_contents(&self) -> String {
        std::fs::read_to_string(&self.config).unwrap_or_default()
    }

    pub fn config_exists(&self) -> bool {
        self.config.exists()
    }

    /// Where this environment's status cache lives. Inside the temp directory,
    /// so a test run never touches the developer's real one.
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    pub fn cache_exists(&self) -> bool {
        self.cache.exists()
    }

    /// A `grit` invocation pointed at this environment's registry.
    ///
    /// `COLUMNS` is pinned so column widths do not depend on the terminal the
    /// suite happens to run in, `GRIT_CACHE` keeps the status cache inside the
    /// temp directory, and the git config files are neutralised so a
    /// developer's own `~/.gitconfig` cannot change what git prints.
    pub fn grit(&self) -> Command {
        let mut cmd = Command::cargo_bin("grit").expect("built binary");
        cmd.env("GRIT_CONFIG", &self.config)
            .env("GRIT_CACHE", &self.cache)
            .env("COLUMNS", "200")
            .env("NO_COLOR", "1")
            .current_dir(&self.root);
        neutralise_git_config(&mut cmd);
        self.neutralise_dolt_config(&mut cmd);
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

    /// Create a dolt database with one table and one commit, and return its
    /// path.
    pub fn dolt_repo(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).expect("create repo dir");

        self.dolt(&path, &["init"]);
        self.dolt_commit(
            &path,
            "create table items (id int primary key, name varchar(64))",
            "initial commit",
        );
        path
    }

    /// Run some SQL, stage every table, and commit.
    pub fn dolt_commit(&self, repo: &Path, sql: &str, message: &str) {
        self.dolt(repo, &["sql", "-q", sql]);
        self.dolt(repo, &["add", "."]);
        self.dolt(repo, &["commit", "-m", message]);
    }

    /// Run some SQL and leave the result unstaged.
    pub fn dolt_sql(&self, repo: &Path, sql: &str) {
        self.dolt(repo, &["sql", "-q", sql]);
    }

    /// Publish `origin` to a file-backed remote and clone it back, so both ends
    /// have a real upstream to be ahead of or behind.
    pub fn dolt_clone_of(&self, origin: &Path, name: &str) -> PathBuf {
        let remote = self.root.join(format!("{name}.remote"));
        let url = format!("file://{}", remote.display());

        self.dolt(origin, &["remote", "add", "origin", &url]);
        self.dolt(origin, &["push", "-u", "origin", "main"]);

        let path = self.root.join(name);
        self.dolt(&self.root, &["clone", &url, path.to_str().unwrap()]);
        path
    }

    /// Run dolt in `dir`, panicking with its stderr if it fails.
    pub fn dolt(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.dolt_output(dir, args);
        assert!(
            out.status.success(),
            "dolt {args:?} failed in {}:\n{}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Run dolt in `dir` and hand back its output whether or not it succeeded —
    /// for commands expected to fail, such as a merge staged to conflict.
    pub fn dolt_output(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        self.dolt_command(dir, args).output().expect("run dolt")
    }

    /// A dolt invocation configured for this environment, not yet run.
    ///
    /// For the cases that need more than the output — spawning a long-running
    /// `dolt sql-server`, say.
    pub fn dolt_command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("dolt");
        cmd.current_dir(dir).args(args);
        self.neutralise_dolt_config(&mut cmd);
        cmd
    }

    /// Point dolt at this environment's own global config rather than the
    /// developer's `~/.dolt`, which holds their identity and credentials.
    fn neutralise_dolt_config(&self, cmd: &mut Command) {
        cmd.env("DOLT_ROOT_PATH", self.root.join(DOLT_HOME));
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
