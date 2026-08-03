//! `grit status` — the dashboard.
//!
//! Reading a repo means running git, so repos are read concurrently: total time
//! is the slowest repo rather than the sum. Nothing is shared mutably between
//! threads, so a scoped spawn is all the machinery needed — no channels, no
//! locks, no async runtime.

use anyhow::Result;
use serde::Serialize;

use crate::cli::StatusArgs;
use crate::context::Ctx;
use crate::registry::{Repo, abbreviate_home};
use crate::render::{Align, Cell, Table, Theme, footer, plural, symbol};
use crate::vcs::{self, RepoState, Snapshot};

/// One repo's reading, or why it could not be read.
struct Row {
    repo: Repo,
    outcome: Result<Snapshot, String>,
}

pub fn run(args: &StatusArgs, ctx: &mut Ctx) -> Result<i32> {
    let repos = ctx.registry.select(&args.aliases, args.tag.as_deref())?;

    if repos.is_empty() {
        if args.json {
            println!("[]");
        } else {
            super::show::print_empty_hint(ctx);
        }
        return Ok(0);
    }

    let rows = collect(repos);

    if args.json {
        print_json(&rows)?;
    } else {
        print_table(&rows, ctx);
    }

    // A repo we could not read at all is a real failure; a repo whose directory
    // is simply gone is information, and shows up as a `missing` row.
    let failed = rows.iter().filter(|r| r.outcome.is_err()).count();
    Ok(if failed > 0 { 1 } else { 0 })
}

/// Snapshot every repo in parallel, returning rows in the input order.
fn collect(repos: Vec<Repo>) -> Vec<Row> {
    let snapshots: Vec<Result<Snapshot, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = repos
            .iter()
            .map(|repo| {
                scope.spawn(move || {
                    vcs::provider_for(repo.kind())
                        .snapshot(repo.path())
                        .map_err(|e| first_line(&e.to_string()))
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("panicked while reading".into()))
            })
            .collect()
    });

    repos
        .into_iter()
        .zip(snapshots)
        .map(|(repo, outcome)| Row { repo, outcome })
        .collect()
}

/// Errors are multi-line (they carry hints); a table cell gets the first line.
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

fn print_table(rows: &[Row], ctx: &Ctx) {
    let mut table = Table::new([
        "alias", "branch", "sync", "state", "commit", "subject", "age",
    ])
    .color(ctx.color)
    .terminal_width(ctx.width)
    .flex(5)
    .align(6, Align::Right);

    for row in rows {
        table.push_row(match &row.outcome {
            Ok(snap) => snapshot_cells(&row.repo, snap),
            Err(msg) => error_cells(&row.repo, msg),
        });
    }

    print!("{}", table.render());
    println!();
    print!("{}", footer(&summarise(rows), ctx.color));
}

fn snapshot_cells(repo: &Repo, snap: &Snapshot) -> Vec<Cell> {
    if snap.state == RepoState::Missing {
        return vec![
            Cell::styled(&repo.alias, Theme::alias()),
            Cell::styled(symbol::NONE, Theme::muted()),
            Cell::empty(),
            Cell::styled("missing", Theme::warn()),
            Cell::empty(),
            Cell::styled(abbreviate_home(repo.path()), Theme::muted()),
            Cell::empty(),
        ];
    }

    let (commit, subject, age) = match &snap.head {
        Some(head) => (
            Cell::styled(&head.short_id, Theme::sha()),
            Cell::styled(&head.subject, Theme::subject()),
            Cell::styled(&head.age, Theme::muted()),
        ),
        None => (
            Cell::styled(symbol::NONE, Theme::muted()),
            Cell::styled("no commits yet", Theme::muted()),
            Cell::empty(),
        ),
    };

    vec![
        Cell::styled(&repo.alias, Theme::alias()),
        branch_cell(snap),
        sync_cell(snap),
        state_cell(snap),
        commit,
        subject,
        age,
    ]
}

fn error_cells(repo: &Repo, message: &str) -> Vec<Cell> {
    vec![
        Cell::styled(&repo.alias, Theme::alias()),
        Cell::styled(symbol::NONE, Theme::muted()),
        Cell::empty(),
        Cell::styled("error", Theme::warn()),
        Cell::empty(),
        Cell::styled(message, Theme::warn()),
        Cell::empty(),
    ]
}

fn branch_cell(snap: &Snapshot) -> Cell {
    match &snap.branch {
        Some(branch) => Cell::styled(branch, Theme::branch()),
        None => Cell::styled("detached", Theme::detached()),
    }
}

/// `✓` in sync, `↑n` / `↓n` otherwise, `·` when there is no upstream to compare
/// against — the three cases are genuinely different and worth distinguishing.
fn sync_cell(snap: &Snapshot) -> Cell {
    if snap.upstream.is_none() {
        return Cell::styled(symbol::NONE, Theme::muted());
    }
    if snap.is_synced() {
        return Cell::styled(symbol::SYNCED, Theme::ok());
    }

    let mut cell = Cell::empty();
    if snap.ahead > 0 {
        cell = cell.push(format!("{}{}", symbol::AHEAD, snap.ahead), Theme::ahead());
    }
    if snap.behind > 0 {
        cell = cell.push_spaced(
            format!("{}{}", symbol::BEHIND, snap.behind),
            Theme::behind(),
        );
    }
    cell
}

fn state_cell(snap: &Snapshot) -> Cell {
    let mut cell = Cell::empty();

    if let Some(label) = snap.state.label() {
        cell = cell.push(label, Theme::warn());
    }
    if snap.conflicts > 0 {
        cell = cell.push_spaced(
            format!("{}{}", symbol::CONFLICT, snap.conflicts),
            Theme::conflict(),
        );
    }
    if snap.staged > 0 {
        cell = cell.push_spaced(
            format!("{}{}", symbol::STAGED, snap.staged),
            Theme::staged(),
        );
    }
    if snap.unstaged > 0 {
        cell = cell.push_spaced(
            format!("{}{}", symbol::UNSTAGED, snap.unstaged),
            Theme::unstaged(),
        );
    }
    if snap.untracked > 0 {
        cell = cell.push_spaced(
            format!("{}{}", symbol::UNTRACKED, snap.untracked),
            Theme::untracked(),
        );
    }
    if snap.stashes > 0 {
        cell = cell.push_spaced(format!("{}{}", symbol::STASH, snap.stashes), Theme::stash());
    }

    if cell.is_empty() {
        return Cell::styled("clean", Theme::muted());
    }
    cell
}

/// The bullet-separated line under the table. Only non-zero facts appear, so a
/// quiet day reads `4 repos · all clean`.
fn summarise(rows: &[Row]) -> Vec<String> {
    let snaps: Vec<&Snapshot> = rows
        .iter()
        .filter_map(|r| r.outcome.as_ref().ok())
        .collect();

    let count = |f: &dyn Fn(&Snapshot) -> bool| snaps.iter().filter(|s| f(s)).count();

    let live: Vec<&&Snapshot> = snaps
        .iter()
        .filter(|s| s.state != RepoState::Missing)
        .collect();
    let dirty = live.iter().filter(|s| !s.is_clean()).count();
    let ahead = count(&|s| s.ahead > 0);
    let behind = count(&|s| s.behind > 0);
    let stashed = count(&|s| s.stashes > 0);
    let missing = count(&|s| s.state == RepoState::Missing);
    let failed = rows.iter().filter(|r| r.outcome.is_err()).count();

    let mut parts = vec![plural(rows.len(), "repo", "repos")];

    if dirty > 0 {
        parts.push(format!("{dirty} dirty"));
    }
    if ahead > 0 {
        parts.push(format!("{ahead} ahead"));
    }
    if behind > 0 {
        parts.push(format!("{behind} behind"));
    }
    if stashed > 0 {
        parts.push(plural(stashed, "stash", "stashes"));
    }
    if missing > 0 {
        parts.push(format!("{missing} missing"));
    }
    if failed > 0 {
        parts.push(format!("{failed} unreadable"));
    }
    if parts.len() == 1 && !live.is_empty() {
        parts.push("all clean".to_string());
    }

    parts
}

#[derive(Serialize)]
struct StatusJson<'a> {
    alias: &'a str,
    path: &'a std::path::Path,
    kind: crate::registry::VcsKind,
    tags: &'a [String],
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    snapshot: Option<&'a Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn print_json(rows: &[Row]) -> Result<()> {
    let payload: Vec<StatusJson<'_>> = rows
        .iter()
        .map(|row| StatusJson {
            alias: &row.repo.alias,
            path: row.repo.path(),
            kind: row.repo.kind(),
            tags: &row.repo.entry.tags,
            snapshot: row.outcome.as_ref().ok(),
            error: row.outcome.as_ref().err().map(String::as_str),
        })
        .collect();

    println!("{}", super::to_json(&payload)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{RepoEntry, VcsKind};
    use crate::vcs::Commit;
    use std::path::PathBuf;

    fn repo(alias: &str) -> Repo {
        Repo {
            alias: alias.to_string(),
            entry: RepoEntry {
                path: PathBuf::from(format!("/code/{alias}")),
                kind: VcsKind::Git,
                tags: vec![],
                added_at: None,
            },
        }
    }

    fn row(alias: &str, snap: Snapshot) -> Row {
        Row {
            repo: repo(alias),
            outcome: Ok(snap),
        }
    }

    fn clean() -> Snapshot {
        Snapshot {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            head: Some(Commit {
                short_id: "abc1234".into(),
                subject: "do a thing".into(),
                age: "3h".into(),
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn a_clean_synced_repo_shows_a_tick_and_clean() {
        let snap = clean();
        assert_eq!(sync_cell(&snap).text(), symbol::SYNCED);
        assert_eq!(state_cell(&snap).text(), "clean");
    }

    #[test]
    fn no_upstream_is_not_the_same_as_in_sync() {
        let snap = Snapshot {
            upstream: None,
            ..clean()
        };
        assert_eq!(sync_cell(&snap).text(), symbol::NONE);
    }

    #[test]
    fn ahead_and_behind_are_shown_together() {
        let snap = Snapshot {
            ahead: 2,
            behind: 5,
            ..clean()
        };
        assert_eq!(sync_cell(&snap).text(), "↑2 ↓5");
    }

    #[test]
    fn working_tree_counts_are_ordered_most_to_least_urgent() {
        let snap = Snapshot {
            staged: 3,
            unstaged: 1,
            untracked: 2,
            conflicts: 1,
            stashes: 4,
            ..clean()
        };
        assert_eq!(state_cell(&snap).text(), "!1 ●3 ○1 ?2 ⚑4");
    }

    #[test]
    fn a_stash_alone_still_counts_as_something_to_show() {
        let snap = Snapshot {
            stashes: 1,
            ..clean()
        };
        assert_eq!(state_cell(&snap).text(), "⚑1");
    }

    #[test]
    fn an_in_progress_rebase_is_labelled() {
        let snap = Snapshot {
            state: RepoState::Rebasing,
            conflicts: 2,
            ..clean()
        };
        assert_eq!(state_cell(&snap).text(), "rebasing !2");
    }

    #[test]
    fn a_detached_head_says_so() {
        let snap = Snapshot {
            branch: None,
            ..clean()
        };
        assert_eq!(branch_cell(&snap).text(), "detached");
    }

    #[test]
    fn a_missing_repo_renders_a_row_rather_than_failing() {
        let snap = Snapshot {
            state: RepoState::Missing,
            ..Snapshot::default()
        };
        let cells = snapshot_cells(&repo("gone"), &snap);
        assert_eq!(cells[3].text(), "missing");
    }

    #[test]
    fn a_repo_with_no_commits_is_described_not_blank() {
        let snap = Snapshot {
            branch: Some("main".into()),
            head: None,
            ..Snapshot::default()
        };
        let cells = snapshot_cells(&repo("fresh"), &snap);
        assert_eq!(cells[5].text(), "no commits yet");
    }

    #[test]
    fn a_quiet_summary_says_all_clean() {
        let rows = vec![row("a", clean()), row("b", clean())];
        assert_eq!(summarise(&rows), ["2 repos", "all clean"]);
    }

    #[test]
    fn the_summary_counts_each_condition_once_per_repo() {
        let rows = vec![
            row("a", clean()),
            row(
                "b",
                Snapshot {
                    staged: 3,
                    unstaged: 1,
                    ahead: 1,
                    ..clean()
                },
            ),
            row(
                "c",
                Snapshot {
                    stashes: 1,
                    behind: 2,
                    ..clean()
                },
            ),
        ];
        assert_eq!(
            summarise(&rows),
            ["3 repos", "1 dirty", "1 ahead", "1 behind", "1 stash"]
        );
    }

    #[test]
    fn unreadable_repos_are_reported_in_the_summary() {
        let rows = vec![
            row("a", clean()),
            Row {
                repo: repo("b"),
                outcome: Err("git blew up".into()),
            },
        ];
        assert!(summarise(&rows).contains(&"1 unreadable".to_string()));
    }

    #[test]
    fn only_the_first_line_of_an_error_reaches_the_table() {
        assert_eq!(first_line("boom\nhint: try again"), "boom");
    }
}
