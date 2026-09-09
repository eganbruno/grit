//! The README's headline dashboard, rendered to the SVGs the README shows.
//!
//! GitHub cannot colour a fenced code block, so the example that exists to show
//! that colour carries meaning has to be an image. An image is a copy, and a
//! copy drifts — so it is generated here, from the same `build_table` the
//! terminal uses and the same `Theme` it paints with, rather than drawn by
//! hand.
//!
//! Two tests hold it together:
//!
//! - `committed_svgs_match_a_fresh_render` fails when the files on disk are
//!   stale, which is what makes a repaint of `Theme` a build failure rather
//!   than a README that quietly lies.
//! - `regenerate_the_readme_svgs` is the fix, and is `#[ignore]`d because it
//!   writes into the working tree:
//!
//!   ```text
//!   cargo test --test readme_svg -- --ignored
//!   ```
//!
//! The rows are invented rather than read from real repositories on purpose:
//! the point is a representative dashboard, and real repos would mean random
//! SHAs and ages that grow every day, which is neither reproducible nor
//! illustrative. Everything about how those rows are *rendered* is real.

use std::path::{Path, PathBuf};

use grit::commands::status::{Row, build_table, summarise};
use grit::context::Ctx;
use grit::registry::{Registry, Repo, RepoEntry, VcsKind};
use grit::render::svg::{DARK, LIGHT, Palette, Screen};
use grit::render::{Theme, footer};
use grit::vcs::{Commit, RepoState, Snapshot};

/// The command line shown above the table.
const PROMPT: &str = "$ grit status";

fn repo(alias: &str) -> Repo {
    Repo {
        alias: alias.to_string(),
        entry: RepoEntry {
            path: PathBuf::from(format!("/repos/{alias}")),
            kind: VcsKind::Git,
            tags: Vec::new(),
            added_at: None,
        },
    }
}

fn snapshot(branch: &str, short_id: &str, subject: &str, age: &str) -> Snapshot {
    Snapshot {
        branch: Some(branch.to_string()),
        upstream: Some(format!("origin/{branch}")),
        ahead: 0,
        behind: 0,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        conflicts: 0,
        stashes: 0,
        head: Some(Commit {
            short_id: short_id.to_string(),
            subject: subject.to_string(),
            age: age.to_string(),
        }),
        state: RepoState::Normal,
    }
}

/// Four repos that between them exercise every colour the two middle columns
/// can produce: ahead and behind, staged, unstaged, untracked, a stash, and a
/// clean row to have something to compare against.
fn rows() -> Vec<Row> {
    let api = Snapshot {
        ahead: 2,
        staged: 3,
        unstaged: 1,
        ..snapshot(
            "feature/rate-limits",
            "15beeba",
            "add rate limit headers",
            "20m",
        )
    };

    let dashboard = snapshot("main", "57a90bc", "bump chart library to 4.2", "2d");

    let docs = Snapshot {
        behind: 1,
        ..snapshot("main", "430125f", "document the webhooks endpoint", "3d")
    };

    let webapp = Snapshot {
        unstaged: 2,
        untracked: 1,
        stashes: 1,
        ..snapshot(
            "fix/login-redirect",
            "a36d467",
            "restore scroll position on back",
            "6h",
        )
    };

    [
        ("api", api),
        ("dashboard", dashboard),
        ("docs", docs),
        ("webapp", webapp),
    ]
    .into_iter()
    .map(|(alias, snap)| Row {
        repo: repo(alias),
        outcome: Ok(snap),
    })
    .collect()
}

/// The whole example: prompt, table, footer — as one screen of styled text.
///
/// Width is left unset so nothing truncates; the image has no terminal to fit
/// inside, and a `…` in the README would be an artefact of this harness rather
/// than something grit did.
fn screen() -> Screen {
    let registry = Registry::load_from(PathBuf::from("/nonexistent/grit-readme.toml"))
        .expect("a missing registry file is an empty registry");
    let ctx = Ctx::with_registry(registry, false, None);

    let rows = rows();
    let mut screen = Screen::new();
    screen.line(PROMPT, Theme::muted());
    // Not what a terminal does — grit prints the table straight after the
    // command. The gap is for the reader: it separates what was typed from
    // what came back, which a screenshot has to do on its own.
    screen.blank();
    screen.table(&build_table(&rows, &ctx));
    screen.blank();
    screen.line(
        footer(&summarise(&rows), false).trim_end_matches('\n'),
        Theme::muted(),
    );
    screen
}

fn asset(palette: &Palette) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(format!("status-{}.svg", palette.name))
}

#[test]
fn the_example_reads_the_way_the_readme_describes_it() {
    // The prose under the image says what each row means. If a row stops
    // saying it, the prose is wrong and no palette test would notice.
    let screen = screen();
    let text = screen.text();

    assert!(text.contains(PROMPT));
    for row in [
        "api        feature/rate-limits  ↑2    ●3 ○1",
        "dashboard  main                 ✓     clean",
        "docs       main                 ↓1    clean",
        "webapp     fix/login-redirect   ✓     ○2 ?1 ⚑1",
    ] {
        assert!(text.contains(row), "missing row:\n{row}\nin:\n{text}");
    }
    assert!(
        text.contains("4 repos · 2 dirty · 1 ahead · 1 behind · 1 stash"),
        "{text}"
    );
}

#[test]
fn the_sync_and_state_columns_are_actually_painted() {
    // The reason the image exists at all. Each of these is a different
    // meaning in the two middle columns, and each must reach the SVG as its
    // own coloured run.
    let svg = screen().to_svg(&DARK);
    for (symbol, class) in [
        ("↑2", "c-yellow"),  // ahead
        ("↓1", "c-red"),     // behind
        ("✓", "c-green"),    // in sync
        ("●3", "c-green"),   // staged
        ("○1", "c-yellow"),  // unstaged
        ("?1", "c-blue"),    // untracked
        ("⚑1", "c-magenta"), // stashed
    ] {
        let run = format!("class=\"{class}\">{symbol}</tspan>");
        assert!(svg.contains(&run), "expected {run} in\n{svg}");
    }
}

#[test]
fn committed_svgs_match_a_fresh_render() {
    for palette in [&LIGHT, &DARK] {
        let path = asset(palette);
        let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{} is missing ({e}); regenerate with \
                 `cargo test --test readme_svg -- --ignored`",
                path.display()
            )
        });

        assert_eq!(
            committed,
            screen().to_svg(palette),
            "{} is stale — the dashboard or the theme changed since it was \
             generated. Regenerate with `cargo test --test readme_svg -- --ignored`",
            path.display()
        );
    }
}

#[test]
#[ignore = "writes into assets/; run with --ignored to regenerate"]
fn regenerate_the_readme_svgs() {
    for palette in [&LIGHT, &DARK] {
        let path = asset(palette);
        std::fs::write(&path, screen().to_svg(palette))
            .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
        println!("wrote {}", path.display());
    }
}
