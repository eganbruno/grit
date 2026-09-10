//! `grit branch` — every branch of every repo, one line each.
//!
//! The passthrough answer to this question — `grit @tag branch -vv` — is at the
//! mercy of what git and dolt choose to print, and both of them print a commit
//! *message* rather than its subject. One merge commit with a rationale in its
//! body turns a fifteen-branch listing into pages of prose, and grit cannot cut
//! it down: the child writes to the terminal directly, on inherited stdio, so
//! there is nothing to intercept. (Piping it in order to trim it would cost the
//! user's pager, colour detection and `$EDITOR` — see `vcs::git::exec` — and
//! would not even work, because the bodies arrive as extra *lines*, not as long
//! ones.)
//!
//! So the listing is grit's own: read through `trait Vcs`, laid out by the same
//! `Table` the dashboard uses, and cut to one line per branch at the point
//! where the data is still structured.
//!
//! Repos are read concurrently for the same reason `status` reads them
//! concurrently: total time is the slowest repo rather than the sum.

use anyhow::Result;
use serde::Serialize;

use crate::cli::BranchArgs;
use crate::context::Ctx;
use crate::registry::Repo;
use crate::render::{Align, Cell, Table, Theme, footer, plural, symbol};
use crate::vcs::{self, Branch, Branches, Tracking};

/// How much of a commit subject a branch listing shows.
///
/// The same reasoning as the dashboard's cap, and deliberately the same number:
/// two tables that answer the same question about the same commits should not
/// disagree about how much of it fits.
const SUBJECT_WIDTH: usize = crate::commands::status::SUBJECT_WIDTH;

/// One repo's branches, or why they could not be read.
struct Row {
    repo: Repo,
    outcome: Result<Branches, String>,
}

pub fn run(args: &BranchArgs, ctx: &mut Ctx) -> Result<i32> {
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

    let failed = rows.iter().filter(|r| r.outcome.is_err()).count();
    Ok(if failed > 0 { 1 } else { 0 })
}

fn collect(repos: Vec<Repo>) -> Vec<Row> {
    let listings: Vec<Result<Branches, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = repos
            .iter()
            .map(|repo| {
                scope.spawn(move || {
                    vcs::provider_for(repo.kind())
                        .branches(repo.path())
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
        .zip(listings)
        .map(|(repo, outcome)| Row { repo, outcome })
        .collect()
}

/// Errors are multi-line (they carry hints); a table cell gets the first line.
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

fn print_table(rows: &[Row], ctx: &Ctx) {
    // The `*` column has no header: the marker is git's, and a word above it
    // would be wider than the thing it labels.
    let mut table = Table::new(["alias", "", "branch", "sync", "commit", "subject", "age"])
        .color(ctx.color)
        .terminal_width(ctx.width)
        .flex(5)
        .max_width(5, SUBJECT_WIDTH)
        .align(6, Align::Right);

    for row in rows {
        match &row.outcome {
            Ok(Branches::Missing) => {
                table.push_row(note_cells(&row.repo, "missing", Theme::warn()))
            }
            Ok(Branches::Listed(branches)) if branches.is_empty() => {
                table.push_row(note_cells(&row.repo, "no branches yet", Theme::muted()))
            }
            Ok(Branches::Listed(branches)) => {
                for branch in branches {
                    table.push_row(branch_cells(&row.repo, branch));
                }
            }
            Err(msg) => table.push_row(note_cells(&row.repo, msg, Theme::warn())),
        }
    }

    print!("{}", table.render());
    println!();
    print!("{}", footer(&summarise(rows), ctx.color));
}

/// The alias is printed on every row rather than only on the first of a repo's
/// block. A blank cell reads as "same as above" only while the rows are in the
/// order they were printed — sort the output through anything else, or grep a
/// line out of it, and an unlabelled row belongs to nobody.
fn branch_cells(repo: &Repo, branch: &Branch) -> Vec<Cell> {
    let (commit, subject, age) = match &branch.head {
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
        if branch.is_head {
            Cell::styled("*", Theme::ok())
        } else {
            Cell::empty()
        },
        Cell::styled(&branch.name, Theme::branch()),
        sync_cell(&branch.tracking),
        commit,
        subject,
        age,
    ]
}

fn note_cells(repo: &Repo, message: &str, style: owo_colors::Style) -> Vec<Cell> {
    vec![
        Cell::styled(&repo.alias, Theme::alias()),
        Cell::empty(),
        Cell::styled(symbol::NONE, Theme::muted()),
        Cell::empty(),
        Cell::empty(),
        Cell::styled(message, style),
        Cell::empty(),
    ]
}

/// `✓` level with the upstream, `↑n` / `↓n` apart from it, `·` when there is no
/// upstream, `gone` when there was one and it has been deleted — and empty when
/// the backend did not measure the distance, which is dolt on every branch but
/// the one checked out.
///
/// Empty rather than a symbol of its own: "not measured" is the absence of a
/// reading, and inventing a glyph for it would put something on screen that
/// looks like a finding.
pub(crate) fn sync_cell(tracking: &Tracking) -> Cell {
    match tracking {
        Tracking::Untracked => Cell::styled(symbol::NONE, Theme::muted()),
        Tracking::Gone { .. } => Cell::styled("gone", Theme::warn()),
        Tracking::Unmeasured { .. } => Cell::empty(),
        Tracking::Tracked { ahead, behind, .. } => {
            if *ahead == 0 && *behind == 0 {
                return Cell::styled(symbol::SYNCED, Theme::ok());
            }
            let mut cell = Cell::empty();
            if *ahead > 0 {
                cell = cell.push(format!("{}{}", symbol::AHEAD, ahead), Theme::ahead());
            }
            if *behind > 0 {
                cell = cell.push_spaced(format!("{}{}", symbol::BEHIND, behind), Theme::behind());
            }
            cell
        }
    }
}

fn summarise(rows: &[Row]) -> Vec<String> {
    let branches: usize = rows
        .iter()
        .filter_map(|r| r.outcome.as_ref().ok())
        .map(|b| b.as_slice().len())
        .sum();

    let mut parts = vec![
        plural(branches, "branch", "branches"),
        format!("across {}", plural(rows.len(), "repo", "repos")),
    ];

    let missing = rows
        .iter()
        .filter(|r| matches!(r.outcome, Ok(Branches::Missing)))
        .count();
    if missing > 0 {
        parts.push(format!("{missing} missing"));
    }

    let unpushed = rows
        .iter()
        .filter_map(|r| r.outcome.as_ref().ok())
        .flat_map(Branches::as_slice)
        .filter(|b| matches!(&b.tracking, Tracking::Tracked { ahead, .. } if *ahead > 0))
        .count();
    if unpushed > 0 {
        parts.push(format!("{unpushed} ahead"));
    }

    let failed = rows.iter().filter(|r| r.outcome.is_err()).count();
    if failed > 0 {
        parts.push(format!("{failed} unreadable"));
    }

    parts
}

#[derive(Serialize)]
struct JsonRepo<'a> {
    alias: &'a str,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    branches: Option<&'a [Branch]>,
    /// Only present, and only true, when the registered path is gone.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    missing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

/// An array, matching `grit status --json`. `grit show --json` answers with an
/// object because it has registry-wide fields to carry; this has none.
fn print_json(rows: &[Row]) -> Result<()> {
    let repos: Vec<JsonRepo> = rows
        .iter()
        .map(|row| JsonRepo {
            alias: &row.repo.alias,
            kind: row.repo.kind().as_str(),
            branches: row.outcome.as_ref().ok().map(Branches::as_slice),
            missing: matches!(row.outcome, Ok(Branches::Missing)),
            error: row.outcome.as_ref().err().map(String::as_str),
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&repos)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::Tracking;

    fn tracked(ahead: u32, behind: u32) -> Tracking {
        Tracking::Tracked {
            upstream: "origin/main".to_string(),
            ahead,
            behind,
        }
    }

    #[test]
    fn a_branch_level_with_its_upstream_gets_a_tick() {
        assert_eq!(sync_cell(&tracked(0, 0)).text(), symbol::SYNCED);
    }

    #[test]
    fn a_diverged_branch_counts_both_ways() {
        assert_eq!(sync_cell(&tracked(2, 5)).text(), "↑2 ↓5");
    }

    #[test]
    fn a_branch_with_no_upstream_is_marked_as_having_none() {
        assert_eq!(sync_cell(&Tracking::Untracked).text(), symbol::NONE);
    }

    #[test]
    fn a_deleted_upstream_says_gone_rather_than_showing_a_tick() {
        let gone = Tracking::Gone {
            upstream: "origin/main".to_string(),
        };
        assert_eq!(sync_cell(&gone).text(), "gone");
    }

    #[test]
    fn a_distance_nobody_measured_renders_as_nothing_at_all() {
        // Not a tick, and not a glyph of its own: "not measured" is the
        // absence of a reading, and anything on screen would look like one.
        let unmeasured = Tracking::Unmeasured {
            upstream: "origin/main".to_string(),
        };
        assert!(sync_cell(&unmeasured).is_empty());
    }
}
