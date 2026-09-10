//! `grit detail` — one repository, in depth.
//!
//! Where `grit status` gives every repo one row, this gives one repo a card:
//! the header the dashboard would have shown, then a section each for the
//! changed paths, the recent commits, the branches and the stash stack.
//!
//! Every section is a [`Table`], so alignment, truncation and colour come from
//! the one layout path the dashboard uses rather than from a second copy of
//! it. Each is laid out on its own, which is the point — a card is a stack of
//! short tables that each fit their own content, not one grid with four
//! different shapes forced through it.
//!
//! Nothing here reads the cache. The cache exists so a dashboard of twenty
//! repositories can appear instantly; this is one repository, asked for by
//! name, and a fresh reading of one repo is fast enough to just take.

use anyhow::{Context as _, Result};

use crate::cli::DetailArgs;
use crate::context::Ctx;
use crate::registry::{Repo, abbreviate_home};
use crate::render::{Align, Cell, Table, Theme, footer, paint, plural, symbol};
use crate::vcs::{self, Branch, Change, Detail, FileChange, RepoState, Snapshot};

pub fn run(args: &DetailArgs, ctx: &mut Ctx) -> Result<i32> {
    let repo = ctx.registry.get(&args.alias)?;

    let detail = vcs::provider_for(repo.kind())
        .detail(repo.path())
        .with_context(|| format!("could not read `{}`", repo.alias))?;

    if args.json {
        println!("{}", super::to_json(&DetailJson::new(&repo, &detail))?);
    } else {
        print_card(&repo, &detail, ctx);
    }

    Ok(0)
}

fn print_card(repo: &Repo, detail: &Detail, ctx: &Ctx) {
    print_header(repo, detail, ctx);

    if detail.snapshot.state == RepoState::Missing {
        // Nothing below the header would say anything: there is no working
        // tree to have changes in. The header already says `missing`.
        return;
    }

    section("changes", changes_table(&detail.files, ctx), ctx);
    section("commits", commits_table(detail, ctx), ctx);
    section("branches", branches_table(&detail.branches, ctx), ctx);
    section("stashes", stashes_table(detail, ctx), ctx);

    let parts = summarise(detail);
    if !parts.is_empty() {
        println!();
        print!("{}", footer(&parts, ctx.color));
    }
}

/// The alias, where it lives, and what the dashboard would have said about it.
fn print_header(repo: &Repo, detail: &Detail, ctx: &Ctx) {
    let snap = &detail.snapshot;

    println!();
    println!(
        "  {}  {}",
        paint(&repo.alias, Theme::alias(), ctx.color),
        paint(&abbreviate_home(repo.path()), Theme::path(), ctx.color),
    );

    // The backend and the tags are registry facts rather than readings, and
    // they belong with the path rather than with the branch line below.
    let mut about = vec![paint(repo.kind().as_str(), Theme::kind(), ctx.color)];
    if !repo.entry.tags.is_empty() {
        about.push(paint(&repo.entry.tags.join(", "), Theme::tag(), ctx.color));
    }
    println!(
        "  {}",
        about.join(&format!(
            " {} ",
            paint(symbol::BULLET, Theme::muted(), ctx.color)
        )),
    );

    println!();
    println!("  {}", header_line(snap, ctx));
}

/// `main → origin/main   ↑2 ↓1   ●3 ○1 ?2`, or `missing`.
///
/// Built as a [`Cell`] and rendered by hand rather than through a table: it is
/// one line of differently-coloured pieces, which is exactly what a cell is,
/// and there are no columns to align it against.
fn header_line(snap: &Snapshot, ctx: &Ctx) -> String {
    if snap.state == RepoState::Missing {
        return paint("missing", Theme::warn(), ctx.color);
    }

    let mut parts = vec![match &snap.branch {
        Some(branch) => paint(branch, Theme::branch(), ctx.color),
        None => paint("detached", Theme::detached(), ctx.color),
    }];

    if let Some(upstream) = &snap.upstream {
        parts.push(paint("→", Theme::muted(), ctx.color));
        parts.push(paint(upstream, Theme::muted(), ctx.color));
    }

    let sync = super::status::sync_cell(snap);
    let state = super::status::state_cell(snap);
    parts.push(render_cell(&sync, ctx));
    parts.push(render_cell(&state, ctx));

    parts.join("  ")
}

/// A cell's spans, painted, with nothing padded around them.
fn render_cell(cell: &Cell, ctx: &Ctx) -> String {
    cell.spans()
        .iter()
        .map(|span| paint(&span.text, span.style, ctx.color))
        .collect()
}

/// A titled section, printed only when its table has rows in it.
fn section(title: &str, table: Option<Table>, ctx: &Ctx) {
    let Some(table) = table else {
        return;
    };

    println!();
    println!(
        "  {}",
        paint(&title.to_uppercase(), Theme::header(), ctx.color)
    );
    print!("{}", table.render());
}

/// Every section's table is built the same way: same colour, same width, and
/// no header row, because the section title above it is the label.
fn card_table<const N: usize>(headers: [&str; N], ctx: &Ctx) -> Table {
    Table::new(headers)
        .color(ctx.color)
        .terminal_width(ctx.width)
        .no_header()
        .no_rule()
}

fn changes_table(files: &[FileChange], ctx: &Ctx) -> Option<Table> {
    if files.is_empty() {
        return None;
    }

    let mut table = card_table(["code", "path"], ctx).flex(1);
    for file in files {
        table.push_row(vec![code_cell(file), path_cell(file)]);
    }
    Some(table)
}

/// The two-column code: staged on the left, unstaged on the right, in the
/// colours the dashboard already uses for those counts.
///
/// A space rather than git's `.` for "nothing on this side" — the dashboard
/// spends `·` on "no upstream", and a column of dots down the left of a file
/// list reads as content rather than as absence.
fn code_cell(file: &FileChange) -> Cell {
    let side = |change: Option<Change>, staged: bool| match change {
        None => (" ".to_string(), Theme::muted()),
        Some(change) => (
            change.code().to_string(),
            match change {
                Change::Conflicted => Theme::conflict(),
                Change::Untracked => Theme::untracked(),
                _ if staged => Theme::staged(),
                _ => Theme::unstaged(),
            },
        ),
    };

    let (left, left_style) = side(file.staged, true);
    let (right, right_style) = side(file.unstaged, false);
    Cell::styled(left, left_style).push(right, right_style)
}

fn path_cell(file: &FileChange) -> Cell {
    let cell = Cell::plain(&file.path);
    match &file.origin {
        // Which way round matters: this is where the path came *from*.
        Some(origin) => cell.push_spaced(format!("← {origin}"), Theme::muted()),
        None => cell,
    }
}

fn commits_table(detail: &Detail, ctx: &Ctx) -> Option<Table> {
    if detail.commits.is_empty() {
        return None;
    }

    let mut table = card_table(["commit", "subject", "age"], ctx)
        .flex(1)
        .align(2, Align::Right);

    for commit in &detail.commits {
        table.push_row(vec![
            Cell::styled(&commit.short_id, Theme::sha()),
            Cell::styled(&commit.subject, Theme::subject()),
            Cell::styled(&commit.age, Theme::muted()),
        ]);
    }
    Some(table)
}

fn branches_table(branches: &[Branch], ctx: &Ctx) -> Option<Table> {
    if branches.is_empty() {
        return None;
    }

    let mut table = card_table(["at", "branch", "upstream", "sync", "subject", "age"], ctx)
        .flex(4)
        .align(5, Align::Right);

    for branch in branches {
        table.push_row(vec![
            match branch.head {
                true => Cell::styled("*", Theme::ok()),
                false => Cell::empty(),
            },
            Cell::styled(&branch.name, Theme::branch()),
            match &branch.upstream {
                Some(upstream) => Cell::styled(upstream, Theme::muted()),
                None => Cell::styled(symbol::NONE, Theme::muted()),
            },
            branch_sync_cell(branch),
            match &branch.tip {
                Some(tip) => Cell::styled(&tip.subject, Theme::subject()),
                None => Cell::styled("no commits yet", Theme::muted()),
            },
            match &branch.tip {
                Some(tip) => Cell::styled(&tip.age, Theme::muted()),
                None => Cell::empty(),
            },
        ]);
    }
    Some(table)
}

/// The same cases the dashboard's sync column distinguishes, for one branch
/// rather than for the checked-out one.
///
/// A branch with no measured distance gets nothing at all — not a `✓`. Its
/// upstream may have been deleted, or the backend may not have been able to
/// run the count; either way nothing has been compared, and a tick would say
/// the opposite.
fn branch_sync_cell(branch: &Branch) -> Cell {
    let Some(distance) = branch.distance else {
        return Cell::empty();
    };
    if distance.is_level() {
        return Cell::styled(symbol::SYNCED, Theme::ok());
    }

    let mut cell = Cell::empty();
    if distance.ahead > 0 {
        cell = cell.push(
            format!("{}{}", symbol::AHEAD, distance.ahead),
            Theme::ahead(),
        );
    }
    if distance.behind > 0 {
        cell = cell.push_spaced(
            format!("{}{}", symbol::BEHIND, distance.behind),
            Theme::behind(),
        );
    }
    cell
}

fn stashes_table(detail: &Detail, ctx: &Ctx) -> Option<Table> {
    if detail.stashes.is_empty() {
        return None;
    }

    let mut table = card_table(["id", "message", "age"], ctx)
        .flex(1)
        .align(2, Align::Right);

    for stash in &detail.stashes {
        table.push_row(vec![
            Cell::styled(&stash.id, Theme::stash()),
            Cell::styled(&stash.message, Theme::subject()),
            Cell::styled(&stash.age, Theme::muted()),
        ]);
    }
    Some(table)
}

/// The one-line summary under the card. Shares `plural` and `footer` with the
/// dashboard so the two count the same things in the same words.
pub fn summarise(detail: &Detail) -> Vec<String> {
    let mut parts = Vec::new();

    if !detail.files.is_empty() {
        parts.push(plural(detail.files.len(), "change", "changes"));
    }
    if !detail.branches.is_empty() {
        parts.push(plural(detail.branches.len(), "branch", "branches"));
    }
    if !detail.stashes.is_empty() {
        parts.push(plural(detail.stashes.len(), "stash", "stashes"));
    }
    parts
}

#[derive(serde::Serialize)]
struct DetailJson<'a> {
    alias: &'a str,
    path: &'a std::path::Path,
    kind: crate::registry::VcsKind,
    tags: &'a [String],
    #[serde(flatten)]
    detail: &'a Detail,
}

impl<'a> DetailJson<'a> {
    fn new(repo: &'a Repo, detail: &'a Detail) -> Self {
        Self {
            alias: &repo.alias,
            path: repo.path(),
            kind: repo.kind(),
            tags: &repo.entry.tags,
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::{Commit, Distance, Stash};

    fn file(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> FileChange {
        FileChange {
            path: path.to_string(),
            origin: None,
            staged,
            unstaged,
        }
    }

    #[test]
    fn a_file_staged_and_then_modified_again_shows_both_sides() {
        let cell = code_cell(&file(
            "a.txt",
            Some(Change::Modified),
            Some(Change::Modified),
        ));
        assert_eq!(cell.text(), "MM");
    }

    #[test]
    fn a_side_with_nothing_on_it_is_a_space_not_a_dot() {
        let cell = code_cell(&file("a.txt", None, Some(Change::Modified)));
        assert_eq!(cell.text(), " M");
    }

    #[test]
    fn an_untracked_file_is_marked_on_the_unstaged_side() {
        let cell = code_cell(&file("new.md", None, Some(Change::Untracked)));
        assert_eq!(cell.text(), " ?");
    }

    #[test]
    fn a_conflict_is_marked_on_both_sides() {
        let cell = code_cell(&file(
            "f.txt",
            Some(Change::Conflicted),
            Some(Change::Conflicted),
        ));
        assert_eq!(cell.text(), "UU");
    }

    #[test]
    fn a_rename_says_where_it_came_from() {
        let mut renamed = file("new.txt", Some(Change::Renamed), None);
        renamed.origin = Some("old.txt".to_string());
        assert_eq!(path_cell(&renamed).text(), "new.txt ← old.txt");
    }

    #[test]
    fn the_code_column_is_always_two_columns_wide() {
        // Ragged codes would knock the path column out of line on every row
        // that happened to have one side clean.
        for (staged, unstaged) in [
            (None, None),
            (Some(Change::Added), None),
            (None, Some(Change::Deleted)),
            (Some(Change::Modified), Some(Change::Modified)),
        ] {
            assert_eq!(code_cell(&file("x", staged, unstaged)).width(), 2);
        }
    }

    fn branch(name: &str, upstream: Option<&str>, ahead: u32, behind: u32) -> Branch {
        Branch {
            name: name.to_string(),
            upstream: upstream.map(str::to_string),
            distance: upstream.map(|_| Distance { ahead, behind }),
            head: false,
            tip: Some(Commit {
                short_id: "abc1234".into(),
                subject: "do a thing".into(),
                age: "3h".into(),
            }),
        }
    }

    #[test]
    fn a_branch_level_with_its_upstream_gets_a_tick() {
        assert_eq!(
            branch_sync_cell(&branch("main", Some("origin/main"), 0, 0)).text(),
            symbol::SYNCED
        );
    }

    #[test]
    fn a_branch_with_no_upstream_gets_no_sync_marker_at_all() {
        // Not a tick: nothing has been compared, which is a different fact
        // from having compared and found no difference.
        assert!(branch_sync_cell(&branch("wip", None, 0, 0)).is_empty());
    }

    #[test]
    fn a_branch_whose_distance_was_never_measured_gets_no_tick_either() {
        // An upstream that has been deleted, or a backend that could not run
        // the count. `git for-each-ref` still names the upstream in that first
        // case, so reading the missing distance as zero is exactly how a ✓
        // ends up against a branch tracking something that is gone.
        let mut unmeasured = branch("stale", Some("origin/stale"), 0, 0);
        unmeasured.distance = None;
        assert!(branch_sync_cell(&unmeasured).is_empty());
    }

    #[test]
    fn a_diverged_branch_counts_both_ways() {
        assert_eq!(
            branch_sync_cell(&branch("main", Some("origin/main"), 2, 5)).text(),
            "↑2 ↓5"
        );
    }

    #[test]
    fn the_summary_names_only_what_is_there() {
        let detail = Detail {
            files: vec![file("a", None, Some(Change::Modified))],
            stashes: vec![Stash {
                id: "stash@{0}".into(),
                message: "wip".into(),
                age: "4h".into(),
            }],
            ..Detail::default()
        };
        assert_eq!(summarise(&detail), ["1 change", "1 stash"]);
    }

    #[test]
    fn a_quiet_repo_summarises_to_nothing_rather_than_to_zeroes() {
        assert!(summarise(&Detail::default()).is_empty());
    }

    #[test]
    fn empty_sections_are_left_out_entirely() {
        let ctx = Ctx::with_registry(
            crate::registry::Registry::empty_at("/tmp/does-not-matter.toml"),
            false,
            Some(80),
        );
        assert!(changes_table(&[], &ctx).is_none());
        assert!(branches_table(&[], &ctx).is_none());
        assert!(commits_table(&Detail::default(), &ctx).is_none());
        assert!(stashes_table(&Detail::default(), &ctx).is_none());
    }
}
