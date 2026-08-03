//! `grit status` — the dashboard, against real repositories.

mod common;

use assert_cmd::prelude::*;
use common::{TestEnv, describe, git, git_output, row};
use predicates::prelude::*;

#[test]
fn an_empty_registry_says_what_to_type_next() {
    TestEnv::new()
        .grit()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("No repositories registered"));
}

#[test]
fn a_clean_repo_reports_its_branch_and_head() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let cells = row(&stdout, "api");

    assert_eq!(cells[0], "api");
    assert_eq!(cells[1], "main");
    assert_eq!(cells[3], "clean");
    assert!(stdout.contains("initial commit"), "{stdout}");
    assert!(stdout.contains("all clean"), "{stdout}");
}

#[test]
fn staged_unstaged_and_untracked_changes_are_counted_separately() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &[]);

    // One staged addition, one unstaged modification, one untracked file.
    env.write(&repo, "staged.txt", "new\n");
    git(&repo, &["add", "staged.txt"]);
    env.write(&repo, "README.md", "# changed\n");
    env.write(&repo, "untracked.txt", "loose\n");

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "api")[3], "●1 ○1 ?1", "{stdout}");
    assert!(stdout.contains("1 dirty"), "{stdout}");
}

#[test]
fn a_stash_shows_up_even_when_the_tree_is_otherwise_clean() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &[]);

    env.write(&repo, "README.md", "# work in progress\n");
    git(&repo, &["stash", "push", "--quiet", "-m", "wip"]);

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "api")[3], "⚑1", "{stdout}");
    assert!(stdout.contains("1 stash"), "{stdout}");
}

#[test]
fn a_branch_ahead_of_its_upstream_shows_the_count() {
    let env = TestEnv::new();
    let origin = env.repo("origin");
    let clone = env.clone_of(&origin, "clone");
    env.register("clone", &clone, &[]);

    env.commit(&clone, "local.txt", "local\n", "local work");

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "clone")[2], "↑1", "{stdout}");
    assert!(stdout.contains("1 ahead"), "{stdout}");
}

#[test]
fn a_branch_behind_its_upstream_shows_the_count() {
    let env = TestEnv::new();
    let origin = env.repo("origin");
    let clone = env.clone_of(&origin, "clone");
    env.register("clone", &clone, &[]);

    env.commit(&origin, "remote.txt", "remote\n", "upstream work");
    git(&clone, &["fetch", "--quiet"]);

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "clone")[2], "↓1", "{stdout}");
    assert!(stdout.contains("1 behind"), "{stdout}");
}

#[test]
fn a_branch_with_no_upstream_is_marked_as_such_not_as_in_sync() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "api")[2], "·", "{stdout}");
}

#[test]
fn a_detached_head_is_labelled() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &[]);

    let head = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "--quiet", head.trim()]);

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "api")[1], "detached", "{stdout}");
}

#[test]
fn a_conflicted_merge_reports_both_the_state_and_the_conflicts() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &[]);

    git(&repo, &["checkout", "--quiet", "-b", "other"]);
    env.commit(&repo, "README.md", "# theirs\n", "theirs");
    git(&repo, &["checkout", "--quiet", "main"]);
    env.commit(&repo, "README.md", "# ours\n", "ours");

    // The merge is meant to fail — that failure *is* the conflict we want to
    // observe. Assert the state we actually depend on, so a merge that fails
    // for an unrelated reason says so here instead of surfacing as a confusing
    // row mismatch further down.
    let merge = git_output(&repo, &["merge", "other"]);
    assert!(
        repo.join(".git").join("MERGE_HEAD").exists(),
        "expected a conflicted merge to be in progress; git said {}",
        describe(&merge)
    );

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "api")[3], "merging !1", "{stdout}");
}

#[test]
fn a_registered_path_that_has_been_deleted_renders_a_row() {
    let env = TestEnv::new();
    let doomed = env.repo("doomed");
    env.register("gone", &doomed, &[]);
    env.register("here", &env.repo("here"), &[]);
    std::fs::remove_dir_all(&doomed).unwrap();

    let out = env.grit().arg("status").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "gone")[2], "missing", "{stdout}");
    // The surviving repo is still reported: one stale entry must not take the
    // whole dashboard down.
    assert_eq!(row(&stdout, "here")[1], "main", "{stdout}");
    assert!(stdout.contains("1 missing"), "{stdout}");

    env.grit().arg("status").assert().success();
}

#[test]
fn filters_narrow_the_dashboard() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);
    env.register("docs", &env.repo("docs"), &["release"]);
    env.register("solo", &env.repo("solo"), &[]);

    env.grit()
        .args(["status", "--tag", "release"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 repos"))
        .stdout(predicate::str::contains("solo").not());

    env.grit()
        .args(["status", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 repo"))
        .stdout(predicate::str::contains("docs").not());
}

#[test]
fn an_unknown_alias_in_a_filter_is_an_error() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    env.grit()
        .args(["status", "ghost"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no repo registered under alias"));
}

#[test]
fn json_output_carries_the_full_snapshot() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &["release"]);
    env.write(&repo, "loose.txt", "x\n");

    let out = env.grit().args(["status", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");

    let entry = &json[0];
    assert_eq!(entry["alias"], "api");
    assert_eq!(entry["branch"], "main");
    assert_eq!(entry["untracked"], 1);
    assert_eq!(entry["state"], "normal");
    assert_eq!(entry["head"]["subject"], "initial commit");
}

#[test]
fn an_empty_registry_produces_an_empty_json_array() {
    let out = TestEnv::new()
        .grit()
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
}

#[test]
fn piped_output_carries_no_ansi_escapes() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    let out = env
        .grit()
        .env_remove("NO_COLOR")
        .arg("status")
        .output()
        .unwrap();

    assert!(!String::from_utf8_lossy(&out.stdout).contains('\u{1b}'));
}

#[test]
fn no_color_suppresses_escapes_even_when_colour_is_auto() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    let out = env
        .grit()
        .env("NO_COLOR", "1")
        .arg("status")
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains('\u{1b}'));
}

#[test]
fn a_narrow_terminal_truncates_rather_than_wrapping() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &[]);
    env.commit(
        &repo,
        "x.txt",
        "x\n",
        "a really quite long commit subject that will not fit anywhere",
    );

    let out = env
        .grit()
        .env("COLUMNS", "70")
        .arg("status")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    for line in stdout.lines() {
        assert!(
            line.chars().count() <= 70,
            "line is {} wide:\n{line}",
            line.chars().count()
        );
    }
    assert!(stdout.contains('…'), "expected truncation in:\n{stdout}");
}
