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
use owo_colors::Style;

use crate::render::{Align, Cell, Table, Theme, footer, paint, plural, symbol};
use crate::vcs::{self, Branch, Change, Detail, FileChange, RepoState, Snapshot};

pub fn run(args: &DetailArgs, ctx: &mut Ctx) -> Result<i32> {
    let repo = ctx.registry.get(&args.alias)?;

    let detail = vcs::provider_for(repo.kind())
        .detail(repo.path())
        .with_context(|| format!("could not read `{}`", repo.alias))?;

    let out = if args.json {
        format!("{}\n", super::to_json(&DetailJson::new(&repo, &detail))?)
    } else {
        card(&repo, &detail, ctx)
    };

    write_out(&out)?;
    Ok(0)
}

/// Write the whole card in one go, and treat a closed pipe as the reader being
/// finished rather than as a failure.
///
/// A card is several sections tall and `grit detail api | head` is an obvious
/// thing to type. Built as one string and written once, because a run of
/// `println!`s hits the closed pipe partway through and Rust's default for
/// that is a panic — `commands::dispatch` already takes this view of
/// `grit help | head`.
fn write_out(text: &str) -> Result<()> {
    use std::io::Write as _;

    match std::io::stdout().write_all(text.as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// One piece of the card, before anyone decides how to paint it.
///
/// The card is a stack of short tables under their own titles, with a few
/// lines of loose spans among them, so it does not fit `Table` alone. Blocks
/// are what both consumers walk: [`card`] paints them for a terminal, and
/// `tests/readme_svg.rs` lays the same ones into the SVG the README shows.
/// The arrangement `status::build_table` has with the shell preview, for the
/// same reason — one layout, two renderers.
pub enum Block {
    /// A line of styled spans, at the table margin.
    Line(Cell),
    Blank,
    /// A titled section. Absent entirely when it has no rows.
    Section(&'static str, Table),
}

/// The card as blocks.
pub fn build_card(repo: &Repo, detail: &Detail, ctx: &Ctx) -> Vec<Block> {
    let mut blocks = vec![
        Block::Blank,
        Block::Line(
            Cell::styled(&repo.alias, Theme::alias())
                .push("  ", Style::new())
                .push(abbreviate_home(repo.path()), Theme::path()),
        ),
        Block::Line(about_cell(repo)),
        Block::Blank,
        Block::Line(header_cell(&detail.snapshot)),
    ];

    if detail.snapshot.state == RepoState::Missing {
        // Nothing below the header would say anything: there is no working
        // tree to have changes in. The header already says `missing`.
        return blocks;
    }

    for (title, table) in [
        ("changes", changes_table(&detail.files, ctx)),
        ("commits", commits_table(detail, ctx)),
        ("branches", branches_table(detail.branches.as_slice(), ctx)),
        ("stashes", stashes_table(detail, ctx)),
    ] {
        if let Some(table) = table {
            blocks.push(Block::Blank);
            blocks.push(Block::Section(title, table));
        }
    }

    let parts = summarise(detail);
    if !parts.is_empty() {
        blocks.push(Block::Blank);
        blocks.push(Block::Line(Cell::styled(
            footer(&parts, false).trim().to_string(),
            Theme::muted(),
        )));
    }

    blocks
}

/// The blocks, painted.
fn card(repo: &Repo, detail: &Detail, ctx: &Ctx) -> String {
    let mut out = String::new();

    for block in build_card(repo, detail, ctx) {
        match block {
            Block::Blank => out.push('\n'),
            Block::Line(cell) => {
                out.push_str(MARGIN);
                out.push_str(&render_cell(&cell, ctx));
                out.push('\n');
            }
            Block::Section(title, table) => {
                out.push_str(MARGIN);
                out.push_str(&paint(&title.to_uppercase(), Theme::header(), ctx.color));
                out.push('\n');
                out.push_str(&table.render());
            }
        }
    }

    out
}

/// The margin a [`Table`] indents by, for the lines that are not tables.
const MARGIN: &str = "  ";

/// The backend and the tags: registry facts rather than readings, which is
/// why they sit with the path rather than with the branch line below.
fn about_cell(repo: &Repo) -> Cell {
    let cell = Cell::styled(repo.kind().as_str(), Theme::kind());
    if repo.entry.tags.is_empty() {
        return cell;
    }
    cell.push(format!(" {} ", symbol::BULLET), Theme::muted())
        .push(repo.entry.tags.join(", "), Theme::tag())
}

/// `main → origin/main   ↑2 ↓1   ●3 ○1 ?2`, or `missing`.
///
/// A [`Cell`] rather than a painted string: it is one line of
/// differently-coloured pieces, which is exactly what a cell is, and being one
/// is what lets the SVG lay it out without a second copy of the colours.
fn header_cell(snap: &Snapshot) -> Cell {
    if snap.state == RepoState::Missing {
        return Cell::styled("missing", Theme::warn());
    }

    let mut cell = match &snap.branch {
        Some(branch) => Cell::styled(branch, Theme::branch()),
        None => Cell::styled("detached", Theme::detached()),
    };

    if let Some(upstream) = &snap.upstream {
        cell = cell
            .push("  ", Style::new())
            .push("→", Theme::muted())
            .push("  ", Style::new())
            .push(upstream, Theme::muted());
    }

    for part in [
        super::status::sync_cell(snap),
        super::status::state_cell(snap),
    ] {
        if part.is_empty() {
            continue;
        }
        cell = cell.push("  ", Style::new());
        for span in part.spans() {
            cell = cell.push(span.text.clone(), span.style);
        }
    }

    cell
}

/// A cell's spans, painted, with nothing padded around them.
fn render_cell(cell: &Cell, ctx: &Ctx) -> String {
    cell.spans()
        .iter()
        .map(|span| paint(&span.text, span.style, ctx.color))
        .collect()
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

/// The branch list, in the same columns and the same colours `grit branch`
/// gives it — and through the very same `sync_cell`, so the two cannot come
/// to disagree about what `gone`, or a distance nobody measured, looks like.
fn branches_table(branches: &[Branch], ctx: &Ctx) -> Option<Table> {
    if branches.is_empty() {
        return None;
    }

    let mut table = card_table(["at", "branch", "upstream", "sync", "subject", "age"], ctx)
        .flex(4)
        .align(5, Align::Right);

    for branch in branches {
        table.push_row(vec![
            match branch.is_head {
                true => Cell::styled("*", Theme::ok()),
                false => Cell::empty(),
            },
            Cell::styled(&branch.name, Theme::branch()),
            match branch.tracking.upstream() {
                Some(upstream) => Cell::styled(upstream, Theme::muted()),
                None => Cell::styled(symbol::NONE, Theme::muted()),
            },
            super::branch::sync_cell(&branch.tracking),
            match &branch.head {
                Some(tip) => Cell::styled(&tip.subject, Theme::subject()),
                None => Cell::styled("no commits yet", Theme::muted()),
            },
            match &branch.head {
                Some(tip) => Cell::styled(&tip.age, Theme::muted()),
                None => Cell::empty(),
            },
        ]);
    }
    Some(table)
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
    let branches = detail.branches.as_slice();
    if !branches.is_empty() {
        parts.push(plural(branches.len(), "branch", "branches"));
    }
    if !detail.stashes.is_empty() {
        parts.push(plural(detail.stashes.len(), "stash", "stashes"));
    }
    parts
}

/// The card as one JSON object.
///
/// `branches` and `missing` are spelled the way `grit branch --json` spells
/// them rather than as the [`Branches`] enum serde would write by itself: two
/// commands answering about the same branches should answer in the same shape.
#[derive(serde::Serialize)]
struct DetailJson<'a> {
    alias: &'a str,
    path: &'a std::path::Path,
    kind: crate::registry::VcsKind,
    tags: &'a [String],
    snapshot: &'a Snapshot,
    files: &'a [FileChange],
    commits: &'a [crate::vcs::Commit],
    stashes: &'a [crate::vcs::Stash],
    #[serde(skip_serializing_if = "Option::is_none")]
    branches: Option<&'a [Branch]>,
    /// Only present, and only true, when the registered path is gone.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    missing: bool,
}

impl<'a> DetailJson<'a> {
    fn new(repo: &'a Repo, detail: &'a Detail) -> Self {
        let missing = matches!(detail.branches, crate::vcs::Branches::Missing);
        Self {
            alias: &repo.alias,
            path: repo.path(),
            kind: repo.kind(),
            tags: &repo.entry.tags,
            snapshot: &detail.snapshot,
            files: &detail.files,
            commits: &detail.commits,
            stashes: &detail.stashes,
            branches: (!missing).then(|| detail.branches.as_slice()),
            missing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcs::Stash;

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
