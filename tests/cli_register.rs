//! `grit -r` / `grit register` / `grit remove`.

mod common;

use assert_cmd::prelude::*;
use common::TestEnv;
use predicates::prelude::*;

#[test]
fn registers_a_repo_and_writes_the_config() {
    let env = TestEnv::new();
    let repo = env.repo("docs");

    env.grit()
        .args(["-r", "docs"])
        .arg(&repo)
        .assert()
        .success()
        .stdout(predicate::str::contains("registered docs"));

    let config = env.config_contents();
    assert!(config.contains("[repos.docs]"), "{config}");
    assert!(config.contains(repo.to_str().unwrap()), "{config}");
}

#[test]
fn the_register_subcommand_is_equivalent_to_the_shorthand() {
    let env = TestEnv::new();
    let repo = env.repo("docs");

    env.grit()
        .args(["register", "docs"])
        .arg(&repo)
        .assert()
        .success();

    assert!(env.config_contents().contains("[repos.docs]"));
}

#[test]
fn add_is_accepted_as_an_alias_for_register() {
    let env = TestEnv::new();
    let repo = env.repo("docs");

    env.grit()
        .args(["add", "docs"])
        .arg(&repo)
        .assert()
        .success();

    assert!(env.config_contents().contains("[repos.docs]"));
}

#[test]
fn a_path_inside_the_repo_registers_the_repo_root() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    let nested = repo.join("src").join("deep");
    std::fs::create_dir_all(&nested).unwrap();

    env.grit()
        .args(["-r", "api"])
        .arg(&nested)
        .assert()
        .success();

    let config = env.config_contents();
    assert!(
        config.contains(&format!("path = \"{}\"", repo.display())),
        "expected the repo root, got:\n{config}"
    );
}

#[test]
fn the_path_defaults_to_the_current_directory() {
    let env = TestEnv::new();
    let repo = env.repo("api");

    env.grit()
        .current_dir(&repo)
        .args(["-r", "api"])
        .assert()
        .success();

    assert!(
        env.config_contents()
            .contains(&format!("path = \"{}\"", repo.display()))
    );
}

#[test]
fn tags_are_stored_sorted_and_deduplicated() {
    let env = TestEnv::new();
    let repo = env.repo("api");

    env.grit()
        .args(["-r", "api"])
        .arg(&repo)
        .args(["--tag", "release,core", "--tag", "release"])
        .assert()
        .success();

    assert!(
        env.config_contents()
            .contains(r#"tags = ["core", "release"]"#),
        "{}",
        env.config_contents()
    );
}

#[test]
fn a_directory_that_is_not_a_repo_is_refused_and_nothing_is_written() {
    let env = TestEnv::new();
    let plain = env.root().join("just-a-folder");
    std::fs::create_dir_all(&plain).unwrap();

    env.grit()
        .args(["-r", "nope"])
        .arg(&plain)
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a dolt or git repository"));

    assert!(!env.config_exists(), "config should not have been created");
}

#[test]
fn a_path_that_does_not_exist_is_refused() {
    let env = TestEnv::new();

    env.grit()
        .args(["-r", "ghost"])
        .arg(env.root().join("nowhere"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn a_command_name_cannot_be_used_as_an_alias() {
    let env = TestEnv::new();
    let repo = env.repo("docs");

    for reserved in ["status", "show", "register", "rm"] {
        env.grit()
            .args(["-r", reserved])
            .arg(&repo)
            .assert()
            .failure()
            .stderr(predicate::str::contains("is a grit command"));
    }

    assert!(!env.config_exists());
}

#[test]
fn an_alias_with_illegal_characters_is_refused() {
    let env = TestEnv::new();
    let repo = env.repo("docs");

    env.grit()
        .args(["-r", "has space"])
        .arg(&repo)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid alias"));
}

#[test]
fn a_duplicate_alias_needs_force() {
    let env = TestEnv::new();
    let first = env.repo("docs");
    let second = env.repo("dashboard");

    env.register("docs", &first, &[]);

    env.grit()
        .args(["-r", "docs"])
        .arg(&second)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already registered"));

    env.grit()
        .args(["-r", "docs"])
        .arg(&second)
        .arg("--force")
        .assert()
        .success()
        .stdout(predicate::str::contains("re-registered"));

    let config = env.config_contents();
    assert!(config.contains(second.to_str().unwrap()));
    assert!(!config.contains(&format!("\"{}\"", first.display())));
}

#[test]
fn remove_forgets_the_alias_but_leaves_the_repo_alone() {
    let env = TestEnv::new();
    let repo = env.repo("docs");
    env.register("docs", &repo, &[]);

    env.grit()
        .args(["rm", "docs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed docs"));

    assert!(!env.config_contents().contains("[repos.docs]"));
    assert!(repo.join(".git").exists(), "the repo itself must survive");
}

#[test]
fn removing_a_mix_of_known_and_unknown_aliases_changes_nothing() {
    let env = TestEnv::new();
    let repo = env.repo("docs");
    env.register("docs", &repo, &[]);

    env.grit()
        .args(["rm", "docs", "ghost"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no repo registered under alias"));

    assert!(
        env.config_contents().contains("[repos.docs]"),
        "the valid alias must not have been removed"
    );
}
