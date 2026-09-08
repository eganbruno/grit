//! The dolt backend, end to end.
//!
//! These drive the real `dolt` binary, so they cover what the unit tests in
//! `src/vcs/dolt.rs` cannot: that the queries there are ones dolt accepts, and
//! that registration picks the right backend for a path. Each one skips itself
//! when dolt is not installed — see [`common::dolt_available`].

mod common;

use std::path::Path;
use std::process::{Child, Stdio};

use assert_cmd::prelude::*;
use common::{TestEnv, describe, dolt_available, row};
use predicates::prelude::*;

/// Every test opens with this. Written out rather than hidden in a macro so a
/// skipped test is obvious when reading the file.
macro_rules! needs_dolt {
    () => {
        if !dolt_available() {
            eprintln!("skipping: dolt is not on PATH");
            return;
        }
    };
}

#[test]
fn a_dolt_database_is_registered_as_dolt() {
    needs_dolt!();
    let env = TestEnv::new();
    env.register("data", &env.dolt_repo("data"), &[]);

    let out = env.grit().args(["show", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["repos"][0]["kind"], "dolt");
}

#[test]
fn registering_a_subdirectory_registers_the_database_root() {
    needs_dolt!();
    let env = TestEnv::new();
    let repo = env.dolt_repo("data");
    let nested = repo.join("sub/deeper");
    std::fs::create_dir_all(&nested).unwrap();

    env.register("data", &nested, &[]);

    assert!(
        env.config_contents().contains(repo.to_str().unwrap()),
        "registered the subdirectory rather than the root:\n{}",
        env.config_contents()
    );
}

/// Dolt keeps its *global* config in `~/.dolt`, so a directory holding only
/// that must not be mistaken for a database.
#[test]
fn a_directory_holding_only_dolts_global_config_is_not_a_repo() {
    needs_dolt!();
    let env = TestEnv::new();
    let fake_home = env.root().join("home");
    std::fs::create_dir_all(fake_home.join(".dolt")).unwrap();
    std::fs::write(fake_home.join(".dolt/config_global.json"), "{}").unwrap();

    env.grit()
        .args(["-r", "home"])
        .arg(&fake_home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a dolt or git repository"));
}

/// The reason [`grit::vcs::all_providers`] puts dolt first: a database that
/// happens to sit inside a git working tree is a dolt repo to whoever registers
/// it, and git's `rev-parse` would otherwise claim it.
#[test]
fn a_database_inside_a_git_worktree_is_detected_as_dolt() {
    needs_dolt!();
    let env = TestEnv::new();
    let outer = env.repo("outer");
    let inner = outer.join("data");
    std::fs::create_dir_all(&inner).unwrap();
    env.dolt(
        &inner,
        &[
            "init",
            "--name",
            "grit tests",
            "--email",
            "tests@grit.invalid",
        ],
    );

    env.register("data", &inner, &[]);

    let out = env.grit().args(["show", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["repos"][0]["kind"], "dolt");
    assert_eq!(json["repos"][0]["path"], inner.to_str().unwrap());
}

#[test]
fn a_clean_database_reports_its_branch_and_head() {
    needs_dolt!();
    let env = TestEnv::new();
    env.register("data", &env.dolt_repo("data"), &[]);

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let cells = row(&stdout, "data");

    assert_eq!(cells[1], "main");
    assert!(
        stdout.contains("initial commit"),
        "no commit subject in:\n{stdout}"
    );
}

#[test]
fn staged_unstaged_and_untracked_tables_are_counted_separately() {
    needs_dolt!();
    let env = TestEnv::new();
    let repo = env.dolt_repo("data");

    // Staged: a new table added but not committed.
    env.dolt_sql(&repo, "create table staged (id int primary key)");
    env.dolt(&repo, &["add", "staged"]);
    // Unstaged: a committed table with new rows.
    env.dolt_sql(&repo, "insert into items values (1, 'a')");
    // Untracked: a new table never added.
    env.dolt_sql(&repo, "create table untracked (id int primary key)");

    env.register("data", &repo, &[]);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["staged"], 1, "{json}");
    assert_eq!(json[0]["unstaged"], 1, "{json}");
    assert_eq!(json[0]["untracked"], 1, "{json}");
}

#[test]
fn a_branch_ahead_of_its_upstream_shows_the_count() {
    needs_dolt!();
    let env = TestEnv::new();
    let origin = env.dolt_repo("origin");
    let clone = env.dolt_clone_of(&origin, "clone");

    env.dolt_commit(&clone, "insert into items values (2, 'b')", "ahead one");
    env.register("clone", &clone, &[]);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["upstream"], "origin/main", "{json}");
    assert_eq!(json[0]["ahead"], 1, "{json}");
    assert_eq!(json[0]["behind"], 0, "{json}");
}

#[test]
fn a_branch_behind_its_upstream_shows_the_count() {
    needs_dolt!();
    let env = TestEnv::new();
    let origin = env.dolt_repo("origin");
    let clone = env.dolt_clone_of(&origin, "clone");

    // Advance the remote past the clone, then let the clone see it.
    env.dolt_commit(&origin, "insert into items values (3, 'c')", "moved on");
    env.dolt(&origin, &["push", "origin", "main"]);
    env.dolt(&clone, &["fetch"]);

    env.register("clone", &clone, &[]);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["behind"], 1, "{json}");
    assert_eq!(json[0]["ahead"], 0, "{json}");
}

#[test]
fn a_database_with_no_upstream_is_marked_as_such_not_as_in_sync() {
    needs_dolt!();
    let env = TestEnv::new();
    env.register("data", &env.dolt_repo("data"), &[]);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json[0]["upstream"].is_null(), "{json}");
}

#[test]
fn a_conflicted_merge_reports_both_the_state_and_the_conflicts() {
    needs_dolt!();
    let env = TestEnv::new();
    let repo = env.dolt_repo("data");

    env.dolt_commit(&repo, "insert into items values (1, 'base')", "base row");
    env.dolt(&repo, &["checkout", "-b", "topic"]);
    env.dolt_commit(
        &repo,
        "update items set name = 'topic' where id = 1",
        "topic edit",
    );
    env.dolt(&repo, &["checkout", "main"]);
    env.dolt_commit(
        &repo,
        "update items set name = 'main' where id = 1",
        "main edit",
    );

    let merge = env.dolt_output(&repo, &["merge", "topic"]);
    assert!(
        !merge.status.success(),
        "the merge was meant to conflict: {}",
        describe(&merge)
    );

    env.register("data", &repo, &[]);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["state"], "merging", "{json}");
    assert_eq!(json[0]["conflicts"], 1, "{json}");
}

#[test]
fn a_stash_shows_up_even_when_the_tree_is_otherwise_clean() {
    needs_dolt!();
    let env = TestEnv::new();
    let repo = env.dolt_repo("data");

    env.dolt_sql(&repo, "insert into items values (4, 'stashed')");
    env.dolt(&repo, &["stash"]);
    env.register("data", &repo, &[]);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["stashes"], 1, "{json}");
    assert_eq!(json[0]["unstaged"], 0, "{json}");
}

#[test]
fn passthrough_runs_dolt_in_the_registered_database() {
    needs_dolt!();
    let env = TestEnv::new();
    env.register("data", &env.dolt_repo("data"), &[]);

    env.grit()
        .args(["data", "log"])
        .assert()
        .success()
        .stdout(predicate::str::contains("initial commit"));
}

#[test]
fn passthrough_hands_back_dolts_exit_status() {
    needs_dolt!();
    let env = TestEnv::new();
    env.register("data", &env.dolt_repo("data"), &[]);

    env.grit()
        .args(["data", "no-such-subcommand"])
        .assert()
        .failure();
}

/// A dolt and a git repo in one dashboard: the whole point of the abstraction.
#[test]
fn dolt_and_git_repos_render_side_by_side() {
    needs_dolt!();
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.register("data", &env.dolt_repo("data"), &[]);

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "api")[1], "main");
    assert_eq!(row(&stdout, "data")[1], "main");
    assert!(stdout.contains("2 repos"), "{stdout}");
}

/// A `dolt sql-server` running against one database for as long as this lives.
///
/// Held in a guard so the server dies even when an assertion panics: a leaked
/// one keeps its lock on the database and wedges every later run.
struct SqlServer {
    child: Child,
}

impl SqlServer {
    /// Start a server and wait until queries actually route through it.
    ///
    /// Readiness is not "the process started" but "values come back as strings"
    /// — that is the observable difference the server makes, and the thing
    /// under test. Before it is up, `dolt sql` opens the database directly and
    /// answers with native JSON types.
    fn start(env: &TestEnv, repo: &Path, port: u16) -> Self {
        let child = env
            .dolt_command(repo, &["sql-server", "--port", &port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start dolt sql-server");

        let server = Self { child };

        for _ in 0..80 {
            let out = env.dolt_output(repo, &["sql", "-r", "json", "-q", "select 1 as n"]);
            if String::from_utf8_lossy(&out.stdout).contains(r#""n":"1""#) {
                return server;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }

        panic!("dolt sql-server did not start serving within 20s");
    }
}

impl Drop for SqlServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// With a server holding the database, `dolt sql` becomes a MySQL client and
/// every value arrives as a string — `"0"` rather than `0`. Reading that as a
/// number is the whole job, and getting it wrong made the dashboard show the
/// repo as unreadable rather than clean.
#[test]
fn a_database_held_by_a_sql_server_still_reads() {
    needs_dolt!();
    let env = TestEnv::new();
    let repo = env.dolt_repo("data");
    env.dolt_sql(&repo, "insert into items values (1, 'a')");
    env.register("data", &repo, &[]);

    let _server = SqlServer::start(&env, &repo, 15799);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    assert!(json[0]["error"].is_null(), "read failed: {json}");
    assert_eq!(json[0]["branch"], "main", "{json}");
    assert_eq!(json[0]["unstaged"], 1, "{json}");
    assert_eq!(json[0]["staged"], 0, "{json}");
}

/// A dolt commit message is the whole message, body and all, where git hands
/// over only the subject. Left as it came, the body landed in the dashboard:
/// its lines pushed the following repo into the wrong columns, and in the shell
/// preview they left rows stranded above the prompt.
#[test]
fn a_commit_body_does_not_get_into_the_table() {
    needs_dolt!();
    let env = TestEnv::new();
    let repo = env.dolt_repo("data");

    let subject = "Move the values to premium slots";
    env.dolt_sql(&repo, "insert into items values (1, 'a')");
    env.dolt(&repo, &["add", "."]);
    env.dolt(
        &repo,
        &[
            "commit",
            "-m",
            &format!("{subject}\n\nA body that would otherwise\nspill across the table.\n"),
        ],
    );
    env.register("data", &repo, &[]);

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let head = json[0]["head"]["subject"].as_str().unwrap_or_default();
    assert_eq!(head, subject, "{json}");

    // And the rendered table is still one line per repo.
    let table = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&table.stdout);
    assert!(
        !stdout.contains("spill across the table"),
        "the body reached the dashboard:\n{stdout}"
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|l| l.contains("A body that would otherwise"))
            .count(),
        0,
        "{stdout}"
    );
    // header, rule, one row, blank, footer
    let rows = stdout
        .lines()
        .filter(|l| l.trim_start().starts_with("data"))
        .count();
    assert_eq!(rows, 1, "expected exactly one row for `data`:\n{stdout}");
}
