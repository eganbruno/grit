//! `grit branch` — the branch listing, against real repositories.

mod common;

use assert_cmd::prelude::*;
use common::{TestEnv, describe, dolt_available, git};
use predicates::prelude::*;

/// The rendered row for a branch, by the branch name it contains.
///
/// Split on runs of two or more spaces, which is the gap the renderer puts
/// between columns, so a single space inside a subject stays put.
fn branch_row(output: &str, branch: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim_end)
        .find(|line| {
            line.split("  ")
                .map(str::trim)
                .any(|cell| cell == branch || cell == format!("* {branch}"))
        })
        .unwrap_or_else(|| panic!("no row for branch `{branch}` in:\n{output}"))
        .trim()
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn stdout_of(env: &TestEnv, args: &[&str]) -> String {
    let out = env.grit().args(args).output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn an_empty_registry_says_what_to_type_next() {
    TestEnv::new()
        .grit()
        .arg("branch")
        .assert()
        .success()
        .stdout(predicate::str::contains("No repositories registered"));
}

#[test]
fn every_branch_of_every_repo_is_listed() {
    let env = TestEnv::new();
    let api = env.repo("api");
    git(&api, &["branch", "wip"]);
    git(&api, &["branch", "spike"]);
    env.register("api", &api, &[]);

    let out = stdout_of(&env, &["branch"]);

    for branch in ["main", "wip", "spike"] {
        assert!(
            branch_row(&out, branch).contains(&"api".to_string()),
            "{branch} missing its alias in:\n{out}"
        );
    }
    assert!(out.contains("3 branches"), "{out}");
}

#[test]
fn the_checked_out_branch_is_starred_and_the_others_are_not() {
    let env = TestEnv::new();
    let api = env.repo("api");
    git(&api, &["branch", "wip"]);
    env.register("api", &api, &[]);

    let out = stdout_of(&env, &["branch"]);

    assert!(branch_row(&out, "main").contains(&"*".to_string()), "{out}");
    assert!(!branch_row(&out, "wip").contains(&"*".to_string()), "{out}");
}

#[test]
fn a_commit_body_never_reaches_the_table() {
    // The whole point of the command: `git branch -vv` prints the message,
    // this prints the subject, so a body cannot push the next row's columns
    // sideways.
    let env = TestEnv::new();
    let api = env.repo("api");
    env.commit(
        &api,
        "a.txt",
        "a\n",
        "the subject\n\nand a body\nover two lines",
    );
    env.register("api", &api, &[]);

    let out = stdout_of(&env, &["branch"]);

    assert!(out.contains("the subject"), "{out}");
    assert!(!out.contains("and a body"), "{out}");
    // One branch, so: header, rule, one row, blank, footer.
    assert_eq!(
        out.lines().filter(|l| l.contains("the subject")).count(),
        1,
        "{out}"
    );
}

#[test]
fn a_long_subject_is_cut_to_the_same_width_the_dashboard_uses() {
    let env = TestEnv::new();
    let api = env.repo("api");
    let subject = "x".repeat(200);
    env.commit(&api, "a.txt", "a\n", &subject);
    env.register("api", &api, &[]);

    // COLUMNS is pinned at 200 by TestEnv, so nothing here is the terminal
    // running out of room — the cap is doing the cutting.
    let out = stdout_of(&env, &["branch"]);

    assert!(out.contains('…'), "{out}");
    assert!(!out.contains(&"x".repeat(61)), "{out}");
}

#[test]
fn a_branch_ahead_of_its_upstream_shows_the_count() {
    let env = TestEnv::new();
    let origin = env.repo("origin");
    let clone = env.clone_of(&origin, "clone");
    env.commit(&clone, "b.txt", "b\n", "local work");
    env.register("api", &clone, &[]);

    let out = stdout_of(&env, &["branch"]);
    assert!(
        branch_row(&out, "main").contains(&"↑1".to_string()),
        "{out}"
    );
    assert!(out.contains("1 ahead"), "{out}");
}

#[test]
fn a_branch_level_with_its_upstream_is_ticked() {
    let env = TestEnv::new();
    let origin = env.repo("origin");
    let clone = env.clone_of(&origin, "clone");
    env.register("api", &clone, &[]);

    let out = stdout_of(&env, &["branch"]);
    assert!(branch_row(&out, "main").contains(&"✓".to_string()), "{out}");
}

#[test]
fn a_branch_with_no_upstream_is_marked_as_having_none() {
    let env = TestEnv::new();
    let api = env.repo("api");
    git(&api, &["branch", "wip"]);
    env.register("api", &api, &[]);

    assert!(branch_row(&stdout_of(&env, &["branch"]), "wip").contains(&"·".to_string()),);
}

#[test]
fn a_deleted_upstream_reads_as_gone_rather_than_as_in_sync() {
    let env = TestEnv::new();
    let origin = env.repo("origin");
    let clone = env.clone_of(&origin, "clone");
    git(&clone, &["checkout", "--quiet", "-b", "feature"]);
    git(&clone, &["config", "branch.feature.remote", "origin"]);
    git(
        &clone,
        &["config", "branch.feature.merge", "refs/heads/feature"],
    );
    env.register("api", &clone, &[]);

    assert!(branch_row(&stdout_of(&env, &["branch"]), "feature").contains(&"gone".to_string()),);
}

#[test]
fn a_tag_narrows_the_listing() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);
    env.register("web", &env.repo("web"), &[]);

    let out = stdout_of(&env, &["branch", "--tag", "release"]);
    assert!(out.contains("api"), "{out}");
    assert!(!out.contains("web"), "{out}");
}

#[test]
fn an_alias_narrows_the_listing() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.register("web", &env.repo("web"), &[]);

    let out = stdout_of(&env, &["branch", "api"]);
    assert!(out.contains("api"), "{out}");
    assert!(!out.contains("web"), "{out}");
}

#[test]
fn json_carries_the_upstream_the_table_only_summarises() {
    let env = TestEnv::new();
    let origin = env.repo("origin");
    let clone = env.clone_of(&origin, "clone");
    env.commit(&clone, "b.txt", "b\n", "local work");
    env.register("api", &clone, &[]);

    let out = stdout_of(&env, &["branch", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");

    let branch = &parsed[0]["branches"][0];
    assert_eq!(branch["name"], "main");
    assert_eq!(branch["is_head"], true);
    assert_eq!(branch["tracking"]["state"], "tracked");
    assert_eq!(branch["tracking"]["upstream"], "origin/main");
    assert_eq!(branch["tracking"]["ahead"], 1);
}

#[test]
fn json_for_an_empty_registry_is_an_empty_array() {
    TestEnv::new()
        .grit()
        .args(["branch", "--json"])
        .assert()
        .success()
        .stdout("[]\n");
}

#[test]
fn a_repo_whose_directory_is_gone_says_missing_rather_than_empty() {
    let env = TestEnv::new();
    let api = env.repo("api");
    env.register("api", &api, &[]);
    env.register("web", &env.repo("web"), &[]);
    std::fs::remove_dir_all(&api).expect("remove repo");

    let out = env.grit().arg("branch").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    // "no branches yet" is a real repo with no commits, which is a different
    // thing entirely — and a stale entry must not take the exit code down.
    assert!(stdout.contains("missing"), "{stdout}");
    assert!(!stdout.contains("no branches yet"), "{stdout}");
    assert!(stdout.contains("web"), "{stdout}");
    assert!(out.status.success(), "{}", describe(&out));
}

#[test]
fn colour_is_dropped_when_stdout_is_not_a_terminal() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    let out = stdout_of(&env, &["branch"]);
    assert!(!out.contains('\u{1b}'), "{out:?}");
}

#[test]
fn a_dolt_database_lists_its_branches_too() {
    if !dolt_available() {
        eprintln!("skipping: dolt is not installed");
        return;
    }

    let env = TestEnv::new();
    let db = env.dolt_repo("data");
    env.dolt(&db, &["checkout", "-b", "ingress/eea-2026"]);
    env.dolt_commit(&db, "insert into items values (2, 'eea')", "add eea rows");
    env.register("data", &db, &[]);

    let out = stdout_of(&env, &["branch"]);
    let row = branch_row(&out, "ingress/eea-2026");

    assert!(row.contains(&"data".to_string()), "{out}");
    assert!(row.contains(&"*".to_string()), "{out}");
    assert!(out.contains("add eea rows"), "{out}");
}

#[test]
fn a_dolt_commit_body_never_reaches_the_table() {
    // `dolt_log.message` is the whole message; the listing must cut it the
    // same way the dashboard does.
    if !dolt_available() {
        eprintln!("skipping: dolt is not installed");
        return;
    }

    let env = TestEnv::new();
    let db = env.dolt_repo("data");
    env.dolt_commit(
        &db,
        "insert into items values (2, 'eea')",
        "the subject\n\nand a body dolt would print in full",
    );
    env.register("data", &db, &[]);

    let out = stdout_of(&env, &["branch"]);
    assert!(out.contains("the subject"), "{out}");
    assert!(!out.contains("and a body"), "{out}");
}
