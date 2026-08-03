//! `grit show` — what is registered, where, and under which tags.

use anyhow::Result;
use serde::Serialize;

use crate::cli::ShowArgs;
use crate::context::Ctx;
use crate::registry::{Repo, abbreviate_home};
use crate::render::{Cell, Table, Theme, footer, paint, plural, symbol};

#[derive(Serialize)]
struct ShowJson<'a> {
    config: String,
    repos: Vec<RepoJson<'a>>,
}

#[derive(Serialize)]
struct RepoJson<'a> {
    alias: &'a str,
    path: &'a std::path::Path,
    kind: crate::registry::VcsKind,
    tags: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    added_at: Option<&'a str>,
}

pub fn run(args: &ShowArgs, ctx: &mut Ctx) -> Result<()> {
    let repos = ctx.registry.select(&[], args.tag.as_deref())?;

    if args.json {
        return print_json(&repos, ctx);
    }

    if repos.is_empty() {
        print_empty_hint(ctx);
        return Ok(());
    }

    let mut table = Table::new(["alias", "kind", "tags", "path"])
        .color(ctx.color)
        .terminal_width(ctx.width)
        .flex(3);

    for repo in &repos {
        table.push_row(vec![
            Cell::styled(&repo.alias, Theme::alias()),
            Cell::styled(repo.kind().as_str(), Theme::kind()),
            tags_cell(repo),
            Cell::styled(abbreviate_home(repo.path()), Theme::path()),
        ]);
    }

    print!("{}", table.render());
    println!();

    let mut parts = vec![plural(repos.len(), "repo", "repos")];
    let tags = ctx.registry.tags();
    if !tags.is_empty() {
        parts.push(format!("tags: {}", tags.join(", ")));
    }
    parts.push(abbreviate_home(ctx.registry.path()));
    print!("{}", footer(&parts, ctx.color));

    Ok(())
}

fn tags_cell(repo: &Repo) -> Cell {
    if repo.entry.tags.is_empty() {
        return Cell::styled(symbol::NONE, Theme::muted());
    }
    Cell::styled(repo.entry.tags.join(", "), Theme::tag())
}

fn print_json(repos: &[Repo], ctx: &Ctx) -> Result<()> {
    let payload = ShowJson {
        config: ctx.registry.path().display().to_string(),
        repos: repos
            .iter()
            .map(|r| RepoJson {
                alias: &r.alias,
                path: r.path(),
                kind: r.kind(),
                tags: &r.entry.tags,
                added_at: r.entry.added_at.as_deref(),
            })
            .collect(),
    };

    println!("{}", crate::commands::to_json(&payload)?);
    Ok(())
}

/// A first-run message that tells the user the exact next command to type.
pub fn print_empty_hint(ctx: &Ctx) {
    println!(
        "  {}\n",
        paint("No repositories registered yet.", Theme::muted(), ctx.color)
    );
    println!(
        "  {}  {}\n",
        paint("grit -r <alias> <path>", Theme::banner(), ctx.color),
        paint("register one", Theme::muted(), ctx.color),
    );
    println!(
        "  {}",
        paint(
            &format!("config: {}", abbreviate_home(ctx.registry.path())),
            Theme::muted(),
            ctx.color
        )
    );
}
