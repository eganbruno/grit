//! The git backend.
//!
//! Reading a repository is two parts: run a plumbing command, then parse its
//! output. The two are kept separate on purpose — every parser here is a pure
//! `&str -> value` function, so the interesting logic is unit-testable without
//! creating a single repository on disk.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::error::{Error, Result};
use crate::registry::VcsKind;
use crate::vcs::{Commit, RepoState, Snapshot, Vcs};

/// Field separator for `git log --format`. ASCII unit separator: cannot appear
/// in a commit subject, so subjects with spaces, tabs or unicode parse cleanly.
const SEP: char = '\u{1f}';

const LOG_FORMAT: &str = "--format=%h\u{1f}%s\u{1f}%cr";

pub struct GitVcs;

impl Vcs for GitVcs {
    fn kind(&self) -> VcsKind {
        VcsKind::Git
    }

    fn discover(&self, path: &Path) -> Option<PathBuf> {
        if !path.exists() {
            return None;
        }
        let out = capture(path, &["rev-parse", "--show-toplevel"]).ok()?;
        let root = out.trim();
        if root.is_empty() {
            return None;
        }
        Some(PathBuf::from(root))
    }

    fn snapshot(&self, path: &Path) -> Result<Snapshot> {
        if !path.exists() {
            return Ok(Snapshot {
                state: RepoState::Missing,
                ..Snapshot::default()
            });
        }

        let status_out = capture(path, &["status", "--porcelain=v2", "--branch"])?;
        let mut snapshot = parse_status(&status_out);

        // A repo with no commits has no HEAD to describe; that is not an error.
        if let Ok(log_out) = capture(path, &["log", "-1", LOG_FORMAT]) {
            snapshot.head = parse_log_line(&log_out);
        }

        // Fails with exit 128 when refs/stash does not exist, i.e. no stashes.
        snapshot.stashes = capture(
            path,
            &["rev-list", "--walk-reflogs", "--count", "refs/stash"],
        )
        .ok()
        .and_then(|out| out.trim().parse().ok())
        .unwrap_or(0);

        // Conflicts imply *some* in-progress operation; a more specific state
        // from the git directory wins over that inference.
        if let Some(state) = in_progress_state(path) {
            snapshot.state = state;
        } else if snapshot.conflicts > 0 {
            snapshot.state = RepoState::Merging;
        }

        Ok(snapshot)
    }

    fn exec(&self, path: &Path, args: &[OsString]) -> Result<ExitStatus> {
        Command::new("git")
            .current_dir(path)
            .args(args)
            // Inherited, not captured: this is what keeps the user's pager
            // (delta, less), colour detection and $EDITOR working.
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|source| Error::Spawn {
                program: "git",
                source,
            })
    }
}

/// Run a git command and return its stdout, erroring if it exits non-zero.
fn capture(path: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(path)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Spawn {
            program: "git",
            source,
        })?;

    if !out.status.success() {
        return Err(Error::CommandFailed {
            program: "git",
            args: args.join(" "),
            status: out.status.to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `git status --porcelain=v2 --branch`.
///
/// The format is documented in `git-status(1)` under "Porcelain Format Version
/// 2". Header lines start with `#`; entry lines start with a type character:
///
/// - `1` ordinary change, `2` rename/copy — the two-character `<XY>` field says
///   whether the change is staged (`X`), unstaged (`Y`), or both; `.` is "no
///   change on this side"
/// - `u` unmerged, i.e. a conflict
/// - `?` untracked, `!` ignored (not requested, so not counted)
fn parse_status(out: &str) -> Snapshot {
    let mut snap = Snapshot::default();

    for line in out.lines() {
        let Some((tag, rest)) = split_once_ws(line) else {
            continue;
        };

        match tag {
            "#" => parse_branch_header(rest, &mut snap),
            "1" | "2" => {
                let Some((xy, _)) = split_once_ws(rest) else {
                    continue;
                };
                let mut chars = xy.chars();
                if chars.next().is_some_and(|c| c != '.') {
                    snap.staged += 1;
                }
                if chars.next().is_some_and(|c| c != '.') {
                    snap.unstaged += 1;
                }
            }
            "u" => snap.conflicts += 1,
            "?" => snap.untracked += 1,
            _ => {}
        }
    }

    snap
}

/// Parse one `# branch.*` header line (the `# ` prefix already stripped).
fn parse_branch_header(rest: &str, snap: &mut Snapshot) {
    let Some((key, value)) = split_once_ws(rest) else {
        return;
    };

    match key {
        "branch.head" => {
            // git writes the literal `(detached)` rather than a branch name.
            if value != "(detached)" {
                snap.branch = Some(value.to_string());
            }
        }
        "branch.upstream" => snap.upstream = Some(value.to_string()),
        "branch.ab" => {
            for token in value.split_whitespace() {
                match token.split_at_checked(1) {
                    Some(("+", n)) => snap.ahead = n.parse().unwrap_or(0),
                    Some(("-", n)) => snap.behind = n.parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Parse the single line produced by [`LOG_FORMAT`].
fn parse_log_line(out: &str) -> Option<Commit> {
    let line = out.lines().next()?;
    let mut fields = line.split(SEP);

    let short_id = fields.next()?.trim().to_string();
    if short_id.is_empty() {
        return None;
    }

    Some(Commit {
        subject: fields.next().unwrap_or_default().to_string(),
        age: compact_relative_time(fields.next().unwrap_or_default()),
        short_id,
    })
}

/// Squeeze git's `%cr` into something that fits a column: `28 hours ago` → `28h`.
///
/// Anything unrecognised is passed through untouched rather than mangled, so a
/// future git wording change degrades to "slightly wide" instead of "wrong".
pub fn compact_relative_time(relative: &str) -> String {
    let text = relative.trim();
    let text = text.strip_suffix(" ago").unwrap_or(text);

    if text.is_empty() || text == "in the future" {
        return text.to_string();
    }

    let mut parts = text.split_whitespace();
    let (Some(count), Some(unit)) = (parts.next(), parts.next()) else {
        return text.to_string();
    };

    if count.parse::<u64>().is_err() {
        return text.to_string();
    }

    // "2 years, 3 months" keeps only the leading term; trim its comma first.
    let unit = unit.trim_end_matches(',');
    let unit = unit.strip_suffix('s').unwrap_or(unit);

    let suffix = match unit {
        "second" => "s",
        "minute" => "m",
        "hour" => "h",
        "day" => "d",
        "week" => "w",
        "month" => "mo",
        "year" => "y",
        _ => return text.to_string(),
    };

    format!("{count}{suffix}")
}

/// Detect an interrupted merge/rebase/etc. by looking for the marker files git
/// leaves in the git directory.
///
/// Pure filesystem checks: this saves a `git rev-parse` subprocess on every
/// repo in the dashboard.
fn in_progress_state(worktree: &Path) -> Option<RepoState> {
    let git_dir = resolve_git_dir(worktree)?;

    // Order matters: a conflicted rebase also has MERGE_HEAD-like markers, and
    // "rebasing" is the more useful thing to tell the user.
    let checks = [
        ("rebase-merge", RepoState::Rebasing),
        ("rebase-apply", RepoState::Rebasing),
        ("MERGE_HEAD", RepoState::Merging),
        ("CHERRY_PICK_HEAD", RepoState::CherryPicking),
        ("REVERT_HEAD", RepoState::Reverting),
        ("BISECT_LOG", RepoState::Bisecting),
    ];

    checks
        .into_iter()
        .find(|(marker, _)| git_dir.join(marker).exists())
        .map(|(_, state)| state)
}

/// Find the git directory for a working tree without shelling out.
///
/// `.git` is normally a directory, but is a file containing `gitdir: <path>`
/// for worktrees and submodules.
fn resolve_git_dir(worktree: &Path) -> Option<PathBuf> {
    let dot_git = worktree.join(".git");

    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }

    let contents = std::fs::read_to_string(&dot_git).ok()?;
    let target = contents.lines().next()?.strip_prefix("gitdir:")?.trim();

    let target = Path::new(target);
    Some(if target.is_absolute() {
        target.to_path_buf()
    } else {
        worktree.join(target)
    })
}

/// Split on the first run of whitespace. Returns `None` if there is no second
/// field, which for this format always means a line we do not care about.
fn split_once_ws(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_end_matches(['\r', '\n']);
    let idx = line.find(char::is_whitespace)?;
    let (head, tail) = line.split_at(idx);
    Some((head, tail.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean repo tracking an in-sync upstream.
    const CLEAN: &str = "\
# branch.oid a1b2c3d4e5f60718293a4b5c6d7e8f9012345678
# branch.head main
# branch.upstream origin/main
# branch.ab +0 -0
";

    #[test]
    fn clean_repo_has_no_counts() {
        let s = parse_status(CLEAN);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (0, 0));
        assert!(s.is_clean());
        assert!(s.is_synced());
    }

    #[test]
    fn ahead_and_behind_are_read_from_branch_ab() {
        let out = CLEAN.replace("+0 -0", "+3 -12");
        let s = parse_status(&out);
        assert_eq!((s.ahead, s.behind), (3, 12));
        assert!(!s.is_synced());
    }

    #[test]
    fn detached_head_has_no_branch() {
        let out = CLEAN.replace("main", "(detached)");
        let s = parse_status(&out);
        assert_eq!(s.branch, None);
    }

    #[test]
    fn a_branch_with_no_upstream_has_no_ab_line() {
        let out = "\
# branch.oid a1b2c3d4
# branch.head feature/thing
";
        let s = parse_status(out);
        assert_eq!(s.branch.as_deref(), Some("feature/thing"));
        assert_eq!(s.upstream, None);
        assert!(s.is_synced());
    }

    #[test]
    fn a_slash_in_the_branch_name_survives() {
        let out = CLEAN.replace("head main", "head feature/rate-limits");
        assert_eq!(
            parse_status(&out).branch.as_deref(),
            Some("feature/rate-limits")
        );
    }

    #[test]
    fn xy_first_column_counts_as_staged() {
        let out = format!("{CLEAN}1 M. N... 100644 100644 100644 abc123 def456 src/main.rs\n");
        let s = parse_status(&out);
        assert_eq!((s.staged, s.unstaged), (1, 0));
        assert!(!s.is_clean());
    }

    #[test]
    fn xy_second_column_counts_as_unstaged() {
        let out = format!("{CLEAN}1 .M N... 100644 100644 100644 abc123 def456 src/main.rs\n");
        let s = parse_status(&out);
        assert_eq!((s.staged, s.unstaged), (0, 1));
    }

    #[test]
    fn a_file_changed_on_both_sides_counts_once_each() {
        let out = format!("{CLEAN}1 MM N... 100644 100644 100644 abc123 def456 src/main.rs\n");
        let s = parse_status(&out);
        assert_eq!((s.staged, s.unstaged), (1, 1));
    }

    #[test]
    fn renames_are_counted_like_ordinary_changes() {
        let out = format!(
            "{CLEAN}2 R. N... 100644 100644 100644 abc123 def456 R100 new/path.rs\u{0}old/path.rs\n"
        );
        let s = parse_status(&out);
        assert_eq!((s.staged, s.unstaged), (1, 0));
    }

    #[test]
    fn untracked_and_ignored_are_told_apart() {
        let out = format!("{CLEAN}? notes.md\n? build/out.log\n! target/\n");
        let s = parse_status(&out);
        assert_eq!(s.untracked, 2);
        assert_eq!((s.staged, s.unstaged), (0, 0));
    }

    #[test]
    fn unmerged_entries_are_conflicts() {
        let out = format!(
            "{CLEAN}u UU N... 100644 100644 100644 100644 a1 b2 c3 src/conflict.rs\n\
             u UU N... 100644 100644 100644 100644 a1 b2 c3 src/other.rs\n"
        );
        let s = parse_status(&out);
        assert_eq!(s.conflicts, 2);
        assert!(!s.is_clean());
    }

    #[test]
    fn a_path_with_spaces_does_not_shift_the_columns() {
        let out = format!("{CLEAN}1 .M N... 100644 100644 100644 abc123 def456 my notes/a b.md\n");
        let s = parse_status(&out);
        assert_eq!((s.staged, s.unstaged), (0, 1));
    }

    #[test]
    fn a_realistic_mixed_status_adds_up() {
        let out = "\
# branch.oid 3f9c1e0b7a2d5148e6c0b9f3a7d215c84e0b6d92
# branch.head feature/rate-limits
# branch.upstream origin/feature/rate-limits
# branch.ab +1 -0
1 M. N... 100644 100644 100644 aaa bbb Cargo.toml
1 .M N... 100644 100644 100644 ccc ddd src/lib.rs
1 MM N... 100644 100644 100644 eee fff src/main.rs
? scratch.txt
";
        let s = parse_status(out);
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicts),
            (2, 2, 1, 0)
        );
        assert_eq!(s.ahead, 1);
    }

    #[test]
    fn empty_output_parses_to_the_default_snapshot() {
        assert_eq!(parse_status(""), Snapshot::default());
    }

    #[test]
    fn log_line_splits_on_the_unit_separator() {
        let line = "3f9c1e0b\u{1f}add rate limit headers\u{1f}28 hours ago\n";
        let commit = parse_log_line(line).unwrap();
        assert_eq!(commit.short_id, "3f9c1e0b");
        assert_eq!(commit.subject, "add rate limit headers");
        assert_eq!(commit.age, "28h");
    }

    #[test]
    fn a_subject_containing_unicode_and_punctuation_is_preserved() {
        let subject = "✨ ship the 2.0 rewrite ✨ (#420) — with a, comma";
        let line = format!("a1b2c3d4\u{1f}{subject}\u{1f}35 hours ago");
        let commit = parse_log_line(&line).unwrap();
        assert_eq!(commit.subject, subject);
    }

    #[test]
    fn a_repo_with_no_commits_yields_no_head() {
        assert_eq!(parse_log_line(""), None);
        assert_eq!(parse_log_line("\n"), None);
    }

    #[test]
    fn relative_times_are_compacted() {
        let cases = [
            ("28 hours ago", "28h"),
            ("1 hour ago", "1h"),
            ("45 seconds ago", "45s"),
            ("3 minutes ago", "3m"),
            ("5 days ago", "5d"),
            ("2 weeks ago", "2w"),
            ("7 months ago", "7mo"),
            ("1 year ago", "1y"),
            ("2 years, 3 months ago", "2y"),
        ];
        for (input, want) in cases {
            assert_eq!(compact_relative_time(input), want, "input: {input}");
        }
    }

    #[test]
    fn unrecognised_relative_times_pass_through_unchanged() {
        assert_eq!(compact_relative_time("in the future"), "in the future");
        assert_eq!(compact_relative_time(""), "");
        assert_eq!(compact_relative_time("some day"), "some day");
    }
}
