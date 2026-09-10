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
use crate::vcs::{
    Branch, Change, Commit, Detail, Distance, FileChange, LOG_LIMIT, RepoState, Snapshot, Stash,
    Vcs,
};

/// Field separator for `git log --format`. ASCII unit separator: cannot appear
/// in a commit subject, so subjects with spaces, tabs or unicode parse cleanly.
const SEP: char = '\u{1f}';

const LOG_FORMAT: &str = "--format=%h\u{1f}%s\u{1f}%cr";

/// `git status` as grit reads it.
///
/// `core.quotePath=false` matters only to the detail view, which is the one
/// thing that prints these paths rather than counting them: left at git's
/// default, a path with any non-ASCII in it arrives C-quoted, and the list
/// shows the literal characters `"caf\\303\\251.txt"`.
const STATUS_ARGS: &[&str] = &[
    "-c",
    "core.quotePath=false",
    "status",
    "--porcelain=v2",
    "--branch",
];

/// `git stash list` fields: `stash@{0}`, the message, the relative age.
const STASH_FORMAT: &str = "--format=%gd\u{1f}%gs\u{1f}%cr";

/// `git for-each-ref` fields, in the order [`parse_branch_list`] reads them.
///
/// `%1f` rather than `%x1f`: for-each-ref spells a hex escape with the digits
/// alone, and `%x1f` would put a literal `x` in the output.
const BRANCH_FORMAT: &str = concat!(
    "--format=",
    "%(refname:short)%1f",
    "%(upstream:short)%1f",
    "%(upstream:track)%1f",
    "%(objectname:short)%1f",
    "%(contents:subject)%1f",
    "%(committerdate:relative)%1f",
    "%(HEAD)"
);

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

        let status_out = capture(path, STATUS_ARGS)?;
        let mut snapshot = parse_status(&status_out).snapshot;

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

    fn detail(&self, path: &Path) -> Result<Detail> {
        if !path.exists() {
            return Ok(Detail {
                snapshot: Snapshot {
                    state: RepoState::Missing,
                    ..Snapshot::default()
                },
                ..Detail::default()
            });
        }

        let Status {
            mut snapshot,
            files,
        } = parse_status(&capture(path, STATUS_ARGS)?);

        // Each of the three below is one cheap call, and each is allowed to
        // fail into an empty list: a repo with no commits has no log, one with
        // no stashes has no `refs/stash`, and neither is a reason to refuse the
        // whole reading. `snapshot` takes the same view.
        let commits = capture(path, &["log", "-n", &LOG_LIMIT.to_string(), LOG_FORMAT])
            .map(|out| parse_log(&out))
            .unwrap_or_default();
        snapshot.head = commits.first().cloned();

        // The list is the count. `snapshot` spends a `rev-list` on the number
        // alone because that is all it shows; here it would be a second process
        // for something already in hand.
        let stashes = capture(path, &["stash", "list", STASH_FORMAT])
            .map(|out| parse_stash_list(&out))
            .unwrap_or_default();
        snapshot.stashes = stashes.len() as u32;

        let branches = capture(path, &["for-each-ref", BRANCH_FORMAT, "refs/heads"])
            .map(|out| parse_branch_list(&out))
            .unwrap_or_default();

        if let Some(state) = in_progress_state(path) {
            snapshot.state = state;
        } else if snapshot.conflicts > 0 {
            snapshot.state = RepoState::Merging;
        }

        Ok(Detail {
            snapshot,
            files,
            commits,
            stashes,
            branches,
        })
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
            stderr: super::one_line(&String::from_utf8_lossy(&out.stderr)),
        });
    }

    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Everything one `git status --porcelain=v2 --branch` says.
///
/// The counts and the per-path list come out of the same walk. The entry
/// grammar below is fiddly enough — three shapes, a tab inside one of them —
/// that a second copy of it to list with, beside the first to count with, is
/// a copy that would drift.
#[derive(Debug)]
struct Status {
    snapshot: Snapshot,
    files: Vec<FileChange>,
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
fn parse_status(out: &str) -> Status {
    let mut snapshot = Snapshot::default();
    let mut files = Vec::new();

    for line in out.lines() {
        let Some((tag, rest)) = split_once_ws(line) else {
            continue;
        };

        match tag {
            "#" => parse_branch_header(rest, &mut snapshot),
            "1" | "2" => {
                let Some((xy, after)) = split_once_ws(rest) else {
                    continue;
                };
                let mut sides = xy.chars();
                let staged = side_change(sides.next().unwrap_or('.'));
                let unstaged = side_change(sides.next().unwrap_or('.'));
                if staged.is_some() {
                    snapshot.staged += 1;
                }
                if unstaged.is_some() {
                    snapshot.unstaged += 1;
                }

                // Six fields sit between the code pair and the path; a rename
                // or copy adds the similarity score, and pairs the new path
                // with the old one after a TAB rather than a space.
                let skip = if tag == "1" { 6 } else { 7 };
                let Some(paths) = skip_fields(after, skip) else {
                    continue;
                };
                let (path, origin) = match paths.split_once('\t') {
                    Some((new, old)) => (new, Some(old.to_string())),
                    None => (paths, None),
                };

                files.push(FileChange {
                    path: path.to_string(),
                    origin,
                    staged,
                    unstaged,
                });
            }
            "u" => {
                snapshot.conflicts += 1;
                // Three stages rather than one, so an unmerged entry carries
                // two more modes and one more hash than an ordinary one.
                if let Some(path) = split_once_ws(rest).and_then(|(_, after)| skip_fields(after, 8))
                {
                    files.push(FileChange {
                        path: path.to_string(),
                        origin: None,
                        staged: Some(Change::Conflicted),
                        unstaged: Some(Change::Conflicted),
                    });
                }
            }
            "?" => {
                snapshot.untracked += 1;
                files.push(FileChange {
                    path: rest.to_string(),
                    origin: None,
                    staged: None,
                    unstaged: Some(Change::Untracked),
                });
            }
            _ => {}
        }
    }

    Status { snapshot, files }
}

/// One half of porcelain v2's `<XY>` pair, where `.` means "nothing changed on
/// this side".
///
/// Anything git spells that is not in the list is read as a modification: the
/// alternative is dropping the path from the list entirely, and something
/// changed there whether or not this build knows the letter for it. That also
/// keeps `Some`/`None` here exactly as wide as the `!= '.'` test the counts
/// used before there was a list to build.
fn side_change(code: char) -> Option<Change> {
    match code {
        '.' => None,
        'A' => Some(Change::Added),
        'D' => Some(Change::Deleted),
        'R' => Some(Change::Renamed),
        'C' => Some(Change::Copied),
        'T' => Some(Change::TypeChanged),
        _ => Some(Change::Modified),
    }
}

/// Drop `n` whitespace-separated fields and return what is left.
///
/// The remainder comes back whole rather than split again, because the field
/// it ends on is a path and a path may contain spaces.
fn skip_fields(mut rest: &str, n: usize) -> Option<&str> {
    for _ in 0..n {
        rest = split_once_ws(rest)?.1;
    }
    Some(rest)
}

/// Parse every line produced by [`LOG_FORMAT`], most recent first.
fn parse_log(out: &str) -> Vec<Commit> {
    out.lines().filter_map(parse_log_line).collect()
}

/// Parse `git stash list` in [`STASH_FORMAT`].
fn parse_stash_list(out: &str) -> Vec<Stash> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.split(SEP);
            let id = fields.next()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            Some(Stash {
                message: fields.next().unwrap_or_default().to_string(),
                age: compact_relative_time(fields.next().unwrap_or_default()),
                id,
            })
        })
        .collect()
}

/// Parse `git for-each-ref` in [`BRANCH_FORMAT`].
fn parse_branch_list(out: &str) -> Vec<Branch> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.split(SEP);
            let name = fields.next()?.trim().to_string();
            if name.is_empty() {
                return None;
            }

            let upstream = fields.next().unwrap_or_default().trim().to_string();
            let track = fields.next().unwrap_or_default();
            let short_id = fields.next().unwrap_or_default().trim().to_string();
            let subject = fields.next().unwrap_or_default().to_string();
            let age = compact_relative_time(fields.next().unwrap_or_default());
            // git marks the checked-out branch with `*` and every other one
            // with a space, so this field is present either way.
            let head = fields.next().unwrap_or_default().trim() == "*";

            Some(Branch {
                name,
                distance: (!upstream.is_empty()).then(|| parse_track(track)).flatten(),
                upstream: (!upstream.is_empty()).then_some(upstream),
                head,
                tip: (!short_id.is_empty()).then_some(Commit {
                    short_id,
                    subject,
                    age,
                }),
            })
        })
        .collect()
}

/// Read `%(upstream:track)`: `[ahead 2]`, `[behind 1]`, `[ahead 2, behind 1]`,
/// `[gone]`, or nothing at all.
///
/// Empty is level with the upstream — git writes nothing rather than
/// `[ahead 0, behind 0]` — so it is a real `Some(0, 0)`.
///
/// `[gone]` is `None`, and that distinction is the point of the return type.
/// The upstream ref has been deleted, so there is no distance to report; git
/// still names the upstream in `%(upstream:short)`, so a caller that read this
/// as zero would put a ✓ against a branch tracking something that is no longer
/// there.
fn parse_track(track: &str) -> Option<Distance> {
    let body = track.trim().trim_start_matches('[').trim_end_matches(']');
    if body == "gone" {
        return None;
    }

    let mut distance = Distance::default();
    for part in body.split(',') {
        let mut words = part.split_whitespace();
        match (words.next(), words.next()) {
            (Some("ahead"), Some(n)) => distance.ahead = n.parse().unwrap_or(0),
            (Some("behind"), Some(n)) => distance.behind = n.parse().unwrap_or(0),
            _ => {}
        }
    }

    Some(distance)
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

    /// Every entry shape porcelain v2 produces, captured verbatim from a real
    /// repository put into each state. The tab in the rename line is a real
    /// tab: git separates the new path from the old one with one, and a test
    /// that spelled it with spaces would pass while the parser was wrong.
    const DIRTY: &str = "\
# branch.oid b777c552d7c3bada129bcbb1f6179985786fb1dd
# branch.head main
# branch.upstream origin/main
# branch.ab +4 -0
1 MM N... 100644 100644 100644 5626abf0f72e58d7a153368ba57db4c673c0e171 292ff6938d747033475548d8c0588a90ed085a8b a.txt
1 .D N... 100644 100644 000000 abaddc0b9edd523c69166a2c9f3a9e31a4c873e3 abaddc0b9edd523c69166a2c9f3a9e31a4c873e3 gone.txt
2 R. N... 100644 100644 100644 9d80ddb4cc7365318accecc8f8084993ecf72e69 9d80ddb4cc7365318accecc8f8084993ecf72e69 R100 renamed.txt\told.txt
1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 d1dee6292a637e55a303e241872da540648a1a7a with space.txt
? c.txt
? deep/
";

    /// A conflicted merge. Three stages means two more modes and one more hash
    /// on the line than an ordinary entry carries.
    const CONFLICTED: &str = "\
# branch.oid 6069f139223a3138340854a7a2a3b26cb484782a
# branch.head main
u UU N... 100644 100644 100644 100644 df967b96a579e45a18b8251732d16804b2e56a55 ba2906d0666cf726c7eaadd2cd3db615dedfdf3a 2299c37978265a95cbe835a4b0f0bbf15aad5549 f.txt
";

    fn paths(out: &str) -> Vec<String> {
        parse_status(out)
            .files
            .into_iter()
            .map(|f| f.path)
            .collect()
    }

    #[test]
    fn the_file_list_and_the_counts_come_from_one_walk() {
        let status = parse_status(DIRTY);
        // Two staged (the MM and the rename, plus the add) against three
        // unstaged sides: whatever the list says, the header has to agree.
        assert_eq!(status.snapshot.staged, 3);
        assert_eq!(status.snapshot.unstaged, 2);
        assert_eq!(status.snapshot.untracked, 2);
        assert_eq!(status.files.len(), 6);
    }

    #[test]
    fn a_file_staged_and_modified_again_carries_both_sides() {
        let file = parse_status(DIRTY).files.remove(0);
        assert_eq!(file.path, "a.txt");
        assert_eq!(file.staged, Some(Change::Modified));
        assert_eq!(file.unstaged, Some(Change::Modified));
    }

    #[test]
    fn a_rename_keeps_the_path_it_came_from() {
        let renamed = parse_status(DIRTY)
            .files
            .into_iter()
            .find(|f| f.path == "renamed.txt")
            .expect("the rename is in the fixture");
        assert_eq!(renamed.origin.as_deref(), Some("old.txt"));
        assert_eq!(renamed.staged, Some(Change::Renamed));
        assert_eq!(renamed.unstaged, None);
    }

    #[test]
    fn a_path_with_a_space_in_it_survives_the_field_walk() {
        // The path is the last field and is not split again, which is the
        // whole reason `skip_fields` returns the remainder whole.
        assert!(paths(DIRTY).contains(&"with space.txt".to_string()));
    }

    #[test]
    fn an_untracked_directory_is_one_entry_not_its_contents() {
        // grit does not pass `-uall`, so git collapses a directory nobody has
        // added into a single row — and the count and the list agree on that.
        let status = parse_status(DIRTY);
        assert!(status.files.iter().any(|f| f.path == "deep/"));
        assert_eq!(status.snapshot.untracked, 2);
    }

    #[test]
    fn an_untracked_file_is_unstaged_and_nothing_else() {
        let untracked = parse_status(DIRTY)
            .files
            .into_iter()
            .find(|f| f.path == "c.txt")
            .expect("the untracked file is in the fixture");
        assert_eq!(untracked.staged, None);
        assert_eq!(untracked.unstaged, Some(Change::Untracked));
    }

    #[test]
    fn a_deletion_is_told_apart_from_a_modification() {
        let gone = parse_status(DIRTY)
            .files
            .into_iter()
            .find(|f| f.path == "gone.txt")
            .expect("the deletion is in the fixture");
        assert_eq!(gone.unstaged, Some(Change::Deleted));
    }

    #[test]
    fn an_unmerged_entry_is_conflicted_on_both_sides() {
        let status = parse_status(CONFLICTED);
        assert_eq!(status.snapshot.conflicts, 1);
        let file = &status.files[0];
        assert_eq!(file.path, "f.txt");
        assert_eq!(file.staged, Some(Change::Conflicted));
        assert_eq!(file.unstaged, Some(Change::Conflicted));
    }

    #[test]
    fn a_status_with_no_entries_lists_no_files() {
        assert!(parse_status(CLEAN).files.is_empty());
    }

    #[test]
    fn a_code_this_build_does_not_know_still_counts_and_still_lists() {
        // A letter git has not shipped yet must not silently drop the path,
        // and must not change the count the dashboard has always given.
        let out = DIRTY.replace("1 MM N...", "1 XM N...");
        let status = parse_status(&out);
        assert_eq!(status.snapshot.staged, 3);
        assert_eq!(status.files[0].staged, Some(Change::Modified));
    }

    #[test]
    fn the_log_reads_every_line_not_just_the_first() {
        let out = "abc1234\u{1f}first\u{1f}2 hours ago\ndef5678\u{1f}second\u{1f}3 days ago\n";
        let commits = parse_log(out);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short_id, "abc1234");
        assert_eq!(commits[1].age, "3d");
    }

    #[test]
    fn an_empty_log_is_no_commits_rather_than_one_blank_one() {
        assert!(parse_log("").is_empty());
        assert!(parse_log("\n").is_empty());
    }

    #[test]
    fn the_stash_list_is_read_newest_first() {
        let out = "\
stash@{0}\u{1f}On main: second stash\u{1f}16 seconds ago
stash@{1}\u{1f}On main: wip: fiddling with a\u{1f}2 days ago
";
        let stashes = parse_stash_list(out);
        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[0].id, "stash@{0}");
        assert_eq!(stashes[0].message, "On main: second stash");
        assert_eq!(stashes[1].age, "2d");
    }

    #[test]
    fn no_stashes_is_an_empty_list_not_a_blank_entry() {
        assert!(parse_stash_list("").is_empty());
    }

    #[test]
    fn a_branch_line_is_read_field_by_field() {
        let out = "main\u{1f}origin/main\u{1f}[ahead 2]\u{1f}5b38814\u{1f}local only 2\u{1f}3 hours ago\u{1f}*\n";
        let branches = parse_branch_list(out);
        assert_eq!(branches.len(), 1);

        let branch = &branches[0];
        assert_eq!(branch.name, "main");
        assert_eq!(branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!(branch.distance, distance(2, 0));
        assert!(branch.head);

        let tip = branch.tip.as_ref().expect("the branch has a commit");
        assert_eq!(tip.short_id, "5b38814");
        assert_eq!(tip.age, "3h");
    }

    #[test]
    fn a_branch_that_is_not_checked_out_is_not_marked() {
        let out = "side\u{1f}\u{1f}\u{1f}abc1234\u{1f}wip\u{1f}2 days ago\u{1f}\n";
        let branch = parse_branch_list(out).remove(0);
        assert!(!branch.head);
        assert_eq!(branch.upstream, None);
        // No upstream means nothing to measure against, not a measurement of
        // zero — the same distinction `[gone]` turns on.
        assert_eq!(branch.distance, None);
    }

    fn distance(ahead: u32, behind: u32) -> Option<Distance> {
        Some(Distance { ahead, behind })
    }

    #[test]
    fn the_tracking_field_is_read_in_all_the_shapes_git_writes_it() {
        // Empty is level, not unknown: git writes nothing rather than
        // `[ahead 0, behind 0]` for a branch that matches its upstream.
        assert_eq!(parse_track(""), distance(0, 0));
        assert_eq!(parse_track("[ahead 2]"), distance(2, 0));
        assert_eq!(parse_track("[behind 7]"), distance(0, 7));
        assert_eq!(parse_track("[ahead 2, behind 7]"), distance(2, 7));
    }

    #[test]
    fn an_upstream_that_is_gone_reports_no_distance_at_all() {
        // Not zero, which reads as "in sync". There is nothing left to be in
        // sync with — and git still names the upstream in `%(upstream:short)`,
        // so this is the only field that can say so.
        assert_eq!(parse_track("[gone]"), None);
    }

    #[test]
    fn a_branch_with_a_deleted_upstream_keeps_the_name_and_loses_the_distance() {
        let out =
            "main\u{1f}origin/main\u{1f}[gone]\u{1f}abc1234\u{1f}one\u{1f}2 days ago\u{1f}*\n";
        let branch = parse_branch_list(out).remove(0);
        assert_eq!(branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!(branch.distance, None);
    }

    #[test]
    fn clean_repo_has_no_counts() {
        let s = parse_status(CLEAN).snapshot;
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (0, 0));
        assert!(s.is_clean());
        assert!(s.is_synced());
    }

    #[test]
    fn ahead_and_behind_are_read_from_branch_ab() {
        let out = CLEAN.replace("+0 -0", "+3 -12");
        let s = parse_status(&out).snapshot;
        assert_eq!((s.ahead, s.behind), (3, 12));
        assert!(!s.is_synced());
    }

    #[test]
    fn detached_head_has_no_branch() {
        let out = CLEAN.replace("main", "(detached)");
        let s = parse_status(&out).snapshot;
        assert_eq!(s.branch, None);
    }

    #[test]
    fn a_branch_with_no_upstream_has_no_ab_line() {
        let out = "\
# branch.oid a1b2c3d4
# branch.head feature/thing
";
        let s = parse_status(out).snapshot;
        assert_eq!(s.branch.as_deref(), Some("feature/thing"));
        assert_eq!(s.upstream, None);
        assert!(s.is_synced());
    }

    #[test]
    fn a_slash_in_the_branch_name_survives() {
        let out = CLEAN.replace("head main", "head feature/rate-limits");
        assert_eq!(
            parse_status(&out).snapshot.branch.as_deref(),
            Some("feature/rate-limits")
        );
    }

    #[test]
    fn xy_first_column_counts_as_staged() {
        let out = format!("{CLEAN}1 M. N... 100644 100644 100644 abc123 def456 src/main.rs\n");
        let s = parse_status(&out).snapshot;
        assert_eq!((s.staged, s.unstaged), (1, 0));
        assert!(!s.is_clean());
    }

    #[test]
    fn xy_second_column_counts_as_unstaged() {
        let out = format!("{CLEAN}1 .M N... 100644 100644 100644 abc123 def456 src/main.rs\n");
        let s = parse_status(&out).snapshot;
        assert_eq!((s.staged, s.unstaged), (0, 1));
    }

    #[test]
    fn a_file_changed_on_both_sides_counts_once_each() {
        let out = format!("{CLEAN}1 MM N... 100644 100644 100644 abc123 def456 src/main.rs\n");
        let s = parse_status(&out).snapshot;
        assert_eq!((s.staged, s.unstaged), (1, 1));
    }

    #[test]
    fn renames_are_counted_like_ordinary_changes() {
        let out = format!(
            "{CLEAN}2 R. N... 100644 100644 100644 abc123 def456 R100 new/path.rs\u{0}old/path.rs\n"
        );
        let s = parse_status(&out).snapshot;
        assert_eq!((s.staged, s.unstaged), (1, 0));
    }

    #[test]
    fn untracked_and_ignored_are_told_apart() {
        let out = format!("{CLEAN}? notes.md\n? build/out.log\n! target/\n");
        let s = parse_status(&out).snapshot;
        assert_eq!(s.untracked, 2);
        assert_eq!((s.staged, s.unstaged), (0, 0));
    }

    #[test]
    fn unmerged_entries_are_conflicts() {
        let out = format!(
            "{CLEAN}u UU N... 100644 100644 100644 100644 a1 b2 c3 src/conflict.rs\n\
             u UU N... 100644 100644 100644 100644 a1 b2 c3 src/other.rs\n"
        );
        let s = parse_status(&out).snapshot;
        assert_eq!(s.conflicts, 2);
        assert!(!s.is_clean());
    }

    #[test]
    fn a_path_with_spaces_does_not_shift_the_columns() {
        let out = format!("{CLEAN}1 .M N... 100644 100644 100644 abc123 def456 my notes/a b.md\n");
        let s = parse_status(&out).snapshot;
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
        let s = parse_status(out).snapshot;
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicts),
            (2, 2, 1, 0)
        );
        assert_eq!(s.ahead, 1);
    }

    #[test]
    fn empty_output_parses_to_the_default_snapshot() {
        assert_eq!(parse_status("").snapshot, Snapshot::default());
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
