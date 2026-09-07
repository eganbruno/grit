//! `grit -r <alias> <path>` / `grit register` / `grit remove`.

use anyhow::{Context as _, Result};

use crate::cli::{RegisterArgs, RemoveArgs};
use crate::context::Ctx;
use crate::error::Error;
use crate::registry::{RepoEntry, VcsKind, abbreviate_home, validate_alias};
use crate::render::{Theme, paint};
use crate::vcs;

pub fn register(args: &RegisterArgs, ctx: &mut Ctx) -> Result<()> {
    validate_alias(&args.alias)?;
    ctx.registry.check_available(&args.alias, args.force)?;

    let given = args
        .path
        .canonicalize()
        .map_err(|_| Error::PathNotFound(args.path.clone()))?;

    let (kind, root) = detect_repo(&given)?;

    let mut tags = args.tags.clone();
    tags.sort();
    tags.dedup();

    let replaced = ctx.registry.insert(
        args.alias.clone(),
        RepoEntry {
            path: root.clone(),
            kind,
            tags: tags.clone(),
            added_at: Some(now_rfc3339()),
        },
    );

    ctx.registry
        .save()
        .with_context(|| format!("could not save {}", ctx.registry.path().display()))?;

    let verb = if replaced.is_some() {
        "re-registered"
    } else {
        "registered"
    };
    let tag_note = if tags.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            paint(&format!("[{}]", tags.join(", ")), Theme::tag(), ctx.color)
        )
    };

    println!(
        "{verb} {}{tag_note} {} {}",
        paint(&args.alias, Theme::alias(), ctx.color),
        paint("→", Theme::muted(), ctx.color),
        paint(&abbreviate_home(&root), Theme::path(), ctx.color),
    );

    Ok(())
}

/// The current time, to the second.
///
/// Sub-second precision would be noise in a file a person reads: this records
/// roughly when an alias was added, not an ordering anything depends on.
fn now_rfc3339() -> String {
    let now = jiff::Timestamp::now();
    now.round(jiff::Unit::Second).unwrap_or(now).to_string()
}

/// Find which backend owns `path`, and the repository root it belongs to.
///
/// Backends are tried in [`vcs::all_providers`] order, so adding dolt later is
/// a one-line change there rather than a change here.
fn detect_repo(path: &std::path::Path) -> Result<(VcsKind, std::path::PathBuf)> {
    for provider in vcs::all_providers() {
        if let Some(root) = provider.discover(path) {
            return Ok((provider.kind(), root));
        }
    }

    Err(Error::NotARepo {
        path: path.to_path_buf(),
        kinds: known_kinds(),
    }
    .into())
}

/// Every backend grit could have recognised, as an English list: `dolt or git`.
///
/// Read off [`vcs::all_providers`] rather than spelled out, so adding a backend
/// cannot leave this message claiming grit only understands the old ones.
fn known_kinds() -> String {
    vcs::all_providers()
        .iter()
        .map(|provider| provider.kind().as_str())
        .collect::<Vec<_>>()
        .join(" or ")
}

pub fn remove(args: &RemoveArgs, ctx: &mut Ctx) -> Result<()> {
    // Remove everything or nothing: check all the aliases exist before
    // mutating, so `grit rm good bad` does not half-apply.
    for alias in &args.aliases {
        ctx.registry.get(alias)?;
    }

    for alias in &args.aliases {
        let entry = ctx.registry.remove(alias)?;
        println!(
            "removed {} {} {}",
            paint(alias, Theme::alias(), ctx.color),
            paint("←", Theme::muted(), ctx.color),
            paint(&abbreviate_home(&entry.path), Theme::path(), ctx.color),
        );
    }

    ctx.registry
        .save()
        .with_context(|| format!("could not save {}", ctx.registry.path().display()))?;

    Ok(())
}
