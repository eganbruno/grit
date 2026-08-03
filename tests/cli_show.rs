//! `grit show`.

mod common;

use assert_cmd::prelude::*;
use common::{TestEnv, row};
use predicates::prelude::*;

#[test]
fn an_empty_registry_says_what_to_type_next() {
    let env = TestEnv::new();

    env.grit()
        .arg("show")
        .assert()
        .success()
        .stdout(predicate::str::contains("No repositories registered"))
        .stdout(predicate::str::contains("grit -r <alias> <path>"));
}

#[test]
fn lists_alias_kind_tags_and_path() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &["release", "core"]);

    let out = env.grit().arg("show").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    let cells = row(&stdout, "api");
    assert_eq!(cells[0], "api");
    assert_eq!(cells[1], "git");
    assert_eq!(cells[2], "core, release");
    assert_eq!(cells[3], repo.to_string_lossy());
}

#[test]
fn a_repo_with_no_tags_shows_a_placeholder_not_a_gap() {
    let env = TestEnv::new();
    env.register("solo", &env.repo("solo"), &[]);

    let out = env.grit().arg("show").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(row(&stdout, "solo")[2], "·");
}

#[test]
fn rows_are_alphabetical_regardless_of_registration_order() {
    let env = TestEnv::new();
    env.register("zulu", &env.repo("z"), &[]);
    env.register("alpha", &env.repo("a"), &[]);
    env.register("mike", &env.repo("m"), &[]);

    let out = env.grit().arg("show").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    let order: Vec<&str> = ["alpha", "mike", "zulu"]
        .into_iter()
        .filter(|a| stdout.contains(*a))
        .collect();
    let positions: Vec<usize> = order.iter().map(|a| stdout.find(a).unwrap()).collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "not alphabetical:\n{stdout}"
    );
}

#[test]
fn the_tag_filter_narrows_the_list() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);
    env.register("solo", &env.repo("solo"), &[]);

    env.grit()
        .args(["show", "--tag", "release"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api"))
        .stdout(predicate::str::contains("solo").not());
}

#[test]
fn an_unknown_tag_is_an_error_rather_than_an_empty_table() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);

    env.grit()
        .args(["show", "--tag", "relase"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no repos tagged `relase`"));
}

#[test]
fn the_footer_reports_the_count_tags_and_config_path() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);
    env.register("docs", &env.repo("docs"), &["release"]);

    env.grit()
        .arg("show")
        .assert()
        .success()
        .stdout(predicate::str::contains("2 repos"))
        .stdout(predicate::str::contains("tags: release"))
        .stdout(predicate::str::contains("config.toml"));
}

#[test]
fn json_output_is_machine_readable() {
    let env = TestEnv::new();
    let repo = env.repo("api");
    env.register("api", &repo, &["release"]);

    let out = env.grit().args(["show", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");

    let entry = &json["repos"][0];
    assert_eq!(entry["alias"], "api");
    assert_eq!(entry["kind"], "git");
    assert_eq!(entry["path"].as_str().unwrap(), repo.to_str().unwrap());
    assert_eq!(entry["tags"][0], "release");
    assert!(entry["added_at"].is_string());
}

#[test]
fn piped_output_carries_no_ansi_escapes() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &["release"]);

    let out = env
        .grit()
        .env_remove("NO_COLOR")
        .arg("show")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains('\u{1b}'),
        "unexpected ANSI in:\n{stdout:?}"
    );
}

#[test]
fn color_always_emits_escapes_even_when_piped() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    let out = env
        .grit()
        .env_remove("NO_COLOR")
        .args(["--color", "always", "show"])
        .output()
        .unwrap();

    assert!(String::from_utf8_lossy(&out.stdout).contains('\u{1b}'));
}
