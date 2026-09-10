//! `grit detail` — one repository, in depth.
//!
//! The parsers behind this are unit-tested against fixture strings in
//! `vcs/git.rs`; what is left for here is the part that needs a real
//! repository: that the sections appear when there is something to put in
//! them, stay away when there is not, and that the card and the dashboard
//! never disagree about the same repo.

mod common;

use assert_cmd::prelude::*;
use common::TestEnv;
use predicates::prelude::*;

/// A repo with one of everything: staged, unstaged, untracked, a stash, a
/// second branch and more than one commit.
fn busy(env: &TestEnv) -> std::path::PathBuf {
    let repo = env.repo("api");
    env.commit(&repo, "a.txt", "one\n", "first commit");
    env.commit(&repo, "b.txt", "two\n", "second commit");

    env.write(&repo, "stashed.txt", "put away\n");
    common::git(&repo, &["add", "stashed.txt"]);
    common::git(&repo, &["stash", "push", "-m", "wip: a stash"]);

    common::git(&repo, &["branch", "feature/thing"]);

    env.write(&repo, "a.txt", "one\nstaged\n");
    common::git(&repo, &["add", "a.txt"]);
    env.write(&repo, "b.txt", "two\nunstaged\n");
    env.write(&repo, "new.txt", "untracked\n");

    repo
}

#[test]
fn the_card_names_the_repo_its_branch_and_where_it_lives() {
    let env = TestEnv::new();
    let repo = busy(&env);
    env.register("api", &repo, &["release"]);

    env.grit()
        .args(["detail", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api"))
        .stdout(predicate::str::contains("main"))
        .stdout(predicate::str::contains("git"))
        .stdout(predicate::str::contains("release"));
}

#[test]
fn every_section_appears_when_there_is_something_in_it() {
    let env = TestEnv::new();
    let repo = busy(&env);
    env.register("api", &repo, &[]);

    let out = env.grit().args(["detail", "api"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);

    for section in ["CHANGES", "COMMITS", "BRANCHES", "STASHES"] {
        assert!(text.contains(section), "no {section} section in:\n{text}");
    }
}

#[test]
fn the_changed_paths_are_listed_with_a_side_each() {
    let env = TestEnv::new();
    let repo = busy(&env);
    env.register("api", &repo, &[]);

    let out = env.grit().args(["detail", "api"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);

    // Staged on the left, unstaged on the right, untracked as `?`.
    assert!(text.contains("M   a.txt"), "staged a.txt missing:\n{text}");
    assert!(
        text.contains(" M  b.txt"),
        "unstaged b.txt missing:\n{text}"
    );
    assert!(text.contains(" ?  new.txt"), "untracked missing:\n{text}");
}

#[test]
fn a_clean_repo_shows_a_header_and_no_changes_section() {
    let env = TestEnv::new();
    let repo = env.repo("docs");
    env.commit(&repo, "guide.md", "hi\n", "first commit");
    env.register("docs", &repo, &[]);

    let out = env.grit().args(["detail", "docs"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);

    assert!(text.contains("docs"), "{text}");
    assert!(text.contains("COMMITS"), "{text}");
    // Sections with nothing in them are left out rather than headed and empty.
    assert!(!text.contains("CHANGES"), "{text}");
    assert!(!text.contains("STASHES"), "{text}");
}

#[test]
fn the_card_and_the_dashboard_agree_about_the_same_repo() {
    let env = TestEnv::new();
    let repo = busy(&env);
    env.register("api", &repo, &[]);

    let card: serde_json::Value = serde_json::from_slice(
        &env.grit()
            .args(["detail", "api", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let dashboard: serde_json::Value = serde_json::from_slice(
        &env.grit()
            .args(["status", "api", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    // The counts are the header of both, and the card's file list is what
    // those counts are counting — a card that listed three changes under a
    // header saying two would be worse than either on its own.
    for field in ["staged", "unstaged", "untracked", "conflicts", "stashes"] {
        assert_eq!(
            card["snapshot"][field], dashboard[0][field],
            "{field} disagrees: card {card:#}\ndashboard {dashboard:#}"
        );
    }
}

#[test]
fn the_file_list_and_the_counts_are_the_same_reading() {
    let env = TestEnv::new();
    let repo = busy(&env);
    env.register("api", &repo, &[]);

    let card: serde_json::Value = serde_json::from_slice(
        &env.grit()
            .args(["detail", "api", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let files = card["files"].as_array().expect("files is an array");
    let staged = files.iter().filter(|f| f["staged"].is_string()).count();
    let unstaged = files.iter().filter(|f| f["unstaged"].is_string()).count();

    assert_eq!(staged as u64, card["snapshot"]["staged"].as_u64().unwrap());
    // Untracked entries sit on the unstaged side, and the dashboard counts
    // those in a column of their own.
    let untracked = card["snapshot"]["untracked"].as_u64().unwrap();
    assert_eq!(
        unstaged as u64,
        card["snapshot"]["unstaged"].as_u64().unwrap() + untracked
    );
}

#[test]
fn the_json_carries_the_registry_facts_as_well_as_the_reading() {
    let env = TestEnv::new();
    let repo = busy(&env);
    env.register("api", &repo, &["release", "backend"]);

    let card: serde_json::Value = serde_json::from_slice(
        &env.grit()
            .args(["detail", "api", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    assert_eq!(card["alias"], "api");
    assert_eq!(card["kind"], "git");
    let tags: Vec<&str> = card["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert!(tags.contains(&"release"), "{tags:?}");
    assert!(tags.contains(&"backend"), "{tags:?}");
    assert!(card["branches"].as_array().unwrap().len() >= 2);
    assert_eq!(card["stashes"][0]["id"], "stash@{0}");
}

#[test]
fn an_unknown_alias_says_so_rather_than_showing_an_empty_card() {
    let env = TestEnv::new();
    env.grit()
        .args(["detail", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no repo registered under alias"));
}

#[test]
fn a_repo_whose_directory_is_gone_is_a_card_saying_missing() {
    let env = TestEnv::new();
    let repo = env.repo("gone");
    env.commit(&repo, "a.txt", "one\n", "first commit");
    env.register("gone", &repo, &[]);
    std::fs::remove_dir_all(&repo).unwrap();

    // Information, not a failure — the same posture the dashboard takes.
    env.grit()
        .args(["detail", "gone"])
        .assert()
        .success()
        .stdout(predicate::str::contains("missing"));
}

#[test]
fn a_repo_with_no_commits_yet_still_renders() {
    let env = TestEnv::new();
    // Not `TestEnv::repo`, which commits a README on the way out.
    let repo = env.root().join("fresh");
    std::fs::create_dir_all(&repo).unwrap();
    common::git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    env.register("fresh", &repo, &[]);

    let out = env.grit().args(["detail", "fresh"]).output().unwrap();
    assert!(out.status.success(), "{}", common::describe(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("fresh"), "{text}");
    assert!(!text.contains("COMMITS"), "{text}");
}

#[test]
fn a_conflicted_merge_marks_the_path_on_both_sides() {
    let env = TestEnv::new();
    let repo = env.repo("clash");
    env.commit(&repo, "f.txt", "base\n", "base");
    common::git(&repo, &["checkout", "-q", "-b", "side"]);
    env.commit(&repo, "f.txt", "side\n", "side");
    common::git(&repo, &["checkout", "-q", "main"]);
    env.commit(&repo, "f.txt", "main\n", "main");
    let _ = common::git_output(&repo, &["merge", "side"]);
    env.register("clash", &repo, &[]);

    let out = env.grit().args(["detail", "clash"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("UU  f.txt"), "no conflict row in:\n{text}");
    assert!(text.contains("merging"), "{text}");
}

#[test]
fn a_rename_says_where_the_path_came_from() {
    let env = TestEnv::new();
    let repo = env.repo("moved");
    env.commit(&repo, "old.txt", "content\n", "first commit");
    common::git(&repo, &["mv", "old.txt", "new.txt"]);
    env.register("moved", &repo, &[]);

    env.grit()
        .args(["detail", "moved"])
        .assert()
        .success()
        .stdout(predicate::str::contains("new.txt ← old.txt"));
}

#[test]
fn a_path_that_is_not_ascii_is_not_shown_escaped() {
    // git quotes a non-ASCII path by default, which would put the literal
    // characters `"caf\303\251.txt"` in the list.
    let env = TestEnv::new();
    let repo = env.repo("unicode");
    env.commit(&repo, "a.txt", "one\n", "first commit");
    env.write(&repo, "café.txt", "hi\n");
    env.register("unicode", &repo, &[]);

    let out = env.grit().args(["detail", "unicode"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("café.txt"), "path came back escaped:\n{text}");
    assert!(!text.contains("\\303"), "{text}");
}

#[test]
fn nothing_is_painted_when_stdout_is_not_a_terminal() {
    let env = TestEnv::new();
    let repo = busy(&env);
    env.register("api", &repo, &[]);

    let out = env.grit().args(["detail", "api"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains('\u{1b}'),
        "an escape reached a pipe:\n{text}"
    );
}

#[test]
fn the_detail_view_takes_a_reading_rather_than_using_the_cache() {
    let env = TestEnv::new();
    let repo = env.repo("live");
    env.commit(&repo, "a.txt", "one\n", "first commit");
    env.register("live", &repo, &[]);

    // Warm the cache while the repo is clean, then dirty it. A card drawn
    // from the cache would still say clean.
    env.grit().arg("status").assert().success();
    env.write(&repo, "a.txt", "one\nchanged\n");

    env.grit()
        .args(["detail", "live"])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt"))
        .stdout(predicate::str::contains("CHANGES"));
}
