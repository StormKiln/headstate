//! Pull request links written since the last import, found without one
//! (#1557).
//!
//! # The gap this closes
//!
//! `claude_session_pr` is rewritten whole by the transcript import
//! (`store::import`), which runs once per app session and on Rescan. The
//! import has to stay the owner of that table -- it is the only pass that
//! can remove a link whose transcript was deleted -- but on its own it
//! leaves every PR opened after it unfindable until the next one. #1545
//! measured one PR in that gap on the owner's machine, and it is the PR a
//! user is most likely to look up: the one they just opened.
//!
//! # What this does instead of a second import
//!
//! A light, INCREMENTAL link read on the live pass's 60-second tick
//! (`commands::claude_live_pass`), over the corpus listing that pass
//! already takes. It reads only bytes the import did not:
//!
//! - A file with a cursor is read from its cursor to its current size.
//!   Claude Code appends, so after the first read this is just the new
//!   records.
//! - A file with no cursor is read from the start if it was modified at
//!   or after the last import began, and skipped if it was not: the
//!   import read all of it.
//! - A file smaller than its cursor was replaced, and is read again from
//!   the start.
//!
//! It only ADDS links (`INSERT OR IGNORE`, which keeps the earlier
//! `first_seen_at` the import may already hold). Removing links stays the
//! import's job.
//!
//! # Bounded
//!
//! [`BYTES_PER_PASS`] bytes per pass across all files, newest-modified
//! first, so the session a user was just in is read before older ones.
//! A cursor stops at the last COMPLETE line: a record Claude Code is
//! still writing is read whole on the next pass, never half-parsed and
//! skipped.
//!
//! # Partial is not nothing
//!
//! A pass that runs out of budget keeps everything it read: its links are
//! committed and every file's cursor advances as far as that file was
//! read, so the next pass resumes rather than starts over. The files it
//! did not reach are counted in [`Refreshed::deferred`].
//!
//! # Where the cursor lives
//!
//! In the `settings` table, as one JSON value under
//! [`crate::store::settings::keys::CLAUDE_PR_LINK_CURSOR`], not in a
//! table of its own: a migration would make a 7.9.2 database refuse to
//! open under 7.9.1, and the value only ever holds files modified since
//! the last import -- a handful, not the corpus. It is cleared by every
//! import (see [`reset`]), and until the first import has set it, a pass
//! does nothing at all: without an import to measure from, "modified
//! since" has no reference point, and guessing one would either skip
//! links or read the whole corpus.
//!
//! Nothing here writes to `~/.claude`. Transcripts are opened read-only.

use std::collections::BTreeMap;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::store::settings;

/// How many transcript bytes one pass may read.
///
/// 16 MiB. The import's `subagent::build` reads 0.86 GB with the same
/// per-line gates in 969 ms (measured, see `transcript::scan`), so this is
/// about 18 ms of reading at that rate -- an ESTIMATE from that
/// measurement, not a measurement of this pass. In steady state a pass
/// reads only what sessions appended in the last minute, far below it;
/// the bound is for the first pass after an import, which re-reads every
/// file modified since the import began from the start.
pub const BYTES_PER_PASS: u64 = 16 * 1024 * 1024;

/// mtime slack when deciding a file predates the import.
///
/// Filesystem mtimes are coarser than the clock on some platforms, and a
/// file appended in the same second the import started must be read
/// again rather than assumed covered. Over-reading is harmless -- every
/// insert is idempotent -- while under-reading loses a link until the
/// next import.
const MTIME_SLACK_MS: i64 = 2_000;

/// Where the incremental read has got to, persisted between passes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// When the last import BEGAN, in ms since the epoch. A file last
    /// modified before this was read whole by that import.
    pub since_ms: i64,
    /// Transcript path -> bytes read, always at a line boundary.
    pub offsets: BTreeMap<String, u64>,
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refreshed {
    /// No import has set a cursor yet, so nothing was read.
    pub not_started: bool,
    /// Files read from, in whole or in part.
    pub files_read: usize,
    /// Transcript bytes consumed.
    pub bytes_read: u64,
    /// Links not already in the table.
    pub links_added: usize,
    /// Files with unread bytes the budget did not reach. They are read on
    /// a later pass; nothing is lost.
    pub deferred: usize,
    /// Files that could not be stat'ed or read, with why.
    pub unreadable: Vec<String>,
}

/// Start the cursor over from an import that began at `since_ms`.
///
/// Called AFTER the import committed. Anything a pass wrote before that
/// is forgotten, which is safe: a file modified since `since_ms` is read
/// again from the start, and every insert is idempotent. The reverse
/// order could keep a cursor for bytes whose links the import's rewrite
/// had just removed.
pub fn reset(conn: &Connection, since_ms: i64) -> Result<(), crate::store::StoreError> {
    settings::set(
        conn,
        settings::keys::CLAUDE_PR_LINK_CURSOR,
        &Cursor {
            since_ms,
            offsets: BTreeMap::new(),
        },
    )
}

/// One bounded pass over the listed corpus.
///
/// `scan` is a listing (`transcript::corpus`): only each session's `path`
/// and `session_id` are read off it.
pub fn refresh(
    conn: &mut Connection,
    scan: &super::Scan,
    budget: u64,
) -> Result<Refreshed, crate::store::StoreError> {
    let mut out = Refreshed::default();
    let Some(mut cursor) =
        settings::get::<Cursor>(conn, settings::keys::CLAUDE_PR_LINK_CURSOR).unwrap_or(None)
    else {
        out.not_started = true;
        return Ok(out);
    };

    // What each file needs reading from, newest-modified first.
    let mut todo: Vec<(i64, &super::Transcript, u64, u64)> = Vec::new();
    for t in &scan.sessions {
        let meta = match std::fs::metadata(&t.path) {
            Ok(m) => m,
            Err(e) => {
                out.unreadable
                    .push(format!("{}: could not read its size: {e}", t.path));
                continue;
            }
        };
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            // An unreadable mtime reads as "modified now": the safe
            // direction, for the reason `MTIME_SLACK_MS` gives.
            .unwrap_or(i64::MAX);
        let from = match cursor.offsets.get(&t.path) {
            Some(&o) if o == size => continue,
            Some(&o) if o < size => o,
            Some(_) => 0,
            None if mtime.saturating_add(MTIME_SLACK_MS) < cursor.since_ms => continue,
            None => 0,
        };
        todo.push((mtime, t, from, size));
    }
    todo.sort_by_key(|a| std::cmp::Reverse(a.0));

    let mut links = Vec::new();
    let mut left = budget;
    for (_, t, from, size) in todo {
        if left == 0 {
            out.deferred += 1;
            continue;
        }
        match read_from(Path::new(&t.path), &t.session_id, from, size, left, budget) {
            Ok(read) => {
                left = left.saturating_sub(read.consumed);
                out.bytes_read += read.consumed;
                out.files_read += 1;
                if read.to < size && left == 0 {
                    out.deferred += 1;
                }
                links.extend(read.links);
                cursor.offsets.insert(t.path.clone(), read.to);
            }
            Err(e) => out
                .unreadable
                .push(format!("{}: could not read it: {e}", t.path)),
        }
    }

    // A cursor for a file that is no longer listed would only grow the
    // value; the file cannot be read from again.
    let listed: std::collections::HashSet<&str> =
        scan.sessions.iter().map(|t| t.path.as_str()).collect();
    cursor.offsets.retain(|p, _| listed.contains(p.as_str()));

    // The links and the cursor that says they were read, in ONE
    // transaction: a cursor committed without its links would skip them
    // for good.
    let tx = conn.transaction()?;
    for l in &links {
        out.links_added += tx.execute(
            "INSERT OR IGNORE INTO claude_session_pr
                 (session_id, repo, number, url, first_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                l.session_id,
                l.repo,
                l.number as i64,
                l.url,
                l.first_seen_at
            ],
        )?;
    }
    settings::set(&tx, settings::keys::CLAUDE_PR_LINK_CURSOR, &cursor)?;
    tx.commit()?;
    Ok(out)
}

/// What reading one file found.
struct FileRead {
    links: Vec<super::subagent::PrLink>,
    /// Bytes taken out of the budget.
    consumed: u64,
    /// The new cursor: a line boundary.
    to: u64,
}

/// Read `path` from `from` towards `size`, taking at most `left` bytes.
///
/// `whole` is the full per-pass budget. A single line longer than that
/// could never be read whole by any pass, so it is stepped over instead:
/// the cursor moves past what was read, and the rest of that line is
/// read next time as unparseable noise up to its newline. Without this, a
/// file whose next line is 20 MB would pin its cursor forever.
fn read_from(
    path: &Path,
    session_id: &str,
    from: u64,
    size: u64,
    left: u64,
    whole: u64,
) -> std::io::Result<FileRead> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let limit = (size - from).min(left);
    let mut reader = std::io::BufReader::new(file.take(limit));
    let mut reader_links = super::subagent::LinkReader::default();
    let mut out = FileRead {
        links: Vec::new(),
        consumed: 0,
        to: from,
    };
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)? as u64;
        if n == 0 {
            break;
        }
        out.consumed += n;
        if buf.last() != Some(&b'\n') {
            // An incomplete line: the end of what Claude Code has written
            // so far, or the end of the budget. Left for the next pass --
            // unless it is one line bigger than any pass, see above.
            if out.to == from && n == whole {
                out.to = from + n;
            }
            break;
        }
        out.to += n;
        // A torn or non-UTF-8 line is not a failure of the file; the
        // import takes the same view.
        if let Ok(line) = std::str::from_utf8(&buf) {
            if let Some(link) = reader_links.line(line.trim_end_matches(['\n', '\r']), session_id) {
                out.links.push(link);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn link_line(session: &str, repo: &str, n: u64) -> String {
        format!(
            r#"{{"type":"pr-link","sessionId":"{session}","prNumber":{n},"prUrl":"https://example.invalid/{repo}/pull/{n}","prRepository":"{repo}","timestamp":"2026-09-11T12:00:00.000Z"}}"#
        )
    }

    fn filler() -> String {
        format!(r#"{{"type":"user","text":"{}"}}"#, "x".repeat(200))
    }

    fn append(path: &Path, lines: &[String]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// A corpus root with one project directory, and a database.
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("projects").join("-work-example");
        std::fs::create_dir_all(&project).unwrap();
        let conn = crate::store::open_db(&dir.path().join("t.db")).unwrap();
        (dir, project, conn)
    }

    fn linked(conn: &Connection, n: u64) -> Vec<String> {
        crate::claude::store::sessions_for_pr_number(conn, n)
            .unwrap()
            .into_iter()
            .map(|l| l.session_id)
            .collect()
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    /// A PR opened after the import is found by the next pass, without
    /// another import (#1557).
    #[test]
    fn a_link_written_after_the_import_is_found_by_the_next_pass() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("s1.jsonl");
        append(&t, &[filler(), link_line("s1", "acme/api", 7)]);

        // The import, as `claude_import_transcripts` runs it: the scan,
        // the rewrite, then the cursor reset.
        let since = now_ms();
        let scan = crate::claude::scan(&root);
        crate::claude::store::import(&mut conn, scan).unwrap();
        reset(&conn, since).unwrap();
        assert_eq!(linked(&conn, 7), ["s1"]);

        // The session opens PR 8 after the import.
        append(&t, &[filler(), link_line("s1", "acme/api", 8)]);
        assert!(
            linked(&conn, 8).is_empty(),
            "stale until something reads it"
        );

        let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 8), ["s1"], "the stale link is found");
        assert_eq!(got.links_added, 1, "PR 7 was already there");
        assert_eq!(got.deferred, 0);

        // A second pass with nothing new reads nothing.
        let again = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(again.bytes_read, 0, "the cursor holds between passes");
        assert_eq!(again.links_added, 0);
    }

    /// A file the import read whole, and untouched since, is not read.
    #[test]
    fn a_file_older_than_the_import_is_not_read_again() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("old.jsonl");
        append(&t, &[link_line("old", "acme/api", 3)]);
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&t)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();

        reset(&conn, now_ms()).unwrap();
        let got = refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(got.bytes_read, 0);
        assert!(linked(&conn, 3).is_empty(), "the import owns that file");
    }

    /// Before any import, a pass does nothing -- and says so.
    #[test]
    fn no_pass_runs_before_the_first_import() {
        let (dir, project, mut conn) = fixture();
        append(&project.join("s1.jsonl"), &[link_line("s1", "acme/api", 7)]);
        let got = refresh(
            &mut conn,
            &crate::claude::corpus(&dir.path().join("projects")),
            BYTES_PER_PASS,
        )
        .unwrap();
        assert!(got.not_started);
        assert!(linked(&conn, 7).is_empty());
    }

    /// Partial is not nothing: a pass that runs out of budget keeps the
    /// links it read and resumes where it stopped.
    #[test]
    fn a_pass_out_of_budget_keeps_what_it_found_and_resumes() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("s1.jsonl");
        let first = link_line("s1", "acme/api", 1);
        let second = link_line("s1", "acme/api", 2);
        append(&t, &[first.clone(), filler(), filler(), second]);
        reset(&conn, now_ms() - 60_000).unwrap();

        // Room for the first record and part of the filler, not the
        // second record.
        let budget = first.len() as u64 + 50;
        let got = refresh(&mut conn, &crate::claude::corpus(&root), budget).unwrap();
        assert_eq!(linked(&conn, 1), ["s1"], "what it read is kept");
        assert!(linked(&conn, 2).is_empty());
        assert_eq!(got.deferred, 1, "the unread rest is counted, not dropped");
        assert_eq!(got.bytes_read, budget);

        // The cursor stopped at the last complete line, so nothing is
        // half-read: the next passes finish the file.
        let mut guard = 0;
        while linked(&conn, 2).is_empty() {
            refresh(&mut conn, &crate::claude::corpus(&root), budget * 4).unwrap();
            guard += 1;
            assert!(guard < 10, "the passes must make progress");
        }
        assert_eq!(linked(&conn, 1), ["s1"]);
    }

    /// A record still being written is not consumed half-way.
    #[test]
    fn an_unfinished_line_is_left_for_the_next_pass() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        let t = project.join("s1.jsonl");
        let line = link_line("s1", "acme/api", 9);
        let (head, tail) = line.split_at(40);
        std::fs::write(&t, head).unwrap();
        reset(&conn, now_ms() - 60_000).unwrap();

        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert!(linked(&conn, 9).is_empty());

        append(&t, &[tail.to_string()]);
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        assert_eq!(linked(&conn, 9), ["s1"], "read whole once it was complete");
    }

    /// The pass adds; it never overwrites the import's earlier timestamp.
    #[test]
    fn an_existing_link_keeps_its_first_seen_time() {
        let (dir, project, mut conn) = fixture();
        let root = dir.path().join("projects");
        conn.execute(
            "INSERT INTO claude_session_pr (session_id, repo, number, url, first_seen_at)
             VALUES ('s1', 'acme/api', 7, 'u', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        append(&project.join("s1.jsonl"), &[link_line("s1", "acme/api", 7)]);
        reset(&conn, now_ms() - 60_000).unwrap();
        refresh(&mut conn, &crate::claude::corpus(&root), BYTES_PER_PASS).unwrap();
        let got = crate::claude::store::sessions_for_pr_number(&conn, 7).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].first_seen_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }
}
