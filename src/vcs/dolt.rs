//! The dolt backend.
//!
//! Dolt borrows git's vocabulary — branches, commits, merges, remotes — but the
//! resemblance stops at the command line: there is no `rev-parse`, `dolt
//! status` has no porcelain format, and `.dolt` holds a database rather than
//! refs and marker files. What dolt does offer is SQL, so every reading here
//! comes from the documented `dolt_*` system tables via `dolt sql -r json`.
//!
//! As in [`super::git`], running the command and parsing its output are kept
//! apart: the parsers below are pure `&str -> value` functions, tested against
//! captured dolt output with no database on disk.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use crate::error::{Error, Result};
use crate::registry::VcsKind;
use crate::vcs::{
    Branch, Change, Commit, Detail, Distance, FileChange, LOG_LIMIT, RepoState, Snapshot, Stash,
    Vcs,
};

/// Everything the dashboard needs that does not depend on knowing the upstream,
/// in one round trip.
///
/// Two details are easy to get wrong. The age is measured against
/// `utc_timestamp()` rather than `now()`, because `dolt_log.date` is UTC while
/// `now()` follows the session's time zone — mixing them makes every age wrong
/// by the machine's offset. And `dolt_log`'s first row is HEAD: it walks the
/// commit graph backwards from there, so `limit 1` needs no `order by`.
const SNAPSHOT_QUERY: &str = "\
select
  active_branch() as branch,
  (select remote from dolt_branches
     where name = active_branch() and remote != '') as upstream_remote,
  (select branch from dolt_branches
     where name = active_branch() and remote != '') as upstream_branch,
  (select count(*) from dolt_status where staged = 1) as staged,
  (select count(*) from dolt_status
     where staged = 0 and status not in ('new table', 'conflict')) as unstaged,
  (select count(*) from dolt_status
     where staged = 0 and status = 'new table') as untracked,
  (select count(*) from dolt_status where status = 'conflict') as conflicts,
  (select count(*) from dolt_stashes) as stashes,
  (select is_merging from dolt_merge_status) as merging,
  (select commit_hash from dolt_log limit 1) as head_id,
  (select message from dolt_log limit 1) as head_subject,
  (select timestampdiff(second, date, utc_timestamp()) from dolt_log limit 1) as head_age";

/// Where each reading lands in the stream of documents [`detail_sql`] returns.
const DOC_SNAPSHOT: usize = 0;
const DOC_STATUS: usize = 1;
const DOC_LOG: usize = 2;
const DOC_STASHES: usize = 3;
const DOC_BRANCHES: usize = 4;

/// Everything the detail view reads, as one multi-statement invocation.
///
/// One invocation rather than five. `dolt sql` accepts several statements and
/// answers with one JSON document per statement, back to back — and the spawn
/// is what costs: four invocations of a trivial query measured 486ms against
/// 140ms for one carrying all four. This runs while somebody is waiting for a
/// pane to fill in.
///
/// The statement order is the `DOC_*` constants above; adding one means adding
/// an index, and `a_detail_reading_maps_each_document_to_its_reading` fails if
/// the two fall out of step.
fn detail_sql() -> String {
    [
        SNAPSHOT_QUERY,
        "select table_name, staged, status from dolt_status",
        // `dolt_log`'s first row is HEAD and it walks backwards from there, so
        // a bare `limit` already means "the most recent N".
        &format!(
            "select commit_hash, message,
               timestampdiff(second, date, utc_timestamp()) as age
             from dolt_log limit {LOG_LIMIT}"
        ),
        "select stash_id, commit_message from dolt_stashes",
        "select name, remote, branch, hash, latest_commit_message,
           timestampdiff(second, latest_commit_date, utc_timestamp()) as age
         from dolt_branches",
    ]
    .join(";\n")
}

/// Dolt commit hashes are 32 characters of base32; git abbreviates to a handful
/// of hex digits. Trimming to a comparable width keeps the two backends' commit
/// columns the same shape.
const SHORT_ID_LEN: usize = 8;

pub struct DoltVcs;

impl Vcs for DoltVcs {
    fn kind(&self) -> VcsKind {
        VcsKind::Dolt
    }

    fn discover(&self, path: &Path) -> Option<PathBuf> {
        let start = path.canonicalize().ok()?;
        start
            .ancestors()
            .find(|dir| is_database(dir))
            .map(Path::to_path_buf)
    }

    fn snapshot(&self, path: &Path) -> Result<Snapshot> {
        if !path.exists() {
            return Ok(Snapshot {
                state: RepoState::Missing,
                ..Snapshot::default()
            });
        }

        let out = query(path, "status query", SNAPSHOT_QUERY)?;
        let mut snapshot = parse_snapshot(&out).map_err(|source| Error::BadOutput {
            program: "dolt",
            source,
        })?;

        // Ahead/behind needs a second round trip: `dolt_log`'s range argument
        // has to name the upstream, and the query above is what tells us its
        // name. Failure is expected rather than exceptional here — the remote's
        // branch may never have been fetched, in which case dolt cannot resolve
        // the range and the counts stay at zero.
        let sql = snapshot
            .branch
            .as_deref()
            .zip(snapshot.upstream.as_deref())
            .map(|(branch, upstream)| ahead_behind_query(branch, upstream));

        if let Some(sql) = sql {
            let counts = query(path, "ahead/behind query", &sql)
                .ok()
                .and_then(|out| parse_ahead_behind(&out).ok());

            if let Some((ahead, behind)) = counts {
                snapshot.ahead = ahead;
                snapshot.behind = behind;
            }
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

        let out = query(path, "detail query", &detail_sql())?;
        let bad = |source| Error::BadOutput {
            program: "dolt",
            source,
        };

        let docs = split_documents(&out).map_err(bad)?;
        let mut detail = parse_detail(&docs).map_err(bad)?;

        fill_distances(path, &mut detail);
        Ok(detail)
    }

    fn exec(&self, path: &Path, args: &[OsString]) -> Result<ExitStatus> {
        Command::new("dolt")
            .current_dir(path)
            .args(args)
            // Inherited, not captured: this is what keeps the user's pager
            // (delta, less), colour detection and $EDITOR working.
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|source| Error::Spawn {
                program: "dolt",
                source,
            })
    }
}

/// True when `dir` holds a dolt database.
///
/// Keyed on the database's own files rather than on `.dolt` alone, because dolt
/// keeps its *global* configuration in `~/.dolt`: a bare existence check would
/// report the user's home directory as a repository.
fn is_database(dir: &Path) -> bool {
    let dolt = dir.join(".dolt");
    dolt.join("repo_state.json").is_file() || dolt.join("noms").is_dir()
}

/// Run one query and hand back its JSON, erroring if dolt exits non-zero.
///
/// `label` stands in for the SQL in any error message, and is not decoration.
/// These queries run to several hundred characters across a dozen lines, so
/// pasting one into the error puts a newline where the first line should be —
/// and `grit status` shows only that first line, which hid dolt's own
/// explanation behind a truncated `select`.
fn query(path: &Path, label: &str, sql: &str) -> Result<String> {
    let out = Command::new("dolt")
        .current_dir(path)
        .args(["sql", "-r", "json", "-q", sql])
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Spawn {
            program: "dolt",
            source,
        })?;

    if !out.status.success() {
        return Err(Error::CommandFailed {
            program: "dolt",
            args: format!("sql -q <{label}>"),
            status: out.status.to_string(),
            stderr: super::one_line(&without_echoed_query(
                &String::from_utf8_lossy(&out.stderr),
                sql,
            )),
        });
    }

    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `dolt sql -r json` wraps its result set in a `rows` array, and collapses an
/// empty result to `{}` — which is why the field needs a default.
#[derive(Debug, Deserialize)]
struct Rows<T> {
    #[serde(default)]
    rows: Vec<T>,
}

/// A value dolt may render as a number, a boolean or a string.
///
/// Which one depends on how the query reached the database. Run against it
/// directly, `count(*)` arrives as a JSON number and `is_merging` as a boolean.
/// But when a `dolt sql-server` is holding the database, `dolt sql` connects to
/// that server as a client over the MySQL wire protocol, and every value comes
/// back as a string instead: `"0"` rather than `0` or `false`. Both are
/// ordinary dolt, and a backend that handles only one breaks on whichever
/// machine happens to have a server running.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Wire {
    Bool(bool),
    Int(i64),
    Str(String),
}

impl Wire {
    fn as_i64(&self) -> std::result::Result<i64, String> {
        match self {
            Wire::Bool(flag) => Ok(i64::from(*flag)),
            Wire::Int(number) => Ok(*number),
            Wire::Str(text) => text
                .trim()
                .parse()
                .map_err(|_| format!("expected a number, got {text:?}")),
        }
    }
}

/// Read a count written either way. Absent keys never reach here — the struct's
/// `default` covers those — so anything unreadable is a real surprise and is
/// reported rather than quietly counted as zero.
fn wire_u32<'de, D>(deserializer: D) -> std::result::Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let number = Wire::deserialize(deserializer)?
        .as_i64()
        .map_err(D::Error::custom)?;

    Ok(number.clamp(0, i64::from(u32::MAX)) as u32)
}

fn wire_i64<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    Wire::deserialize(deserializer)?
        .as_i64()
        .map_err(D::Error::custom)
}

fn wire_bool<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(wire_i64(deserializer)? != 0)
}

/// The single row [`SNAPSHOT_QUERY`] produces.
///
/// Dolt omits null columns from its JSON altogether rather than writing them as
/// `null`, so a repo with no upstream has no `upstream_*` keys at all. Every
/// field therefore has to tolerate being absent.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SnapshotRow {
    branch: Option<String>,
    upstream_remote: Option<String>,
    upstream_branch: Option<String>,
    #[serde(deserialize_with = "wire_u32")]
    staged: u32,
    #[serde(deserialize_with = "wire_u32")]
    unstaged: u32,
    #[serde(deserialize_with = "wire_u32")]
    untracked: u32,
    #[serde(deserialize_with = "wire_u32")]
    conflicts: u32,
    #[serde(deserialize_with = "wire_u32")]
    stashes: u32,
    #[serde(deserialize_with = "wire_bool")]
    merging: bool,
    head_id: Option<String>,
    head_subject: Option<String>,
    #[serde(deserialize_with = "wire_i64")]
    head_age: i64,
}

/// Parse [`SNAPSHOT_QUERY`]'s output.
fn parse_snapshot(json: &str) -> serde_json::Result<Snapshot> {
    let row: SnapshotRow = serde_json::from_str::<Rows<SnapshotRow>>(json)?
        .rows
        .into_iter()
        .next()
        .unwrap_or_default();

    let head = row.head_id.as_deref().map(|id| Commit {
        short_id: id.chars().take(SHORT_ID_LEN).collect(),
        subject: subject_line(row.head_subject.as_deref().unwrap_or_default()).to_string(),
        age: compact_age(row.head_age),
    });

    Ok(Snapshot {
        // Dolt has no detached HEAD — it refuses to check out a bare commit —
        // so `active_branch()` always names a branch.
        branch: row.branch,
        upstream: row
            .upstream_remote
            .zip(row.upstream_branch)
            .map(|(remote, branch)| format!("{remote}/{branch}")),
        // Filled in by the caller's second query.
        ahead: 0,
        behind: 0,
        staged: row.staged,
        unstaged: row.unstaged,
        untracked: row.untracked,
        conflicts: row.conflicts,
        stashes: row.stashes,
        head,
        // Dolt records only whether a merge is under way. There is no rebase,
        // cherry-pick, revert or bisect state to read — `dolt rebase` keeps its
        // progress on a temporary branch rather than in a system table — so the
        // remaining `RepoState` variants are unreachable for this backend.
        state: if row.merging || row.conflicts > 0 {
            RepoState::Merging
        } else {
            RepoState::Normal
        },
    })
}

/// Split the answer to a multi-statement invocation into one document per
/// statement.
///
/// Splitting on newlines would work today and break the first time dolt
/// pretty-prints a result set; the stream deserialiser reads the values as
/// values, whatever whitespace is between them.
fn split_documents(out: &str) -> serde_json::Result<Vec<serde_json::Value>> {
    serde_json::Deserializer::from_str(out)
        .into_iter::<serde_json::Value>()
        .collect()
}

/// The rows of one document, or none at all when the statement produced no
/// result set.
// `Default` comes from `Rows`'s `#[serde(default)]`, which serde's derive
// turns into a bound on the row type as well as on the vector.
fn rows_at<T: serde::de::DeserializeOwned + Default>(
    docs: &[serde_json::Value],
    index: usize,
) -> serde_json::Result<Vec<T>> {
    match docs.get(index) {
        Some(doc) => Ok(serde_json::from_value::<Rows<T>>(doc.clone())?.rows),
        None => Ok(Vec::new()),
    }
}

/// One row of `dolt_status`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StatusRow {
    table_name: String,
    #[serde(deserialize_with = "wire_bool")]
    staged: bool,
    status: String,
}

/// One row of `dolt_log`, for the commit list rather than for HEAD alone.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LogRow {
    commit_hash: Option<String>,
    message: Option<String>,
    #[serde(deserialize_with = "wire_i64")]
    age: i64,
}

/// One row of `dolt_stashes`. There is no timestamp column to read.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct StashRow {
    stash_id: Option<String>,
    commit_message: Option<String>,
}

/// One row of `dolt_branches`. `remote` and `branch` name the upstream and are
/// the empty string — not null, and not absent — when there is not one.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BranchRow {
    name: Option<String>,
    remote: Option<String>,
    branch: Option<String>,
    hash: Option<String>,
    latest_commit_message: Option<String>,
    #[serde(deserialize_with = "wire_i64")]
    age: i64,
}

/// Assemble a [`Detail`] from the documents [`detail_sql`] produced.
fn parse_detail(docs: &[serde_json::Value]) -> serde_json::Result<Detail> {
    let snapshot = match docs.get(DOC_SNAPSHOT) {
        Some(doc) => parse_snapshot(&doc.to_string())?,
        None => Snapshot::default(),
    };

    let files = parse_files(rows_at::<StatusRow>(docs, DOC_STATUS)?);

    let commits = rows_at::<LogRow>(docs, DOC_LOG)?
        .into_iter()
        .filter_map(|row| {
            row.commit_hash.map(|id| Commit {
                short_id: id.chars().take(SHORT_ID_LEN).collect(),
                subject: subject_line(row.message.as_deref().unwrap_or_default()).to_string(),
                age: compact_age(row.age),
            })
        })
        .collect();

    let stashes = rows_at::<StashRow>(docs, DOC_STASHES)?
        .into_iter()
        .map(|row| Stash {
            id: row.stash_id.unwrap_or_default(),
            message: subject_line(row.commit_message.as_deref().unwrap_or_default()).to_string(),
            // `dolt_stashes` carries no timestamp. Left empty rather than
            // filled with the stashed commit's age, which is the age of the
            // work and not of the stash — a plausible number in the wrong
            // column is worse than a blank one.
            age: String::new(),
        })
        .collect();

    let current = snapshot.branch.clone().unwrap_or_default();
    let branches = rows_at::<BranchRow>(docs, DOC_BRANCHES)?
        .into_iter()
        .filter_map(|row| {
            let name = row.name?;
            let upstream = match (row.remote.as_deref(), row.branch.as_deref()) {
                (Some(remote), Some(branch)) if !remote.is_empty() && !branch.is_empty() => {
                    Some(format!("{remote}/{branch}"))
                }
                _ => None,
            };
            Some(Branch {
                head: name == current,
                name,
                upstream,
                // Filled in by `fill_distances`, which needs a range query
                // per branch — not something `dolt_branches` can answer on
                // its own. Until then it is unmeasured, and that is a
                // different claim from being level.
                distance: None,
                tip: row.hash.map(|id| Commit {
                    short_id: id.chars().take(SHORT_ID_LEN).collect(),
                    subject: subject_line(row.latest_commit_message.as_deref().unwrap_or_default())
                        .to_string(),
                    age: compact_age(row.age),
                }),
            })
        })
        .collect();

    Ok(Detail {
        snapshot,
        files,
        commits,
        stashes,
        branches,
    })
}

/// Fold `dolt_status`'s rows into one entry per table.
///
/// Dolt lists the staged and the unstaged side of a table as two rows where
/// git puts both on one line, and the detail view shows one row per path
/// either way.
fn parse_files(rows: Vec<StatusRow>) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();

    for row in rows {
        let change = status_change(&row.status, row.staged);
        let (path, origin) = split_rename(&row.table_name);

        let index = match files.iter().position(|file| file.path == path) {
            Some(index) => index,
            None => {
                files.push(FileChange {
                    path,
                    origin: None,
                    staged: None,
                    unstaged: None,
                });
                files.len() - 1
            }
        };

        let file = &mut files[index];
        if row.staged {
            file.staged = Some(change);
        } else {
            file.unstaged = Some(change);
        }
        if origin.is_some() {
            file.origin = origin;
        }
    }

    files
}

/// Read one `dolt_status.status` value as a [`Change`].
///
/// The same word means different things on the two sides: an unstaged `new
/// table` is a table dolt has never been told about, which is what git calls
/// untracked. [`SNAPSHOT_QUERY`] counts it that way too, and the two have to
/// agree or the detail view contradicts the header above it.
fn status_change(status: &str, staged: bool) -> Change {
    match status {
        "new table" if !staged => Change::Untracked,
        "new table" => Change::Added,
        "deleted" => Change::Deleted,
        "renamed" => Change::Renamed,
        "conflict" => Change::Conflicted,
        // `modified`, and whatever a later dolt adds.
        _ => Change::Modified,
    }
}

/// Dolt spells a rename by putting both names in the one column:
/// `sprockets -> widgets2`.
fn split_rename(table: &str) -> (String, Option<String>) {
    match table.split_once(" -> ") {
        Some((from, to)) => (to.to_string(), Some(from.to_string())),
        None => (table.to_string(), None),
    }
}

/// Count each branch against its upstream.
///
/// A second round trip for the same reason [`DoltVcs::snapshot`] needs one:
/// `dolt_log`'s range argument has to name the upstream, and the query that
/// names it is the one that just returned.
///
/// Batched first, then one at a time if that fails. The batch is what makes
/// this a single spawn in the ordinary case, but `dolt sql` stops at the first
/// statement that errors and exits non-zero, and an upstream that has never
/// been fetched is exactly such an error — so one unfetched branch would
/// otherwise cost every other branch its reading too. A branch that cannot be
/// counted keeps `distance: None`, which renders as nothing rather than as a
/// tick.
fn fill_distances(path: &Path, detail: &mut Detail) {
    let queries: Vec<(usize, String)> = detail
        .branches
        .iter()
        .enumerate()
        .filter_map(|(index, branch)| {
            let upstream = branch.upstream.as_deref()?;
            Some((index, ahead_behind_query(&branch.name, upstream)))
        })
        .collect();

    if queries.is_empty() {
        return;
    }

    let batched = queries
        .iter()
        .map(|(_, sql)| sql.as_str())
        .collect::<Vec<_>>()
        .join(";\n");

    let measured: Vec<Option<Distance>> = match run_distances(path, &batched) {
        // One document per statement, in order.
        Some(docs) if docs.len() == queries.len() => docs,
        // Short or missing: fall back to asking for them one by one, so the
        // branches that *can* be counted still are.
        _ => queries
            .iter()
            .map(|(_, sql)| run_distances(path, sql).and_then(|d| d.into_iter().next().flatten()))
            .collect(),
    };

    for ((index, _), distance) in queries.iter().zip(measured) {
        let Some(distance) = distance else {
            continue;
        };

        let branch = &mut detail.branches[*index];
        branch.distance = Some(distance);

        // The checked-out branch is the one the header reads from, and
        // `parse_snapshot` left its counts at zero for this to fill in.
        if branch.head {
            detail.snapshot.ahead = distance.ahead;
            detail.snapshot.behind = distance.behind;
        }
    }
}

/// Run one or more ahead/behind statements, one [`Distance`] per document.
///
/// `None` for the whole call when the invocation failed; `None` for an
/// individual document when it could not be read as a pair of counts.
fn run_distances(path: &Path, sql: &str) -> Option<Vec<Option<Distance>>> {
    let out = query(path, "ahead/behind query", sql).ok()?;
    let docs = split_documents(&out).ok()?;

    Some(
        docs.iter()
            .map(|doc| {
                parse_ahead_behind(&doc.to_string())
                    .ok()
                    .map(|(ahead, behind)| Distance { ahead, behind })
            })
            .collect(),
    )
}

/// The subject of a commit message: its first line.
///
/// `dolt_log.message` is the *whole* message where git's `%s` is the subject
/// alone, so this is where the two backends are made to agree. Without it the
/// subject column carries a commit body, newlines and all, and the table comes
/// apart around it — the row after the body lands in the wrong column, and in
/// the shell preview zsh is left counting rows that are not where it thinks.
fn subject_line(message: &str) -> &str {
    message.lines().next().unwrap_or_default().trim_end()
}

/// Count the commits on each side of the upstream.
///
/// `upstream` is the display form, `origin/main`; dolt's ref for it lives under
/// `remotes/`, hence the prefix.
fn ahead_behind_query(branch: &str, upstream: &str) -> String {
    let ahead = sql_literal(&format!("remotes/{upstream}..{branch}"));
    let behind = sql_literal(&format!("{branch}..remotes/{upstream}"));

    format!(
        "select (select count(*) from dolt_log({ahead})) as ahead, \
         (select count(*) from dolt_log({behind})) as behind"
    )
}

fn parse_ahead_behind(json: &str) -> serde_json::Result<(u32, u32)> {
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct AheadBehind {
        #[serde(deserialize_with = "wire_u32")]
        ahead: u32,
        #[serde(deserialize_with = "wire_u32")]
        behind: u32,
    }

    let row: AheadBehind = serde_json::from_str::<Rows<AheadBehind>>(json)?
        .rows
        .into_iter()
        .next()
        .unwrap_or_default();

    Ok((row.ahead, row.behind))
}

/// Drop the copy of the query that dolt echoes back inside its own error.
///
/// Dolt reports a failure as `error on line 1 for query <the entire SQL>: <what
/// actually went wrong>`. Left in, that echo is several hundred characters of
/// noise standing between the reader and the last four words, which are the
/// only part that says anything.
fn without_echoed_query(stderr: &str, sql: &str) -> String {
    stderr.replace(sql, "…")
}

/// Quote a value for use as a SQL string literal.
///
/// Branch names are the repository's data rather than ours, and dolt does allow
/// a `'` in one, so this is escaping and not decoration.
fn sql_literal(value: &str) -> String {
    // Dolt follows MySQL, where a backslash escapes inside a string literal —
    // so it has to be doubled before the quote is.
    let escaped = value.replace('\\', r"\\").replace('\'', "''");
    format!("'{escaped}'")
}

/// Turn an age in seconds into something that fits a column: `7_320` → `2h`.
///
/// The unit vocabulary and the points at which it switches are git's, so rows
/// from the two backends read alike. Unlike git, which hands out a ready-made
/// phrase, dolt gives us a timestamp and leaves the arithmetic to us.
pub fn compact_age(seconds: i64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    /// The average Gregorian month, which is how git rounds one too.
    const MONTH: u64 = 2_629_746;
    const YEAR: u64 = 12 * MONTH;

    // A commit dated in the future is clock skew rather than information.
    let seconds = seconds.max(0) as u64;

    let (count, suffix) = match seconds {
        s if s < MINUTE => (s, "s"),
        s if s < HOUR => (s / MINUTE, "m"),
        s if s < DAY => (s / HOUR, "h"),
        s if s < 14 * DAY => (s / DAY, "d"),
        s if s < 10 * WEEK => (s / WEEK, "w"),
        s if s < YEAR => (s / MONTH, "mo"),
        s => (s / YEAR, "y"),
    };

    format!("{count}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A freshly initialised repo: one commit, no remote. Captured verbatim
    /// from `dolt sql -r json`, including its omission of the null columns.
    const FRESH: &str = r#"{"rows": [{"branch":"main","conflicts":0,"head_age":118,"head_id":"2mfsiihpci43vta4n53viivcmlvetpgn","head_subject":"Initialize data repository","merging":false,"staged":0,"stashes":0,"unstaged":0,"untracked":0}]}"#;

    /// A repo tracking `origin/main` with work in every state at once.
    const BUSY: &str = r#"{"rows": [{"branch":"main","conflicts":0,"head_age":287,"head_id":"c976njr9fgtc4kidr0hf7gqf77dnb6ol","head_subject":"add gone","merging":false,"staged":1,"stashes":0,"unstaged":3,"untracked":1,"upstream_branch":"main","upstream_remote":"origin"}]}"#;

    /// A reading taken while a `dolt sql-server` held the database: identical
    /// query, but every number arrives as a string over the wire protocol.
    /// Captured from a real server-backed repo.
    const SERVER_MODE: &str = r#"{"rows": [{"branch":"ingress/auslci-v48-2026","conflicts":"0","head_age":"2262","head_id":"he83ho5oqni9iacrlkogmcufcof9kdml","head_subject":"Add 2008 emission factors","merging":"0","staged":"0","stashes":"0","unstaged":"0","untracked":"0","upstream_branch":"ingress/auslci-v48-2026","upstream_remote":"origin"}]}"#;

    /// Stopped in the middle of a merge with one table unresolved.
    const CONFLICTED: &str = r#"{"rows": [{"branch":"main","conflicts":1,"head_age":207,"head_id":"llv5ak83i3c2ah4ce8gid07l8tgshbc9","head_subject":"main sets 2","merging":true,"staged":0,"stashes":0,"unstaged":0,"untracked":0}]}"#;

    /// `dolt_status` in every shape it produces, captured verbatim. Note that
    /// one table appears twice — staged as a new table, unstaged as deleted —
    /// and that a rename puts both names in the one column.
    const STATUS: &str = r#"{"rows": [{"staged":1,"status":"new table","table_name":"gadgets"},{"staged":1,"status":"new table","table_name":"sprockets"},{"staged":0,"status":"deleted","table_name":"gadgets"},{"staged":0,"status":"renamed","table_name":"sprockets -> widgets2"},{"staged":0,"status":"modified","table_name":"widgets"}]}"#;

    const STASHES: &str = r#"{"rows": [{"branch":"main","commit_message":"second commit with a much longer subject line\nand a body paragraph that must never reach a table cell","hash":"eiejsd2eb9mbt2lo1ulqncqir4ivjuok","name":"dolt-cli","stash_id":"stash@{0}"}]}"#;

    const BRANCHES: &str = r#"{"rows": [{"age":69,"branch":"","dirty":false,"hash":"eiejsd2eb9mbt2lo1ulqncqir4ivjuok","latest_commit_message":"second commit\nwith a body","name":"feature/thing","remote":""},{"age":69,"branch":"main","dirty":true,"hash":"eiejsd2eb9mbt2lo1ulqncqir4ivjuok","latest_commit_message":"second commit\nwith a body","name":"main","remote":"origin"}]}"#;

    const DETAIL_LOG: &str = r#"{"rows": [{"age":0,"commit_hash":"eiejsd2eb9mbt2lo1ulqncqir4ivjuok","committer":"T","message":"second commit with a much longer subject line\nand a body paragraph that must never reach a table cell"},{"age":1,"commit_hash":"2q5i8s10o2a12dg0t3gb940pck9om0ik","committer":"T","message":"Initialize data repository"}]}"#;

    /// An empty result set. Dolt drops the `rows` key altogether rather than
    /// writing an empty array, which is what `Rows`'s default is for.
    const EMPTY: &str = "{}";

    fn detail_of(status: &str, log: &str, stashes: &str, branches: &str) -> Detail {
        let joined = format!("{FRESH}\n{status}\n{log}\n{stashes}\n{branches}");
        parse_detail(&split_documents(&joined).expect("fixtures are valid json"))
            .expect("fixtures deserialise")
    }

    #[test]
    fn a_multi_statement_answer_splits_into_one_document_per_statement() {
        let joined = format!("{FRESH}\n{STATUS}\n{EMPTY}");
        assert_eq!(split_documents(&joined).unwrap().len(), 3);
    }

    #[test]
    fn a_detail_reading_maps_each_document_to_its_reading() {
        let detail = detail_of(STATUS, DETAIL_LOG, STASHES, BRANCHES);
        assert_eq!(detail.snapshot.branch.as_deref(), Some("main"));
        assert_eq!(detail.files.len(), 4);
        assert_eq!(detail.commits.len(), 2);
        assert_eq!(detail.stashes.len(), 1);
        assert_eq!(detail.branches.len(), 2);
    }

    #[test]
    fn the_two_sides_of_one_table_fold_into_one_row() {
        // `gadgets` is staged as a new table and deleted in the working set;
        // git puts both on one line and so does the detail view.
        let detail = detail_of(STATUS, EMPTY, EMPTY, EMPTY);
        let gadgets = detail
            .files
            .iter()
            .find(|f| f.path == "gadgets")
            .expect("gadgets is in the fixture");
        assert_eq!(gadgets.staged, Some(Change::Added));
        assert_eq!(gadgets.unstaged, Some(Change::Deleted));
    }

    #[test]
    fn a_rename_is_split_out_of_the_one_column_dolt_puts_it_in() {
        let detail = detail_of(STATUS, EMPTY, EMPTY, EMPTY);
        let renamed = detail
            .files
            .iter()
            .find(|f| f.path == "widgets2")
            .expect("the rename is in the fixture");
        assert_eq!(renamed.origin.as_deref(), Some("sprockets"));
        assert_eq!(renamed.unstaged, Some(Change::Renamed));
    }

    #[test]
    fn an_unstaged_new_table_is_untracked_and_a_staged_one_is_added() {
        // The same word on the two sides means different things, and
        // SNAPSHOT_QUERY counts them the same way this reads them.
        assert_eq!(status_change("new table", false), Change::Untracked);
        assert_eq!(status_change("new table", true), Change::Added);
    }

    #[test]
    fn a_status_word_this_build_does_not_know_is_read_as_a_modification() {
        assert_eq!(status_change("something new", false), Change::Modified);
    }

    #[test]
    fn a_table_name_with_no_arrow_in_it_is_not_a_rename() {
        assert_eq!(split_rename("widgets"), ("widgets".to_string(), None));
    }

    #[test]
    fn a_commit_body_never_reaches_the_detail_view_either() {
        // The same trap `subject_line` exists for, one layer out: dolt's
        // message column is the whole message in the log, in the stash list
        // and in the branch list alike.
        let detail = detail_of(EMPTY, DETAIL_LOG, STASHES, BRANCHES);
        for subject in [
            detail.commits[0].subject.as_str(),
            detail.stashes[0].message.as_str(),
            detail.branches[0].tip.as_ref().unwrap().subject.as_str(),
        ] {
            assert!(
                !subject.contains('\n'),
                "a body reached a cell: {subject:?}"
            );
        }
    }

    #[test]
    fn a_commit_hash_is_abbreviated_the_way_the_dashboard_abbreviates_it() {
        let detail = detail_of(EMPTY, DETAIL_LOG, EMPTY, EMPTY);
        assert_eq!(detail.commits[0].short_id, "eiejsd2e");
        assert_eq!(detail.commits[0].short_id.len(), SHORT_ID_LEN);
    }

    #[test]
    fn a_stash_has_no_age_because_dolt_records_none() {
        let detail = detail_of(EMPTY, EMPTY, STASHES, EMPTY);
        assert_eq!(detail.stashes[0].id, "stash@{0}");
        assert!(detail.stashes[0].age.is_empty());
    }

    #[test]
    fn a_branch_upstream_is_joined_only_when_both_halves_are_there() {
        let detail = detail_of(EMPTY, EMPTY, EMPTY, BRANCHES);
        assert_eq!(detail.branches[0].upstream, None);
        assert_eq!(detail.branches[1].upstream.as_deref(), Some("origin/main"));
    }

    #[test]
    fn a_branch_starts_out_unmeasured_rather_than_level() {
        // `fill_distances` needs a second round trip to count these, and it
        // may not get one. Zero here would put a ✓ against every branch in a
        // repo whose upstreams have never been fetched.
        let detail = detail_of(EMPTY, EMPTY, EMPTY, BRANCHES);
        assert!(detail.branches.iter().all(|b| b.distance.is_none()));
    }

    #[test]
    fn the_checked_out_branch_is_the_one_the_snapshot_names() {
        let detail = detail_of(EMPTY, EMPTY, EMPTY, BRANCHES);
        assert!(!detail.branches[0].head, "feature/thing is not checked out");
        assert!(detail.branches[1].head, "main is");
    }

    #[test]
    fn an_empty_result_set_reads_as_nothing_rather_than_failing() {
        let detail = detail_of(EMPTY, EMPTY, EMPTY, EMPTY);
        assert!(detail.files.is_empty());
        assert!(detail.commits.is_empty());
        assert!(detail.stashes.is_empty());
        assert!(detail.branches.is_empty());
    }

    #[test]
    fn a_detail_reading_survives_a_server_stringifying_everything() {
        // The same trap as the snapshot's: with a `dolt sql-server` holding
        // the database, `staged` arrives as "1" and `age` as "69".
        let wired = STATUS.replace(r#""staged":1"#, r#""staged":"1""#);
        let detail = detail_of(&wired, EMPTY, EMPTY, EMPTY);
        assert_eq!(
            detail
                .files
                .iter()
                .find(|f| f.path == "sprockets")
                .and_then(|f| f.staged),
            Some(Change::Added)
        );
    }

    #[test]
    fn the_detail_query_carries_one_statement_per_document_index() {
        // The `DOC_*` constants are positions in a list built somewhere else,
        // and nothing but this ties the two together.
        let sql = detail_sql();
        assert_eq!(sql.matches(";\n").count() + 1, DOC_BRANCHES + 1);
        assert!(sql.contains(&format!("limit {LOG_LIMIT}")));
    }

    #[test]
    fn a_fresh_repo_is_clean_and_synced() {
        let s = parse_snapshot(FRESH).unwrap();
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert!(s.is_clean());
        assert!(s.is_synced());
        assert_eq!(s.state, RepoState::Normal);
    }

    #[test]
    fn a_repo_with_no_upstream_has_none() {
        // Dolt leaves the column out of the JSON rather than writing null, so
        // this asserts the parser tolerates an absent key.
        assert!(!FRESH.contains("upstream"));
        assert_eq!(parse_snapshot(FRESH).unwrap().upstream, None);
    }

    #[test]
    fn an_upstream_is_joined_into_remote_slash_branch_form() {
        let s = parse_snapshot(BUSY).unwrap();
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
    }

    #[test]
    fn staged_unstaged_and_untracked_are_counted_apart() {
        let s = parse_snapshot(BUSY).unwrap();
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicts),
            (1, 3, 1, 0)
        );
        assert!(!s.is_clean());
    }

    #[test]
    fn a_conflict_reports_the_repo_as_merging() {
        let s = parse_snapshot(CONFLICTED).unwrap();
        assert_eq!(s.conflicts, 1);
        assert_eq!(s.state, RepoState::Merging);
        assert!(!s.is_clean());
    }

    #[test]
    fn a_merge_with_nothing_conflicted_yet_still_reports_merging() {
        let out = CONFLICTED.replace(r#""conflicts":1"#, r#""conflicts":0"#);
        let s = parse_snapshot(&out).unwrap();
        assert_eq!(s.conflicts, 0);
        assert_eq!(s.state, RepoState::Merging);
    }

    #[test]
    fn the_head_commit_is_abbreviated_and_aged() {
        let head = parse_snapshot(BUSY).unwrap().head.unwrap();
        assert_eq!(head.short_id, "c976njr9");
        assert_eq!(head.subject, "add gone");
        assert_eq!(head.age, "4m");
    }

    /// Verbatim from the commit that put a body in the dashboard.
    #[test]
    fn only_the_first_line_of_a_commit_message_is_the_subject() {
        let message = "Move EXIOBASE FLAG and land management values to premium slots (API-9974)\n\
                       \nEXIOBASE FLAG and land management data is a licence add-on on top of a\n\
                       basic EXIOBASE 3.11 licence, so its values have to sit in indicator slots\n\
                       the API can gate separately from the rest of the dataset.";
        assert_eq!(
            subject_line(message),
            "Move EXIOBASE FLAG and land management values to premium slots (API-9974)"
        );
    }

    #[test]
    fn a_message_body_never_reaches_the_snapshot() {
        let out = BUSY.replace("add gone", "the subject\\n\\nand a body");
        let head = parse_snapshot(&out).unwrap().head.unwrap();
        assert_eq!(head.subject, "the subject");
        assert!(!head.subject.contains('\n'), "{:?}", head.subject);
    }

    #[test]
    fn a_one_line_message_is_left_alone() {
        assert_eq!(subject_line("just the subject"), "just the subject");
        assert_eq!(subject_line(""), "");
    }

    #[test]
    fn a_subject_containing_unicode_and_punctuation_is_preserved() {
        let subject = "✨ ship the 2.0 rewrite ✨ (#420) — with a, comma";
        let out = BUSY.replace("add gone", subject);
        let head = parse_snapshot(&out).unwrap().head.unwrap();
        assert_eq!(head.subject, subject);
    }

    #[test]
    fn stashes_are_read_from_the_stash_table() {
        let out = FRESH.replace(r#""stashes":0"#, r#""stashes":3"#);
        assert_eq!(parse_snapshot(&out).unwrap().stashes, 3);
    }

    #[test]
    fn an_empty_result_set_parses_to_the_default_snapshot() {
        // `dolt sql` prints a bare `{}` when a query returns no rows.
        assert_eq!(parse_snapshot("{}").unwrap(), Snapshot::default());
        assert_eq!(
            parse_snapshot(r#"{"rows": []}"#).unwrap(),
            Snapshot::default()
        );
    }

    #[test]
    fn output_that_is_not_json_is_an_error_rather_than_a_default() {
        assert!(parse_snapshot("Warning: something went sideways").is_err());
    }

    #[test]
    fn ahead_and_behind_are_read_from_the_counts() {
        let out = r#"{"rows": [{"ahead":2,"behind":7}]}"#;
        assert_eq!(parse_ahead_behind(out).unwrap(), (2, 7));
    }

    #[test]
    fn a_missing_ahead_behind_row_counts_as_zero() {
        assert_eq!(parse_ahead_behind("{}").unwrap(), (0, 0));
    }

    #[test]
    fn the_ahead_range_runs_from_the_upstream_to_the_branch() {
        // The classic way to get this wrong is to swap the two ends, which
        // silently reports "behind" as "ahead".
        let query = ahead_behind_query("main", "origin/main");
        assert!(
            query.contains("dolt_log('remotes/origin/main..main')) as ahead"),
            "{query}"
        );
        assert!(
            query.contains("dolt_log('main..remotes/origin/main')) as behind"),
            "{query}"
        );
    }

    #[test]
    fn a_branch_name_with_a_quote_cannot_break_out_of_the_literal() {
        // Dolt really does allow this: `dolt branch "we'ird"` succeeds.
        assert_eq!(sql_literal("we'ird"), "'we''ird'");
        assert_eq!(sql_literal(r"back\slash"), r"'back\\slash'");
        assert!(ahead_behind_query("we'ird", "origin/main").contains("..we''ird'"));
    }

    #[test]
    fn numbers_arriving_as_strings_are_read_the_same_way() {
        // What a running `dolt sql-server` does to every value in the row.
        let s = parse_snapshot(SERVER_MODE).unwrap();
        assert_eq!(s.branch.as_deref(), Some("ingress/auslci-v48-2026"));
        assert_eq!(
            s.upstream.as_deref(),
            Some("origin/ingress/auslci-v48-2026")
        );
        assert_eq!(
            (s.staged, s.unstaged, s.untracked, s.conflicts),
            (0, 0, 0, 0)
        );
        assert!(s.is_clean());
        assert_eq!(s.state, RepoState::Normal);
        assert_eq!(s.head.unwrap().age, "37m");
    }

    #[test]
    fn a_merge_flag_of_string_one_still_reports_merging() {
        let out = SERVER_MODE.replace(r#""merging":"0""#, r#""merging":"1""#);
        assert_eq!(parse_snapshot(&out).unwrap().state, RepoState::Merging);
    }

    #[test]
    fn counts_arriving_as_strings_are_counted() {
        let out = SERVER_MODE
            .replace(r#""staged":"0""#, r#""staged":"4""#)
            .replace(r#""stashes":"0""#, r#""stashes":"2""#);
        let s = parse_snapshot(&out).unwrap();
        assert_eq!((s.staged, s.stashes), (4, 2));
        assert!(!s.is_clean());
    }

    #[test]
    fn ahead_and_behind_arriving_as_strings_are_read_too() {
        let out = r#"{"rows": [{"ahead":"2","behind":"7"}]}"#;
        assert_eq!(parse_ahead_behind(out).unwrap(), (2, 7));
    }

    #[test]
    fn a_count_that_is_not_a_number_at_all_is_an_error() {
        // Better a visibly unreadable row than a confidently clean one.
        let out = SERVER_MODE.replace(r#""staged":"0""#, r#""staged":"lots""#);
        assert!(parse_snapshot(&out).is_err());
    }

    #[test]
    fn dolts_echo_of_the_query_is_stripped_from_its_error() {
        // Dolt quotes the whole statement back at us. What matters is the tail.
        let stderr = format!("error on line 1 for query {SNAPSHOT_QUERY}: no database selected");
        let cleaned = without_echoed_query(&stderr, SNAPSHOT_QUERY);
        assert_eq!(cleaned, "error on line 1 for query …: no database selected");
    }

    #[test]
    fn an_error_that_does_not_echo_the_query_is_left_alone() {
        let stderr = "error: database locked by another dolt process";
        assert_eq!(without_echoed_query(stderr, SNAPSHOT_QUERY), stderr);
    }

    /// The failure that made this necessary: the query is multi-line, `grit
    /// status` shows only an error's first line, and pasting the query in put a
    /// newline where the explanation should have been.
    #[test]
    fn a_query_failure_says_what_went_wrong_on_its_first_line() {
        let stderr = format!("error on line 1 for query {SNAPSHOT_QUERY}: no database selected");
        let message = crate::error::Error::CommandFailed {
            program: "dolt",
            args: "sql -q <status query>".to_string(),
            status: "exit status: 1".to_string(),
            stderr: crate::vcs::one_line(&without_echoed_query(&stderr, SNAPSHOT_QUERY)),
        }
        .to_string();

        let first_line = message.lines().next().unwrap();
        assert!(
            first_line.contains("no database selected"),
            "the cause never reached the first line: {first_line}"
        );
        assert!(!first_line.contains("select count(*)"), "{first_line}");
    }

    #[test]
    fn ages_are_compacted() {
        let cases = [
            (0, "0s"),
            (45, "45s"),
            (90, "1m"),
            (7_320, "2h"),
            (100_800, "1d"),
            (5 * 86_400, "5d"),
            (14 * 86_400, "2w"),
            (5 * 604_800, "5w"),
            (7 * 2_629_746, "7mo"),
            (31_556_952, "1y"),
            (2 * 31_556_952 + 3 * 2_629_746, "2y"),
        ];
        for (input, want) in cases {
            assert_eq!(compact_age(input), want, "input: {input}");
        }
    }

    #[test]
    fn a_commit_dated_in_the_future_is_treated_as_brand_new() {
        assert_eq!(compact_age(-500), "0s");
    }
}
