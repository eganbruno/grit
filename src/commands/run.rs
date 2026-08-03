//! `grit <alias|@tag> <args...>` — run a repo's VCS from anywhere.
//!
//! grit adds nothing to the command it runs. stdio is inherited, so the child
//! sees a real terminal and behaves exactly as it would if you had `cd`'d
//! there first: pagers page, colour stays on, `$EDITOR` opens.

use std::ffi::OsString;
use std::io::Write as _;

use anyhow::{Result, bail};

use crate::context::Ctx;
use crate::error::Error;
use crate::registry::Repo;
use crate::render::{Theme, divider, footer, paint, plural};
use crate::vcs;

pub fn run(argv: &[OsString], keep_going: bool, ctx: &mut Ctx) -> Result<i32> {
    let Some((target, args)) = argv.split_first() else {
        bail!("nothing to run");
    };

    let Some(target) = target.to_str() else {
        return Err(Error::UnknownAlias(target.to_string_lossy().into_owned()).into());
    };

    let repos = ctx.registry.resolve_target(target)?;

    if args.is_empty() {
        bail!(
            "`grit {target}` needs a command to run, e.g. `grit {target} status`\n\
             for grit's own view of the repo, use `grit status {}`",
            target.trim_start_matches('@')
        );
    }

    match repos.as_slice() {
        [repo] => exec_one(repo, args),
        many => fan_out(many, args, keep_going, ctx),
    }
}

fn exec_one(repo: &Repo, args: &[OsString]) -> Result<i32> {
    let status = vcs::provider_for(repo.kind()).exec(repo.path(), args)?;
    Ok(exit_code(status))
}

/// Run the same command in every repo of a group, one after another.
///
/// Sequential on purpose. Interleaved output from four repos is unreadable, and
/// a mutating command that half-applies in parallel is worse than one that
/// stops at the first repo it could not handle.
fn fan_out(repos: &[Repo], args: &[OsString], keep_going: bool, ctx: &Ctx) -> Result<i32> {
    let mut first_failure = 0;
    let mut ran = 0;
    let mut failed = Vec::new();

    for repo in repos {
        print!("{}", divider(&repo.alias, ctx.width, ctx.color));
        // The child writes straight to the fd, so our own buffer has to go
        // first or the divider lands after the output it labels.
        std::io::stdout().flush().ok();

        let code = exec_one(repo, args)?;
        ran += 1;

        if code != 0 {
            failed.push(repo.alias.clone());
            if first_failure == 0 {
                first_failure = code;
            }
            if !keep_going {
                println!();
                print!(
                    "{}",
                    paint(
                        &format!(
                            "  stopped at `{}` (exit {code}); {} left. \
                             pass -k to keep going\n",
                            repo.alias,
                            repos.len() - ran
                        ),
                        Theme::warn(),
                        ctx.color,
                    )
                );
                return Ok(first_failure);
            }
        }
    }

    println!();
    let mut parts = vec![plural(ran, "repo", "repos")];
    if failed.is_empty() {
        parts.push("all ok".to_string());
    } else {
        parts.push(format!("failed: {}", failed.join(", ")));
    }
    print!("{}", footer(&parts, ctx.color));

    Ok(first_failure)
}

/// Turn a child's exit status into an exit code we can return.
///
/// A process killed by a signal has no exit code; the shell convention of
/// `128 + signal` keeps that information rather than flattening it to 1.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Registry, RepoEntry, VcsKind};
    use std::path::PathBuf;

    fn ctx_with(aliases: &[(&str, &[&str])]) -> Ctx {
        let mut registry = Registry::empty_at("/tmp/grit-test.toml");
        for (alias, tags) in aliases {
            registry.insert(
                (*alias).to_string(),
                RepoEntry {
                    path: PathBuf::from(format!("/code/{alias}")),
                    kind: VcsKind::Git,
                    tags: tags.iter().map(|s| s.to_string()).collect(),
                    added_at: None,
                },
            );
        }
        Ctx::with_registry(registry, false, Some(80))
    }

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    #[test]
    fn an_unknown_alias_is_rejected_before_anything_runs() {
        let mut ctx = ctx_with(&[("docs", &[])]);
        let err = run(&argv(&["nope", "status"]), false, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("no repo registered under alias"));
    }

    #[test]
    fn an_unknown_group_is_rejected_before_anything_runs() {
        let mut ctx = ctx_with(&[("docs", &["release"])]);
        let err = run(&argv(&["@nope", "fetch"]), false, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("no repos tagged"));
    }

    #[test]
    fn a_target_with_no_command_explains_what_to_type() {
        let mut ctx = ctx_with(&[("docs", &[])]);
        let err = run(&argv(&["docs"]), false, &mut ctx).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("needs a command to run"), "{msg}");
        assert!(msg.contains("grit status docs"), "{msg}");
    }

    #[test]
    fn a_group_with_no_command_suggests_the_untagged_form() {
        let mut ctx = ctx_with(&[("docs", &["release"])]);
        let err = run(&argv(&["@release"]), false, &mut ctx).unwrap_err();
        assert!(err.to_string().contains("grit status release"));
    }

    #[test]
    fn a_normal_exit_code_passes_straight_through() {
        // `false` exits 1, `true` exits 0 — the cheapest real processes around.
        let ok = std::process::Command::new("true").status().unwrap();
        let bad = std::process::Command::new("false").status().unwrap();
        assert_eq!(exit_code(ok), 0);
        assert_eq!(exit_code(bad), 1);
    }
}
