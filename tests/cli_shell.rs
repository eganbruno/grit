//! `grit shell` — the integration script, and the protocol it speaks.
//!
//! The protocol tests parse `grit shell preview` the way the emitted zsh does,
//! because an off-by-one in a character offset is invisible in the output and
//! obvious the moment you use it to slice the text.

mod common;

use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use common::TestEnv;
use predicates::prelude::*;

/// The preview protocol, taken apart: the highlight ranges and the table text.
struct Preview {
    highlights: Vec<(usize, usize, String)>,
    text: String,
}

impl Preview {
    fn parse(raw: &str) -> Self {
        let mut lines = raw.split('\n');
        let count: usize = lines
            .next()
            .expect("a first line")
            .parse()
            .unwrap_or_else(|e| panic!("first line is not a count ({e}) in:\n{raw}"));

        let highlights = (0..count)
            .map(|_| {
                let line = lines.next().expect("a highlight line");
                let mut parts = line.splitn(3, ' ');
                let mut next = || parts.next().unwrap_or_else(|| panic!("short: {line:?}"));
                (
                    next().parse().expect("a start offset"),
                    next().parse().expect("an end offset"),
                    next().to_string(),
                )
            })
            .collect();

        Self {
            highlights,
            text: lines.collect::<Vec<_>>().join("\n"),
        }
    }

    /// The characters a range covers — the only way to see an offset bug.
    fn covered(&self, index: usize) -> String {
        let (start, end, _) = &self.highlights[index];
        self.text.chars().skip(*start).take(end - start).collect()
    }

    fn find(&self, spec: &str, wanted: &str) -> bool {
        (0..self.highlights.len())
            .any(|i| self.highlights[i].2 == spec && self.covered(i) == wanted)
    }
}

fn preview(env: &TestEnv, args: &[&str]) -> String {
    let out = env
        .grit()
        .args(["shell", "preview"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", common::describe(&out));
    String::from_utf8(out.stdout).expect("utf-8")
}

/// Whether a shell is installed, so its syntax check can skip rather than fail.
fn available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn write_script(env: &TestEnv, shell: &str) -> std::path::PathBuf {
    let out = env
        .grit()
        .args(["shell", "init", shell])
        .output()
        .expect("run grit");
    assert!(out.status.success(), "{}", common::describe(&out));

    let path = env.root().join(format!("init.{shell}"));
    std::fs::write(&path, &out.stdout).expect("write script");
    path
}

/// `zsh -n` parses without running. A script that is emitted but does not parse
/// is a broken shell for whoever pasted the `eval` into their rc file, and no
/// Rust test would otherwise notice.
fn assert_parses(program: &str, args: &[&str], script: &Path) {
    if !available(program) {
        eprintln!("skipping: {program} is not installed");
        return;
    }
    let out = Command::new(program)
        .args(args)
        .arg(script)
        .output()
        .expect("run the shell");
    assert!(
        out.status.success(),
        "{program} rejected its own init script:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_zsh_script_is_valid_zsh() {
    let env = TestEnv::new();
    assert_parses("zsh", &["-n"], &write_script(&env, "zsh"));
}

#[test]
fn the_bash_script_is_valid_bash() {
    let env = TestEnv::new();
    assert_parses("bash", &["-n"], &write_script(&env, "bash"));
}

#[test]
fn the_fish_script_is_valid_fish() {
    let env = TestEnv::new();
    assert_parses("fish", &["--no-execute"], &write_script(&env, "fish"));
}

#[test]
fn an_unknown_shell_is_refused_rather_than_guessed() {
    TestEnv::new()
        .grit()
        .args(["shell", "init", "csh"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("zsh"));
}

#[test]
fn a_cold_cache_previews_nothing_at_all() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    // Not an error and not a placeholder: the integration is about to start a
    // refresh, and a row that says "loading" would only flicker.
    let out = env
        .grit()
        .args(["shell", "preview"])
        .output()
        .expect("run grit");
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

#[test]
fn a_refresh_fills_the_cache_without_saying_anything() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);

    env.grit()
        .args(["shell", "refresh"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    assert!(env.cache_exists(), "refresh wrote no cache");
    assert!(!preview(&env, &[]).is_empty());
}

#[test]
fn a_fresh_enough_cache_is_left_alone() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();

    let before = std::fs::read_to_string(env.cache()).unwrap();
    env.grit()
        .args(["shell", "refresh", "--max-age", "3600"])
        .assert()
        .success();

    // The point of --max-age: the integration fires a refresh on every idle
    // tick, and without this that is a `git status` per repo per second.
    assert_eq!(before, std::fs::read_to_string(env.cache()).unwrap());
}

#[test]
fn every_highlight_covers_real_characters_on_one_line() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.register("docs", &env.repo("docs"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();

    let preview = Preview::parse(&preview(&env, &[]));
    let total = preview.text.chars().count();
    assert!(!preview.highlights.is_empty(), "nothing was styled");

    for (i, (start, end, spec)) in preview.highlights.iter().enumerate() {
        assert!(start < end, "empty range {start}..{end}");
        assert!(
            *end <= total,
            "range {start}..{end} runs past {total} chars"
        );
        assert!(
            !preview.covered(i).contains('\n'),
            "{spec} at {start}..{end} straddles a line break"
        );
    }
}

#[test]
fn the_ranges_land_on_the_cells_they_describe() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();

    let preview = Preview::parse(&preview(&env, &[]));

    // An offset counted in bytes rather than characters still lands inside the
    // text; only checking *what* it covers catches that.
    assert!(preview.find("bold", "api"), "the alias is not bold");
    assert!(preview.find("fg=cyan", "main"), "the branch is not cyan");
}

#[test]
fn the_preview_carries_no_ansi_of_its_own() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();

    // zsh renders POSTDISPLAY literally, so an escape here would appear on
    // screen as the characters `ESC [ 3 6 m`.
    assert!(!preview(&env, &[]).contains('\u{1b}'));
}

#[test]
fn max_rows_trims_the_table_and_says_how_many_were_left() {
    let env = TestEnv::new();
    for alias in ["api", "docs", "web"] {
        env.register(alias, &env.repo(alias), &[]);
    }
    env.grit().args(["shell", "refresh"]).assert().success();

    let preview = Preview::parse(&preview(&env, &["--max-rows", "1"]));

    assert!(preview.text.contains("api"), "{}", preview.text);
    assert!(!preview.text.contains("web"), "{}", preview.text);
    assert!(preview.text.contains("2 more"), "{}", preview.text);
    // The summary still counts every repo, not just the visible ones.
    assert!(preview.text.contains("3 repos"), "{}", preview.text);
}

#[test]
fn no_room_means_no_preview() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();

    assert!(preview(&env, &["--max-rows", "0"]).is_empty());
}

#[test]
fn a_registry_change_invalidates_the_preview() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();
    assert!(!preview(&env, &[]).is_empty());

    // A row for a repo that is no longer registered — or a missing row for one
    // that now is — would be a lie rather than a stale truth.
    env.register("docs", &env.repo("docs"), &[]);
    assert!(preview(&env, &[]).is_empty());
}

#[test]
fn an_empty_registry_previews_nothing() {
    let env = TestEnv::new();
    env.grit().args(["shell", "refresh"]).assert().success();

    assert!(preview(&env, &[]).is_empty());
}

#[test]
fn a_reading_too_old_to_trust_is_withheld() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();

    // The table is read at a glance and acted on. One from this morning, drawn
    // exactly like a live one, is the failure the whole feature has to avoid —
    // so the caller gets nothing and starts a refresh instead.
    backdate(env.cache(), "2020-01-01T00:00:00Z");
    assert!(preview(&env, &["--max-age", "60"]).is_empty());

    // Without the flag it is still rendered: `grit status --cached` says how
    // old its reading is and lets you decide.
    assert!(!preview(&env, &[]).is_empty());
}

#[test]
fn a_fresh_reading_passes_the_same_check() {
    let env = TestEnv::new();
    env.register("api", &env.repo("api"), &[]);
    env.grit().args(["shell", "refresh"]).assert().success();

    assert!(!preview(&env, &["--max-age", "60"]).is_empty());
}

/// Rewrite the cache's timestamp, leaving everything else as it was.
fn backdate(cache: &Path, when: &str) {
    let raw = std::fs::read_to_string(cache).expect("a cache");
    let mut value: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    value["captured_at"] = serde_json::Value::String(when.to_string());
    std::fs::write(cache, value.to_string()).expect("rewrite the cache");
}

// `enable` and `disable` — the step that used to be a line to copy by hand.
//
// Every one of these writes to a file under the test's own directory via
// `--file`. Nothing here may touch the startup file of whoever is running the
// suite, which is exactly what the flag is for.

/// The startup file this test is allowed to write.
fn rc(env: &TestEnv) -> std::path::PathBuf {
    env.root().join("startup")
}

fn setup(env: &TestEnv, action: &str, shell: &str) -> String {
    let out = env
        .grit()
        .args(["shell", action, shell, "--file"])
        .arg(rc(env))
        .output()
        .expect("run grit shell");
    assert!(
        out.status.success(),
        "grit shell {action} failed: {}",
        common::describe(&out)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn enabling_writes_a_line_that_loads_the_integration() {
    let env = TestEnv::new();
    std::fs::write(rc(&env), "setopt nomatch\n").unwrap();

    let said = setup(&env, "enable", "zsh");
    let contents = std::fs::read_to_string(rc(&env)).unwrap();

    assert!(said.contains("added"), "{said}");
    assert!(
        contents.contains(r#"eval "$(grit shell init zsh)""#),
        "{contents}"
    );
    assert!(contents.starts_with("setopt nomatch\n"), "{contents}");
}

#[test]
fn what_it_writes_is_valid_shell() {
    // The point of writing it rather than printing it: nobody proofreads a
    // startup file, so a stray quote here is a broken login shell.
    if !available("zsh") {
        eprintln!("skipping: zsh is not installed");
        return;
    }
    let env = TestEnv::new();
    setup(&env, "enable", "zsh");

    let out = Command::new("zsh")
        .arg("-n")
        .arg(rc(&env))
        .output()
        .expect("run zsh -n");
    assert!(
        out.status.success(),
        "zsh -n rejected it: {}",
        common::describe(&out)
    );
}

#[test]
fn enabling_twice_leaves_one_copy() {
    let env = TestEnv::new();
    setup(&env, "enable", "zsh");
    let once = std::fs::read_to_string(rc(&env)).unwrap();

    let said = setup(&env, "enable", "zsh");

    assert!(said.contains("already"), "{said}");
    assert_eq!(std::fs::read_to_string(rc(&env)).unwrap(), once);
}

#[test]
fn disabling_gives_back_the_file_it_started_as() {
    let env = TestEnv::new();
    let original = "export PATH=/usr/bin\nsetopt nomatch\n";
    std::fs::write(rc(&env), original).unwrap();

    setup(&env, "enable", "zsh");
    setup(&env, "disable", "zsh");

    assert_eq!(std::fs::read_to_string(rc(&env)).unwrap(), original);
}

#[test]
fn disabling_something_never_enabled_says_so_and_writes_nothing() {
    let env = TestEnv::new();
    std::fs::write(rc(&env), "setopt nomatch\n").unwrap();

    let said = setup(&env, "disable", "zsh");

    assert!(said.contains("not in"), "{said}");
    assert_eq!(
        std::fs::read_to_string(rc(&env)).unwrap(),
        "setopt nomatch\n"
    );
}

#[test]
fn a_line_grit_did_not_write_is_reported_rather_than_edited() {
    let env = TestEnv::new();
    let by_hand = "eval \"$(grit shell init zsh)\"\n";
    std::fs::write(rc(&env), by_hand).unwrap();

    let enabled = setup(&env, "enable", "zsh");
    assert!(enabled.contains("did not write"), "{enabled}");
    assert_eq!(std::fs::read_to_string(rc(&env)).unwrap(), by_hand);

    let disabled = setup(&env, "disable", "zsh");
    assert!(disabled.contains("did not write"), "{disabled}");
    assert_eq!(std::fs::read_to_string(rc(&env)).unwrap(), by_hand);
}

#[test]
fn a_startup_file_that_does_not_exist_yet_is_created() {
    let env = TestEnv::new();
    let nested = env.root().join("nested").join("deeper").join("config.fish");

    env.grit()
        .args(["shell", "enable", "fish", "--file"])
        .arg(&nested)
        .assert()
        .success();

    let contents = std::fs::read_to_string(&nested).unwrap();
    // fish has no `eval "$(...)"`.
    assert!(
        contents.contains("grit shell init fish | source"),
        "{contents}"
    );
}

#[test]
fn the_shell_is_taken_from_the_environment_when_it_is_not_given() {
    let env = TestEnv::new();

    env.grit()
        .env("SHELL", "/opt/homebrew/bin/fish")
        .args(["shell", "enable", "--file"])
        .arg(rc(&env))
        .assert()
        .success()
        .stdout(predicate::str::contains("fish"));
}

#[test]
fn a_shell_grit_has_no_script_for_is_refused_rather_than_guessed() {
    // Writing zsh's script into a nushell config would break every prompt.
    let env = TestEnv::new();

    env.grit()
        .env("SHELL", "/usr/bin/nushell")
        .args(["shell", "enable", "--file"])
        .arg(rc(&env))
        .assert()
        .failure()
        .stderr(predicate::str::contains("which shell"));

    assert!(!rc(&env).exists(), "it wrote a file anyway");
}
