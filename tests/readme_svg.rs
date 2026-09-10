//! The README's terminal examples, rendered to the SVGs it shows.
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

use grit::commands::detail::{Block, build_card};
use grit::commands::status::{Row, build_table, summarise};
use grit::context::Ctx;
use grit::registry::{Registry, Repo, RepoEntry, VcsKind};
use grit::render::svg::{DARK, LIGHT, Palette, Screen};
use grit::render::{Theme, footer};
use grit::vcs::{
    Branch, Branches, Change, Commit, Detail, FileChange, RepoState, Snapshot, Stash, Tracking,
};

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
    let ctx = ctx();
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

/// The margin `Table` indents by, which the card's loose lines share.
const MARGIN: &str = "  ";

/// The repo the detail card and the picker are both about.
///
/// One repo with one of everything, so the card shows every section and the
/// two columns of `CHANGES` show both sides.
fn detailed() -> (Repo, Detail) {
    let mut repo = repo("api");
    repo.entry.tags = vec!["release".to_string()];

    let snapshot = Snapshot {
        ahead: 2,
        staged: 2,
        unstaged: 1,
        untracked: 1,
        ..snapshot(
            "feature/rate-limits",
            "15beeba",
            "add rate limit headers",
            "20m",
        )
    };

    let file = |path: &str, staged, unstaged| FileChange {
        path: path.to_string(),
        origin: None,
        staged,
        unstaged,
    };

    let detail = Detail {
        files: vec![
            file("src/limits.rs", Some(Change::Modified), None),
            file("src/lib.rs", None, Some(Change::Modified)),
            file("src/headers.rs", Some(Change::Added), None),
            file("notes.md", None, Some(Change::Untracked)),
        ],
        commits: vec![
            Commit {
                short_id: "15beeba".into(),
                subject: "add rate limit headers".into(),
                age: "20m".into(),
            },
            Commit {
                short_id: "8c31f0d".into(),
                subject: "pull the window size out of the config".into(),
                age: "2h".into(),
            },
        ],
        stashes: vec![Stash {
            id: "stash@{0}".into(),
            message: "On main: half a migration".into(),
            age: "4h".into(),
        }],
        branches: Branches::Listed(vec![
            Branch {
                name: "feature/rate-limits".into(),
                is_head: true,
                tracking: Tracking::Tracked {
                    upstream: "origin/feature/rate-limits".into(),
                    ahead: 2,
                    behind: 0,
                },
                head: Some(Commit {
                    short_id: "15beeba".into(),
                    subject: "add rate limit headers".into(),
                    age: "20m".into(),
                }),
            },
            Branch {
                name: "main".into(),
                is_head: false,
                tracking: Tracking::Tracked {
                    upstream: "origin/main".into(),
                    ahead: 0,
                    behind: 0,
                },
                head: Some(Commit {
                    short_id: "57a90bc".into(),
                    subject: "bump chart library to 4.2".into(),
                    age: "2d".into(),
                }),
            },
        ]),
        snapshot,
    };

    (repo, detail)
}

/// `grit detail api`: the card, laid out by the same blocks the terminal gets.
fn detail_screen() -> Screen {
    let ctx = ctx();
    let (repo, detail) = detailed();

    let mut screen = Screen::new();
    screen.line("$ grit detail api", Theme::muted());

    for block in build_card(&repo, &detail, &ctx) {
        match block {
            Block::Blank => screen.blank(),
            Block::Line(cell) => screen.cell(MARGIN, &cell),
            Block::Section(title, table) => {
                screen.cell(
                    MARGIN,
                    &grit::render::Cell::styled(title.to_uppercase(), Theme::header()),
                );
                screen.table(&table);
            }
        }
    }

    screen
}

/// The picker `^G^G` opens: fzf's chrome around grit's own rows.
///
/// The rows are the real thing — `grit shell rows` renders exactly this table,
/// headerless and flush left, and that is what fzf is handed on stdin. Only
/// the prompt, the counter and the key hints are fzf's, and they are the parts
/// grit does not draw.
fn picker_screen() -> Screen {
    let ctx = ctx();
    let rows = rows();

    let mut screen = Screen::new();
    screen.line(
        "  enter: run git here · ctrl-d: cd · ctrl-r: refresh",
        Theme::muted(),
    );
    screen.line("  4/4", Theme::muted());
    screen.blank();
    screen.table(&build_table(&rows, &ctx).no_header().no_rule().no_indent());
    screen.blank();
    screen.line("> api", Theme::banner());
    screen
}

fn ctx() -> Ctx {
    let registry = Registry::load_from(PathBuf::from("/nonexistent/grit-readme.toml"))
        .expect("a missing registry file is an empty registry");
    Ctx::with_registry(registry, false, None)
}

fn asset(name: &str, palette: &Palette) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(format!("{name}-{}.svg", palette.name))
}

/// Every example the README shows, by the name its files carry.
fn examples() -> Vec<(&'static str, Screen)> {
    vec![
        ("status", screen()),
        ("detail", detail_screen()),
        ("picker", picker_screen()),
    ]
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
fn the_card_shows_every_section_and_both_sides_of_a_change() {
    // The prose under the image explains the two code columns and says the
    // sections are left out when empty. This is the example it describes.
    let text = detail_screen().text().to_string();

    for section in ["CHANGES", "COMMITS", "BRANCHES", "STASHES"] {
        assert!(text.contains(section), "no {section} in:\n{text}");
    }
    for row in ["M   src/limits.rs", " M  src/lib.rs", " ?  notes.md"] {
        assert!(text.contains(row), "missing row:\n{row}\nin:\n{text}");
    }
    assert!(text.contains("4 changes · 2 branches · 1 stash"), "{text}");
}

#[test]
fn the_picker_lists_the_repos_flush_left() {
    // What `grit shell rows` emits, and why: fzf reads field one as the alias,
    // and the table's usual margin would make that field empty.
    let text = picker_screen().text().to_string();
    for line in text.lines() {
        assert!(
            !line.starts_with("  api") && !line.starts_with("  webapp"),
            "a repo row is indented, which would cost fzf its first field:\n{line}"
        );
    }
    assert!(text.contains("api "), "{text}");
    assert!(text.contains("ctrl-d: cd"), "{text}");
}

#[test]
fn committed_svgs_match_a_fresh_render() {
    for (name, screen) in examples() {
        for palette in [&LIGHT, &DARK] {
            let path = asset(name, palette);
            let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "{} is missing ({e}); regenerate with \
                     `cargo test --test readme_svg -- --ignored`",
                    path.display()
                )
            });

            assert_eq!(
                committed,
                screen.to_svg(palette),
                "{} is stale — an example or the theme changed since it was \
                 generated. Regenerate with `cargo test --test readme_svg -- --ignored`",
                path.display()
            );
        }
    }
}

#[test]
#[ignore = "writes into assets/; run with --ignored to regenerate"]
fn regenerate_the_readme_svgs() {
    for (name, screen) in examples() {
        for palette in [&LIGHT, &DARK] {
            let path = asset(name, palette);
            std::fs::write(&path, screen.to_svg(palette))
                .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
            println!("wrote {}", path.display());
        }
    }
}
