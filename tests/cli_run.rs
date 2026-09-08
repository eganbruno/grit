//! `grit <alias|@tag> <args...>` — passthrough and group fan-out.

mod common;

use assert_cmd::prelude::*;
use common::{TestEnv, git};
use predicates::prelude::*;

#[test]
fn runs_git_inside_the_aliased_repo_from_somewhere_else() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &[]);

    // Deliberately run from the temp root, not from inside the repo.
    env.grit()
        .args(["api", "rev-parse", "--show-toplevel"])
        .assert()
        .success()
        .stdout(predicate::str::contains(repo.to_str().unwrap()));
}

#[test]
fn git_flags_are_passed_through_untouched() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    env.grit()
        .args(["api", "log", "--oneline", "-n", "1", "--format=%s"])
        .assert()
        .success()
        .stdout(predicate::str::contains("initial commit"));
}

#[test]
fn a_command_that_mutates_the_repo_takes_effect() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &[]);

    env.grit()
        .args(["api", "checkout", "--quiet", "-b", "feature/x"])
        .assert()
        .success();

    assert_eq!(
        git(&repo, &["branch", "--show-current"]).trim(),
        "feature/x"
    );
}

#[test]
fn the_child_exit_code_becomes_grits_exit_code() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    let status = env
        .grit()
        .args(["api", "rev-parse", "--verify", "no-such-ref"])
        .status()
        .unwrap();

    assert_eq!(
        status.code(),
        Some(128),
        "git's own exit code should survive"
    );
}

#[test]
fn an_unknown_alias_fails_before_running_anything() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    env.grit()
        .args(["ghost", "status"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no repo registered under alias"));
}

#[test]
fn a_target_with_no_command_explains_what_to_type() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    env.grit()
        .arg("api")
        .assert()
        .failure()
        .stderr(predicate::str::contains("needs a command to run"))
        .stderr(predicate::str::contains("grit status api"));
}

#[test]
fn a_group_runs_the_command_in_every_tagged_repo() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);
    env.register("docs", &env.repo("docs"), &["release"]);
    env.register("solo", &env.repo("solo"), &[]);

    let out = env
        .grit()
        .args(["@release", "rev-parse", "--show-toplevel"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("── api ─"), "{stdout}");
    assert!(stdout.contains("── docs ─"), "{stdout}");
    assert!(!stdout.contains("── solo ─"), "{stdout}");
    assert!(stdout.contains("2 repos"), "{stdout}");
    assert!(stdout.contains("all ok"), "{stdout}");
}

#[test]
fn group_output_is_ordered_and_labelled() {
    let env = TestEnv::new();
    env.register("bravo", &env.repo("b"), &["grp"]);
    env.register("alpha", &env.repo("a"), &["grp"]);

    let out = env
        .grit()
        .args(["@grp", "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    let alpha = stdout.find("── alpha ─").expect("alpha header");
    let bravo = stdout.find("── bravo ─").expect("bravo header");
    assert!(alpha < bravo, "expected alphabetical order:\n{stdout}");
}

#[test]
fn an_unknown_group_fails_before_running_anything() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);

    env.grit()
        .args(["@relase", "fetch"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no repos tagged `relase`"));
}

#[test]
fn a_fan_out_stops_at_the_first_failure() {
    let env = TestEnv::new();
    env.register("alpha", &env.repo("a"), &["grp"]);
    env.register("bravo", &env.repo("b"), &["grp"]);

    let out = env
        .grit()
        .args(["@grp", "rev-parse", "--verify", "no-such-ref"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("── alpha ─"), "{stdout}");
    assert!(
        !stdout.contains("── bravo ─"),
        "should have stopped:\n{stdout}"
    );
    assert!(stdout.contains("pass -k to keep going"), "{stdout}");
    assert_eq!(out.status.code(), Some(128));
}

#[test]
fn keep_going_visits_every_repo_and_still_fails() {
    let env = TestEnv::new();
    env.register("alpha", &env.repo("a"), &["grp"]);
    env.register("bravo", &env.repo("b"), &["grp"]);

    let out = env
        .grit()
        .args(["-k", "@grp", "rev-parse", "--verify", "no-such-ref"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("── alpha ─"), "{stdout}");
    assert!(stdout.contains("── bravo ─"), "{stdout}");
    assert!(stdout.contains("failed: alpha, bravo"), "{stdout}");
    assert_eq!(out.status.code(), Some(128));
}

#[test]
fn a_group_of_one_skips_the_divider() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);

    let out = env
        .grit()
        .args(["@release", "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(stdout.trim(), "main", "a single repo needs no chrome");
}

// `help` and `version` reach grit as passthrough targets, because clap has the
// help subcommand disabled and `--version` is a flag. They are answered rather
// than looked up as aliases, which is what they used to be.

#[test]
fn the_bare_word_help_prints_help_rather_than_hunting_for_an_alias() {
    let env = TestEnv::new();

    env.grit()
        .arg("help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Work across many repositories"))
        .stdout(predicate::str::contains("Examples:"));
}

#[test]
fn help_takes_a_command_and_shows_that_commands_help() {
    let env = TestEnv::new();

    env.grit()
        .args(["help", "status"])
        .assert()
        .success()
        // The usage line has to name the whole path, not the bare leaf.
        .stdout(predicate::str::contains("Usage: grit status"));

    env.grit()
        .args(["help", "shell", "enable"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: grit shell enable"));
}

#[test]
fn help_for_something_that_is_not_a_command_says_so() {
    let env = TestEnv::new();

    env.grit()
        .args(["help", "nonsense"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("`nonsense` is not a grit command"));
}

#[test]
fn the_bare_word_version_agrees_with_the_flag() {
    let env = TestEnv::new();

    let word = env.grit().arg("version").output().unwrap();
    let flag = env.grit().arg("--version").output().unwrap();

    assert!(word.status.success());
    assert_eq!(
        String::from_utf8_lossy(&word.stdout),
        String::from_utf8_lossy(&flag.stdout),
        "`grit version` and `grit --version` must not drift"
    );
}

#[test]
fn a_repo_may_still_be_called_something_that_merely_looks_like_a_command() {
    let env = TestEnv::new();
    env.register("helper", &env.repo("helper"), &[]);

    // Only the exact words are intercepted; `helper` is an ordinary alias.
    env.grit()
        .args(["helper", "rev-parse", "--abbrev-ref", "HEAD"])
        .assert()
        .success()
        .stdout(predicate::str::contains("main"));
}
