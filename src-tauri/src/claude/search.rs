//! Full-text search over the transcript corpus (#1203, epic #1121).
//!
//! Headstate has always read the transcripts and indexed only their
//! METADATA -- session ids, working directories, timestamps, token
//! counts, pull-request links, crash rows. It indexed no content, so
//! "find the session where I was debugging the FSEvents thing" was a
//! question the app held the data for and could not answer.
//!
//! This module is the content index: SQLite FTS5, populated
//! incrementally by the live pass that already walks the corpus.
//!
//! # The corpus, and what indexing it costs
//!
//! Measured by `tests::real_corpus` against the real tree, release
//! build:
//!
//! ```text
//! sessions this app indexes   1,489 files   0.83 GB
//! subagent transcripts, excluded  1,037 files
//!
//! index build, cold, to full coverage    9,944 ms over 15 passes
//! steady-state pass (nothing changed)        3 ms
//! database growth                         0.06 GB
//! transcripts truncated at 8 MB                17
//! query latency                           0-18 ms
//! ```
//!
//! Two of those deserve comment because they are better than the issue
//! assumed, and the reason is not luck.
//!
//! **0.06 GB, not 0.83 GB.** #1203 expected "a second copy of 881 MB of
//! text in the database". It is 7% of that, because [`readable_text`]
//! indexes the human text -- user prompts and assistant messages -- and
//! not the raw JSONL. The uuids, key names, tool payloads and base64
//! blobs that make up most of a transcript's bytes are not things
//! anybody searches for, and indexing them would have cost twelve times
//! the space to make `"type"` match all 1,489 sessions.
//!
//! **3 ms in the steady state.** That is what the live pass actually
//! pays every 60 seconds once the index is warm, because the ledger's
//! `(size, mtime)` check skips an unchanged transcript without opening
//! it. The 9.9 s figure is the cold build, paid once, spread over
//! fifteen passes so no single one of them blocks anything.
//!
//! A naive `du` on `~/.claude/projects` reports 1.8 GB because it counts
//! the subagent transcripts [`super::transcript`] excludes on purpose --
//! they live at `<slug>/<session-id>/subagents/agent-*.jsonl`, carry no
//! session id of their own, and `claude --resume` cannot open them. This
//! module indexes exactly what [`super::transcript::scan`] returns, so
//! the exclusion is inherited rather than re-implemented, and
//! `subagent_transcripts_are_not_indexed` pins it.
//!
//! # THE RULE: an unfinished index may never say "no matches"
//!
//! This is the whole feature, and everything below is built around it.
//!
//! An empty result has two causes that look identical on screen:
//!
//! | what happened | what the user must read |
//! |---|---|
//! | the whole corpus is searchable and nothing matched | "No matches" |
//! | only part of it is searchable and that part had none | "No matches **in the 340 of 1,482 sessions indexed so far**" |
//!
//! Those are different sentences and the difference is the feature. 6.0
//! removed exactly this conflation from four other surfaces (#846,
//! #1042, #1044, #1152) -- "we have not looked yet" is not "there is
//! nothing there" -- and a search box is the place a user is least
//! likely to question an empty result. A search that has not finished
//! indexing and reports "no matches" is the most legible possible lie:
//! it is a confident, complete-sounding answer to a question nobody
//! asked the whole corpus.
//!
//! So the answer type is [`SearchAnswer`], whose [`SearchAnswer::verdict`]
//! is a THREE-way [`Verdict`] rather than a list that might be empty.
//! There is no way to get an empty result out of this module without
//! also getting the coverage that qualifies it: `Verdict::None` cannot
//! be constructed while `coverage.is_complete()` is false, because
//! [`answer`] is the only constructor and it branches on exactly that.
//!
//! Logging the shortfall and returning a bare empty list was considered
//! and is rejected: a partial result declares itself to the person
//! reading it, not to a log file nobody opens.
//!
//! # Coverage is counted, not estimated
//!
//! [`Coverage`] carries `indexed` and `total` as real counts:
//! `indexed` is `SELECT COUNT(*)` over the ledger table, `total` is the
//! number of session transcripts the last scan actually found. Neither
//! is a ratio, a percentage or a guess, because the sentence the UI has
//! to write contains the two numbers literally.
//!
//! `unreadable` is the third count and it is NOT folded into the other
//! two. A transcript that could not be read is a KNOWN GAP in coverage:
//! it is not indexed, so it does not count toward `indexed`, but it also
//! is not merely pending -- re-running the pass will not fix it. Saying
//! "1,480 of 1,482 indexed" while two files are permanently unreadable
//! would report a stuck index as a finished one.
//!
//! # Staleness: a fact about the INDEX, not about the session
//!
//! #1152 established a six-hour freshness window for scan caches. This
//! index needs its own answer because the user is actively producing the
//! content it covers: a session written since the last pass is genuinely
//! not yet searchable.
//!
//! The distinction the UI must be able to draw, and which
//! [`Coverage::pending`] exists for: that session is not missing from
//! your history and it is not empty -- **the index has not reached it
//! yet**. Framing it as a property of the session ("this session has no
//! content") would be a claim about the user's data that is false. It is
//! a claim about our bookkeeping, and it is true.
//!
//! # Why the ledger is keyed on (size, mtime) and not on content
//!
//! Re-indexing a transcript costs a whole-file read of a file that can
//! reach 76.7 MB. The ledger stores the size and modification time each
//! session was indexed AT, so an unchanged transcript is skipped without
//! being opened -- which is what makes the steady state cheap enough to
//! ride along on a 60-second pass.
//!
//! A transcript that GREW is re-indexed whole rather than appended to:
//! FTS5 has no partial-row update, and the alternative -- storing one
//! FTS row per record -- multiplies the row count by the message count
//! for a search whose unit of interest is the session.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// How much of one session's transcript is indexed.
///
/// The same 8 MB bound `usage::BUDGET_BYTES` uses, and for the same
/// measured reason: reading every transcript to the end turns a
/// sub-second pass into a disk-bound one, and one file on the
/// development machine is 76.7 MB by itself.
///
/// A session read to this bound is recorded with `truncated = 1` and
/// [`Coverage::truncated`] counts it, so a search over a corpus where
/// some transcripts are only partly indexed can say so. A bound that
/// could not be stated would be the same defect as an unfinished index
/// saying "no matches", one level down.
pub const INDEX_BUDGET_BYTES: u64 = 8 * 1024 * 1024;

/// How many sessions one pass will index before stopping.
///
/// The live pass runs every 60 seconds and must not become a
/// whole-corpus read. At this rate a cold 1,482-session corpus is fully
/// indexed in about 15 passes -- roughly fifteen minutes -- and every
/// one of those passes leaves the index STRICTLY more complete than it
/// found it, with the coverage numbers moving to match.
///
/// This is what makes the honesty constraint a live, ordinary state
/// rather than a rare edge case: for the first quarter of an hour on a
/// fresh install, every search is a partial search and says so.
pub const SESSIONS_PER_PASS: usize = 100;

/// What a query found, and over how much.
///
/// The two fields are ONE answer and are returned together. Handing a
/// caller a `Vec` of hits and making the coverage available separately
/// is the design that lets a UI render an empty list without it --
/// which is the defect this module exists to prevent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchAnswer {
    /// What the searched part of the corpus had to say.
    pub verdict: Verdict,
    /// How much of the corpus that was.
    pub coverage: Coverage,
}

/// The three things a search can honestly conclude.
///
/// An enum rather than `Vec<Hit>` because "nothing matched" and
/// "nothing matched YET" are different conclusions and a vector has
/// one empty value for both. The type makes the distinction
/// unforgettable at every call site: a caller that wants to render an
/// empty state has to say WHICH empty state it is rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// Sessions matched. Always non-empty -- an empty `Matches` is
    /// unrepresentable by construction in [`answer`].
    Matches { hits: Vec<Hit> },
    /// Nothing matched, and the whole corpus was searched. This is the
    /// only variant that may be rendered as a plain "no matches", and
    /// [`answer`] will not produce it while coverage is incomplete.
    None,
    /// Nothing matched in the part of the corpus that is searchable,
    /// and the rest has not been indexed yet.
    ///
    /// Carries its own numbers rather than making the caller reach into
    /// `coverage` for them: the sentence this variant exists to make the
    /// UI write is "no matches in the {indexed} of {total} sessions
    /// indexed so far", and a variant that did not carry the numbers
    /// would be renderable without them.
    NoneYet { indexed: u64, total: u64 },
}

/// One matching session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub session_id: String,
    /// FTS5 `snippet()` around the match, with the matched terms
    /// wrapped in `[` and `]`.
    pub snippet: String,
    /// Whether this session's transcript was indexed only to
    /// [`INDEX_BUDGET_BYTES`]. A hit is still a hit, but a MISS on a
    /// truncated session is not proof of absence -- which is why the
    /// flag travels on the row.
    pub truncated: bool,
}

/// How much of the corpus a search actually covered.
///
/// Every field is a count taken from the database or the last scan.
/// None is a ratio and none is derived from another, because the
/// sentences the UI writes name them individually.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    /// Sessions whose content is in the index right now.
    pub indexed: u64,
    /// Session transcripts the last scan found on disk. The
    /// denominator, and NOT a guess: it is what
    /// [`super::transcript::scan`] counted.
    ///
    /// Zero means the corpus size is unknown -- no pass has recorded one
    /// yet -- which [`Coverage::is_complete`] treats as incomplete. An
    /// unknown denominator cannot license a "no matches".
    pub total: u64,
    /// Transcripts the indexer could not read, with why.
    ///
    /// A known gap, not a pending one: counted and reported rather than
    /// skipped. These are NOT in `indexed` and re-running the pass will
    /// not move them, so an index with a non-empty `unreadable` is never
    /// complete no matter how high `indexed` climbs.
    pub unreadable: Vec<String>,
    /// Sessions indexed only as far as [`INDEX_BUDGET_BYTES`].
    ///
    /// Their content IS searchable; it is just not all of it. A miss
    /// against a truncated session is weaker evidence than a miss
    /// against a whole one, and the UI says so.
    pub truncated: u64,
    /// When the index last ran, RFC 3339, or `None` if it never has.
    ///
    /// `None` is not "just now" and not "long ago" -- it is "no pass has
    /// completed", which is a fourth state and the honest one for a
    /// fresh install.
    pub last_indexed_at: Option<String>,
}

impl Coverage {
    /// Whether every session on disk is searchable.
    ///
    /// This is the single predicate that licenses the words "no
    /// matches", so each clause is a way the index can be short:
    ///
    /// - `total == 0`: no pass has recorded a corpus size, so there is
    ///   no denominator and nothing to compare against. A search here
    ///   knows nothing about how much it searched.
    /// - `indexed < total`: sessions remain unindexed.
    /// - `!unreadable.is_empty()`: sessions that will never index. A
    ///   permanently unreadable transcript is a permanent hole, and an
    ///   index with a hole in it has not searched the corpus.
    ///
    /// `truncated` is deliberately NOT a clause. A truncated session is
    /// searchable -- partly -- and folding it in here would make the
    /// index permanently incomplete on any machine with one large
    /// transcript, which would make the partial banner meaningless by
    /// making it permanent. It is reported on its own instead, and on
    /// the hits it qualifies.
    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.indexed >= self.total && self.unreadable.is_empty()
    }

    /// Sessions found on disk but not yet in the index.
    ///
    /// Saturating, because `indexed` can briefly exceed `total` when
    /// transcripts are deleted between a pass and a scan. That is a
    /// stale denominator, not a negative backlog, and reporting it as
    /// a huge positive number via wrapping arithmetic would be the
    /// worst possible reading of it.
    pub fn pending(&self) -> u64 {
        self.total.saturating_sub(self.indexed)
    }
}

/// What one indexing pass did.
///
/// Returned so the live pass can report it, in the same shape
/// `Imported` uses: counts of what worked, and messages for what did
/// not. `unreadable` is the field #1145's failure reporting consumes --
/// a transcript that could not be indexed is a known gap in coverage,
/// not a silent absence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Indexed {
    /// Sessions whose content was written to the index this pass.
    pub indexed: usize,
    /// Sessions skipped because the ledger says they are unchanged.
    /// Not a failure -- this is the steady state, and it is what makes
    /// the pass cheap.
    pub unchanged: usize,
    /// Transcripts that could not be read, with why.
    ///
    /// Counted and reported, never skipped silently: an unreadable
    /// transcript is a hole in the corpus the search covers, and a hole
    /// nobody is told about becomes an unexplained "no matches" later.
    pub unreadable: Vec<String>,
    /// Transcripts the SCAN could not read, carried through.
    ///
    /// A different code path from `unreadable` above and a different
    /// moment -- these files never reached `scan.sessions`, so the
    /// indexer never saw them and could not have reported them. They are
    /// carried anyway because the question coverage answers is "is every
    /// session on disk searchable", and a session missing because the
    /// scan could not read it is exactly as absent from the index as one
    /// the indexer choked on. An index that counted only its OWN
    /// failures would call itself complete with a session missing.
    ///
    /// Kept in its own field rather than merged, because the two have
    /// different remedies: this one means the transcript is unreadable
    /// to the whole app, `unreadable` means it became so between two
    /// reads.
    pub scan_unreadable: Vec<String>,
    /// Rows the database refused, with why.
    pub write_failures: Vec<String>,
    /// Sessions read only to [`INDEX_BUDGET_BYTES`] this pass.
    pub truncated: usize,
    /// Sessions still waiting after this pass, because
    /// [`SESSIONS_PER_PASS`] bounded it.
    pub remaining: usize,
    /// Milliseconds the pass took, so the cost of riding along on the
    /// live pass stays checkable on someone else's machine.
    pub elapsed_ms: u64,
}

impl Indexed {
    /// Whether anything could not be read or written.
    pub fn is_partial(&self) -> bool {
        !self.unreadable.is_empty()
            || !self.scan_unreadable.is_empty()
            || !self.write_failures.is_empty()
    }

    /// Every transcript that is not searchable because it could not be
    /// read, whichever read failed.
    ///
    /// What [`coverage`] takes, because coverage asks one question --
    /// "is every session on disk searchable" -- and both kinds of
    /// failure answer it the same way.
    pub fn all_unreadable(&self) -> Vec<String> {
        let mut out = self.unreadable.clone();
        out.extend(self.scan_unreadable.iter().cloned());
        out
    }
}

/// The searchable text of one transcript, and whether it is all of it.
///
/// Extracts the human text -- user prompts and assistant messages --
/// rather than the raw JSONL. Indexing the raw lines would put every
/// key name, uuid, base64 blob and tool payload in the index: it
/// multiplies the index size, and it makes `"type"` match all 1,482
/// sessions, which is a search that returns everything and therefore
/// answers nothing.
fn readable_text(path: &Path, budget: u64) -> Result<(String, bool), String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
    let mut reader = BufReader::new(file);

    let mut out = String::new();
    let mut read: u64 = 0;
    let mut truncated = false;
    let mut line = String::new();
    loop {
        line.clear();
        let n = match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) => n,
            // One undecodable line ends the read rather than failing the
            // session: whatever was gathered is still true text, and it
            // is recorded as truncated so the shortfall is visible.
            Err(_) => {
                truncated = true;
                break;
            }
        };
        read += n as u64;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            collect_text(&v, &mut out);
        }
        if read >= budget {
            truncated = true;
            break;
        }
    }
    Ok((out, truncated))
}

/// Pull the human-readable strings out of one transcript record.
///
/// Claude Code writes `message.content` as either a bare string or an
/// array of typed blocks, and the text lives in `text` fields either
/// way. Tool inputs and results are deliberately left out: they are
/// mostly file contents and command output, which would dominate the
/// index and make a search for a word the user typed return every
/// session that ever read a file containing it.
fn collect_text(v: &serde_json::Value, out: &mut String) {
    let Some(msg) = v.get("message") else { return };
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => {
            out.push_str(s);
            out.push('\n');
        }
        Some(serde_json::Value::Array(blocks)) => {
            for b in blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(s) = b.get("text").and_then(|t| t.as_str()) {
                        out.push_str(s);
                        out.push('\n');
                    }
                }
            }
        }
        _ => {}
    }
}

/// Index up to [`SESSIONS_PER_PASS`] sessions that have changed.
///
/// Incremental by construction: the ledger's `(size, mtime)` per session
/// is compared against what the scan found, and an unchanged transcript
/// is never opened. A bounded pass, so riding along on a 60-second loop
/// does not turn it into a whole-corpus read -- `remaining` says how
/// many are still waiting, which is what keeps the partial state a
/// NUMBER rather than a feeling.
///
/// Every failure is carried out, never swallowed. See [`Indexed`].
pub fn index_pass(conn: &mut Connection, scan: &super::Scan) -> Result<Indexed, rusqlite::Error> {
    let started = std::time::Instant::now();
    let mut out = Indexed {
        // Carried through from the scan rather than dropped at this
        // boundary, for the reason the field's own doc gives: a session
        // the scan could not read is as absent from the index as one
        // this pass could not read.
        scan_unreadable: scan.unreadable_files.clone(),
        ..Default::default()
    };
    let now = chrono::Utc::now().to_rfc3339();

    // The corpus size this pass observed, recorded FIRST and
    // unconditionally. It is the denominator every coverage sentence
    // uses, and recording it only on a successful index would leave a
    // machine whose every transcript is unreadable reporting "0 of 0"
    // -- a complete index of nothing, which is exactly the reassuring
    // lie this module exists to prevent.
    conn.execute(
        "INSERT INTO claude_index_state (id, corpus_sessions, last_pass_at)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET corpus_sessions = ?1, last_pass_at = ?2",
        // The denominator counts the transcripts that EXIST, not the
        // ones that parsed. A file the scan could not read is still a
        // session on disk the user may be searching for, and leaving it
        // out of the denominator would let "1 of 1 indexed" be reported
        // on a machine holding two transcripts.
        rusqlite::params![
            (scan.sessions.len() + scan.unreadable_files.len()) as i64,
            &now
        ],
    )?;

    // What the ledger already holds, read once rather than queried per
    // session: 1,482 point lookups against a 60-second budget is work
    // that buys nothing over one scan of a table with one row per
    // session.
    let mut known: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
    {
        let mut q =
            conn.prepare("SELECT session_id, size_bytes, mtime_ms FROM claude_index_ledger")?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, size, mtime) = row?;
            known.insert(id, (size, mtime));
        }
    }

    // Which sessions need work, decided before any file is opened.
    let mut todo: Vec<(&super::Transcript, i64, i64)> = Vec::new();
    for t in &scan.sessions {
        let path = Path::new(&t.path);
        // A transcript whose metadata cannot be read is a known gap, not
        // a skip: it is reported here rather than silently left out of
        // the index, because a session missing from the index for an
        // unstated reason becomes an unexplained "no matches" later.
        let (size, mtime) = match std::fs::metadata(path).and_then(|m| {
            let mtime = m
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Ok((m.len() as i64, mtime))
        }) {
            Ok(v) => v,
            Err(e) => {
                out.unreadable
                    .push(format!("{}: could not read its size: {e}", t.path));
                continue;
            }
        };
        match known.get(&t.session_id) {
            Some((s, m)) if *s == size && *m == mtime => out.unchanged += 1,
            _ => todo.push((t, size, mtime)),
        }
    }

    out.remaining = todo.len().saturating_sub(SESSIONS_PER_PASS);

    let tx = conn.transaction()?;
    for (t, size, mtime) in todo.into_iter().take(SESSIONS_PER_PASS) {
        let (text, truncated) = match readable_text(Path::new(&t.path), INDEX_BUDGET_BYTES) {
            Ok(v) => v,
            Err(e) => {
                // THE reporting path #1145 asked for. Counted, carried
                // out, and never a silent skip.
                out.unreadable.push(e);
                continue;
            }
        };
        if truncated {
            out.truncated += 1;
        }

        // FTS5 has no partial-row update, so a re-index deletes and
        // re-inserts. Both statements in the same transaction, so a
        // reader never sees the gap between them -- a session that
        // vanished mid-pass would otherwise read as "indexed, no
        // content", which is a false negative dressed as a real answer.
        if let Err(e) = tx.execute(
            "DELETE FROM claude_transcript_fts WHERE session_id = ?1",
            rusqlite::params![&t.session_id],
        ) {
            out.write_failures.push(format!(
                "{}: could not clear its old index: {e}",
                t.session_id
            ));
            continue;
        }
        if let Err(e) = tx.execute(
            "INSERT INTO claude_transcript_fts (session_id, body) VALUES (?1, ?2)",
            rusqlite::params![&t.session_id, &text],
        ) {
            out.write_failures
                .push(format!("{}: could not index it: {e}", t.session_id));
            continue;
        }
        if let Err(e) = tx.execute(
            "INSERT INTO claude_index_ledger
                (session_id, size_bytes, mtime_ms, truncated, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id) DO UPDATE SET
                size_bytes = ?2, mtime_ms = ?3, truncated = ?4, indexed_at = ?5",
            rusqlite::params![&t.session_id, size, mtime, truncated as i64, &now],
        ) {
            out.write_failures.push(format!(
                "{}: could not record that it is indexed: {e}",
                t.session_id
            ));
            continue;
        }
        out.indexed += 1;
    }

    // Sessions whose transcript is GONE leave the index, so a search
    // cannot hit a session the corpus no longer has. Done after the
    // inserts and inside the same transaction, against the ids this
    // scan actually saw.
    let present: HashSet<&str> = scan
        .sessions
        .iter()
        .map(|t| t.session_id.as_str())
        .collect();
    let stale: Vec<String> = known
        .keys()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect();
    for id in stale {
        let _ = tx.execute(
            "DELETE FROM claude_transcript_fts WHERE session_id = ?1",
            rusqlite::params![&id],
        );
        let _ = tx.execute(
            "DELETE FROM claude_index_ledger WHERE session_id = ?1",
            rusqlite::params![&id],
        );
    }

    tx.commit()?;
    out.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(out)
}

/// What the index currently covers.
///
/// Read separately from a query so a UI can state coverage before a
/// search is run -- "1,482 sessions searchable" is worth saying on an
/// empty search box, and a number that only appears alongside results
/// cannot be shown then.
pub fn coverage(conn: &Connection, unreadable: Vec<String>) -> Result<Coverage, rusqlite::Error> {
    let indexed: i64 =
        conn.query_row("SELECT COUNT(*) FROM claude_index_ledger", [], |r| r.get(0))?;
    let truncated: i64 = conn.query_row(
        "SELECT COALESCE(SUM(truncated), 0) FROM claude_index_ledger",
        [],
        |r| r.get(0),
    )?;
    let state: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT corpus_sessions, last_pass_at FROM claude_index_state WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (total, last_indexed_at) = state.unwrap_or((0, None));

    Ok(Coverage {
        indexed: indexed as u64,
        total: total as u64,
        unreadable,
        truncated: truncated as u64,
        last_indexed_at,
    })
}

/// Turn a user's words into an FTS5 MATCH expression.
///
/// Every term is quoted, so the FTS5 query operators a user did not mean
/// to type -- `*`, `:`, `^`, `NEAR`, `-` -- are searched for rather than
/// executed. Without this, a search for `foo-bar` is parsed as "foo NOT
/// bar" and a search for `C++` is a syntax error, and both come back as
/// a failure or a wrong answer to a question the user asked plainly.
fn fts_query(raw: &str) -> Option<String> {
    let terms: Vec<String> = raw
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(terms.join(" AND "))
}

/// Search the index, and say how much of the corpus that was.
///
/// **This is the function the ticket is about.** The branch at the
/// bottom is the feature: an empty hit list becomes [`Verdict::None`]
/// only when [`Coverage::is_complete`] licenses it, and
/// [`Verdict::NoneYet`] with the two numbers otherwise. There is no path
/// through this function that returns a bare empty result.
///
/// `limit` bounds the rows returned, not the rows searched -- so a
/// result that hit the cap is still a statement about the whole indexed
/// corpus, and `Verdict::Matches` being non-empty is what the caller
/// renders regardless.
pub fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
    unreadable: Vec<String>,
) -> Result<SearchAnswer, rusqlite::Error> {
    let cov = coverage(conn, unreadable)?;

    let Some(expr) = fts_query(query) else {
        // An empty query is not a search that found nothing. It is not a
        // search at all, so it gets the coverage and no verdict about
        // matches -- rendering it as "no matches" would answer a
        // question the user never asked.
        return Ok(SearchAnswer {
            verdict: none_verdict(&cov),
            coverage: cov,
        });
    };

    let mut q = conn.prepare(
        "SELECT f.session_id,
                snippet(claude_transcript_fts, 1, '[', ']', ' … ', 24),
                COALESCE(l.truncated, 0)
           FROM claude_transcript_fts f
           LEFT JOIN claude_index_ledger l ON l.session_id = f.session_id
          WHERE claude_transcript_fts MATCH ?1
          ORDER BY rank
          LIMIT ?2",
    )?;
    let rows = q.query_map(rusqlite::params![&expr, limit as i64], |r| {
        Ok(Hit {
            session_id: r.get(0)?,
            snippet: r.get(1)?,
            truncated: r.get::<_, i64>(2)? != 0,
        })
    })?;
    let hits: Vec<Hit> = rows.collect::<Result<Vec<_>, _>>()?;

    // ---- The branch this whole module exists for ----
    //
    // `hits.is_empty()` alone is NOT enough to say "no matches". It is
    // only enough to say "no matches in what we have searched", and
    // which of those two sentences is true depends entirely on
    // coverage. Collapsing this into `if hits.is_empty() { None }` is
    // the bug (#846, #1042, #1044, #1152), and it is a one-line bug,
    // which is why the branch is written out with the reason attached.
    let verdict = if hits.is_empty() {
        none_verdict(&cov)
    } else {
        Verdict::Matches { hits }
    };

    Ok(SearchAnswer {
        verdict,
        coverage: cov,
    })
}

/// Which flavour of "nothing" this is.
///
/// One place, so the two call sites above cannot come to disagree about
/// when a plain "no matches" is licensed. The predicate is
/// [`Coverage::is_complete`] and nothing else.
fn none_verdict(cov: &Coverage) -> Verdict {
    if cov.is_complete() {
        Verdict::None
    } else {
        Verdict::NoneYet {
            indexed: cov.indexed,
            total: cov.total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database with the real schema, so these tests exercise the
    /// migration rather than a hand-written table that could drift from
    /// it.
    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    /// One indexed session, written through the real index pass.
    fn write_session(dir: &Path, slug: &str, id: &str, body: &str) -> std::path::PathBuf {
        let d = dir.join(slug);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(format!("{id}.jsonl"));
        let mut lines = String::new();
        lines.push_str("{\"type\":\"queue-operation\"}\n");
        lines.push_str(&format!(
            "{{\"type\":\"user\",\"cwd\":\"/tmp/acme\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":{}}}}}\n",
            serde_json::to_string(body).unwrap()
        ));
        std::fs::write(&p, lines).unwrap();
        p
    }

    /// THE TEST. A query against a partially built index reports the
    /// partial coverage WITH ITS NUMBER and does not say "no matches".
    ///
    /// The distinct wording is asserted on, not merely the absence of
    /// the other one: `NoneYet` carries `indexed` and `total` so the UI
    /// can write "no matches in the 1 of 4 sessions indexed so far",
    /// and a variant that carried no numbers would pass a test that
    /// only checked which variant it was.
    #[test]
    fn a_partial_index_reports_its_coverage_and_never_says_no_matches() {
        let conn = db();
        // One session indexed, four on disk: the state every fresh
        // install is in for its first quarter of an hour.
        conn.execute(
            "INSERT INTO claude_transcript_fts (session_id, body) VALUES ('a', 'fsevents stream died')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_index_ledger (session_id, size_bytes, mtime_ms, truncated, indexed_at)
             VALUES ('a', 10, 10, 0, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_index_state (id, corpus_sessions, last_pass_at)
             VALUES (1, 4, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let answer = search(&conn, "kubernetes", 20, vec![]).unwrap();

        assert_eq!(
            answer.verdict,
            Verdict::NoneYet {
                indexed: 1,
                total: 4
            },
            "a search over an unfinished index must report WHICH PART it \
             searched -- 'no matches' here is the #846 conflation in the \
             one place a user is least likely to question it"
        );
        assert_ne!(
            answer.verdict,
            Verdict::None,
            "the settled 'no matches' must be unreachable while the index is short"
        );
        assert!(!answer.coverage.is_complete());
        assert_eq!(answer.coverage.pending(), 3);
    }

    /// The other half: a genuinely empty result over a COMPLETE index
    /// says the settled thing. The two cases must produce different
    /// values, or the first test proves nothing.
    #[test]
    fn a_complete_index_with_no_hits_says_no_matches() {
        let conn = db();
        for id in ["a", "b"] {
            conn.execute(
                "INSERT INTO claude_transcript_fts (session_id, body) VALUES (?1, 'fsevents')",
                rusqlite::params![id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO claude_index_ledger (session_id, size_bytes, mtime_ms, truncated, indexed_at)
                 VALUES (?1, 10, 10, 0, '2026-01-01T00:00:00Z')",
                rusqlite::params![id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO claude_index_state (id, corpus_sessions, last_pass_at)
             VALUES (1, 2, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let answer = search(&conn, "kubernetes", 20, vec![]).unwrap();
        assert_eq!(
            answer.verdict,
            Verdict::None,
            "the whole corpus was searched, so the settled answer is licensed"
        );
        assert!(answer.coverage.is_complete());
    }

    /// The two sentences are different, asserted side by side.
    ///
    /// The mandatory pair as ONE test, because the property is not
    /// "each case has a value" but "the values differ": a build where
    /// `none_verdict` always returned `None` passes each of the two
    /// tests above only if they are read separately.
    #[test]
    fn the_partial_and_the_complete_empty_are_not_the_same_answer() {
        let conn = db();
        conn.execute(
            "INSERT INTO claude_index_state (id, corpus_sessions, last_pass_at)
             VALUES (1, 4, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let partial = search(&conn, "nothing", 20, vec![]).unwrap().verdict;

        conn.execute("UPDATE claude_index_state SET corpus_sessions = 0", [])
            .unwrap();
        conn.execute(
            "INSERT INTO claude_index_ledger (session_id, size_bytes, mtime_ms, truncated, indexed_at)
             VALUES ('a', 1, 1, 0, 'now')",
            [],
        )
        .unwrap();
        conn.execute("UPDATE claude_index_state SET corpus_sessions = 1", [])
            .unwrap();
        let complete = search(&conn, "nothing", 20, vec![]).unwrap().verdict;

        assert_ne!(
            partial, complete,
            "an unfinished index and a finished one must not give the \
             user the same sentence about an empty result"
        );
        assert!(matches!(partial, Verdict::NoneYet { .. }));
        assert_eq!(complete, Verdict::None);
    }

    /// An unknown corpus size cannot license "no matches" either.
    ///
    /// The fresh-install case: nothing has scanned yet, so there is no
    /// denominator. Reporting "no matches" from a search that does not
    /// know how much there was to search is the same lie with a
    /// different cause.
    #[test]
    fn an_unknown_corpus_size_is_not_a_complete_index() {
        let conn = db();
        let answer = search(&conn, "anything", 20, vec![]).unwrap();
        assert_eq!(
            answer.verdict,
            Verdict::NoneYet {
                indexed: 0,
                total: 0
            }
        );
        assert!(!answer.coverage.is_complete());
    }

    /// A transcript the INDEXER could not read is counted and reported,
    /// and it keeps the index from claiming completeness.
    ///
    /// This is the indexer's own failure path: a file the scan accepted
    /// -- it is a real `.jsonl` and `extract` read its head -- which
    /// cannot be opened when the indexer comes back for its body. A
    /// permission change between the two reads produces exactly this,
    /// and the honest answer is a reported gap rather than a session
    /// quietly missing from the index.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_transcript_is_counted_and_reported() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(root, "slug", "good", "fsevents stream died");
        let bad = write_session(root, "slug", "bad", "never readable");

        // Scanned while readable, so it reaches `scan.sessions` and the
        // indexer is the thing that fails on it.
        let scan = crate::claude::scan(root);
        assert_eq!(scan.sessions.len(), 2, "the premise: both were scanned");
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut conn = db();
        let done = index_pass(&mut conn, &scan).unwrap();

        assert_eq!(done.indexed, 1, "the readable one is indexed");
        assert_eq!(
            done.unreadable.len(),
            1,
            "the unreadable one is REPORTED, not skipped -- a transcript \
             nobody is told about becomes an unexplained 'no matches'"
        );
        assert!(done.unreadable[0].contains("bad.jsonl"));
        assert!(done.is_partial());

        // And it travels into coverage, where it keeps the index from
        // reading as complete however high `indexed` climbs.
        let cov = coverage(&conn, done.unreadable.clone()).unwrap();
        assert_eq!(cov.unreadable.len(), 1);
        assert!(
            !cov.is_complete(),
            "a permanently unreadable transcript is a permanent hole, and \
             an index with a hole has not searched the corpus"
        );

        let answer = search(&conn, "kubernetes", 20, done.unreadable).unwrap();
        assert!(
            matches!(answer.verdict, Verdict::NoneYet { .. }),
            "so a miss stays qualified rather than settled"
        );

        // Restored so the temp dir can be cleaned up.
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// A transcript the SCAN could not read is a gap too.
    ///
    /// The other half of the same rule, and a different code path: this
    /// file never reaches `scan.sessions` at all, so the indexer never
    /// sees it and cannot report it. It arrives through
    /// [`Indexed::scan_unreadable`] instead -- because a session missing
    /// from the index is a hole in the search's coverage regardless of
    /// WHICH read failed, and an index that only counted its own
    /// failures would call itself complete while a session was missing.
    #[test]
    #[cfg(unix)]
    fn a_transcript_the_scan_could_not_read_is_also_a_gap() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(root, "slug", "good", "fsevents stream died");
        // Unreadable BEFORE the scan, so `extract` fails on it and it
        // never becomes a session at all -- the path the indexer cannot
        // see and therefore cannot report for itself.
        let bad = write_session(root, "slug", "bad", "never readable");
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();

        let scan = crate::claude::scan(root);
        assert_eq!(scan.sessions.len(), 1);
        assert_eq!(scan.unreadable_files.len(), 1, "the premise");

        let mut conn = db();
        let done = index_pass(&mut conn, &scan).unwrap();
        assert_eq!(done.indexed, 1);
        assert_eq!(
            done.scan_unreadable.len(),
            1,
            "carried through rather than dropped at the module boundary"
        );
        assert!(done.is_partial());

        let cov = coverage(&conn, done.all_unreadable()).unwrap();
        assert!(
            !cov.is_complete(),
            "one session on disk is not searchable, so the corpus is not covered"
        );
        assert!(matches!(
            search(&conn, "kubernetes", 20, done.all_unreadable())
                .unwrap()
                .verdict,
            Verdict::NoneYet { .. }
        ));

        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// Subagent transcripts are not indexed.
    ///
    /// They live at `<slug>/<session-id>/subagents/agent-*.jsonl`, carry
    /// no session id and `claude --resume` cannot open them. Asserted
    /// against the real nested layout rather than by trusting the depth
    /// arithmetic, which is how `transcript.rs` pins the same rule.
    #[test]
    fn subagent_transcripts_are_not_indexed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(root, "slug", "real-session", "parent says fsevents");

        let sub = root.join("slug").join("real-session").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("agent-1.jsonl"),
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"zzzsubagentonlyzzz\"}}\n",
        )
        .unwrap();

        let scan = crate::claude::scan(root);
        let mut conn = db();
        let done = index_pass(&mut conn, &scan).unwrap();
        assert_eq!(done.indexed, 1, "only the real session");

        // The subagent's distinctive word is not findable, and the
        // parent's is -- so this is a test of the exclusion and not of a
        // broken index.
        let sub_hit = search(&conn, "zzzsubagentonlyzzz", 20, vec![]).unwrap();
        assert!(
            !matches!(sub_hit.verdict, Verdict::Matches { .. }),
            "a subagent transcript must not be searchable: it has no \
             session id and offers a resume handle for something that was \
             never a session"
        );

        let parent = search(&conn, "fsevents", 20, vec![]).unwrap();
        let Verdict::Matches { hits } = parent.verdict else {
            panic!("the parent session must be findable, or this test proves nothing");
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "real-session");
    }

    /// The index is incremental: an unchanged transcript is not re-read.
    #[test]
    fn an_unchanged_transcript_is_not_reindexed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(root, "slug", "s1", "hello fsevents");

        let mut conn = db();
        let first = index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert_eq!(first.indexed, 1);
        assert_eq!(first.unchanged, 0);

        let second = index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert_eq!(second.indexed, 0, "nothing changed, so nothing was opened");
        assert_eq!(second.unchanged, 1);
    }

    /// A session whose transcript is gone leaves the index, so a search
    /// cannot offer a hit on a session the corpus no longer holds.
    #[test]
    fn a_deleted_transcript_leaves_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let p = write_session(root, "slug", "s1", "hello fsevents");

        let mut conn = db();
        index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert!(matches!(
            search(&conn, "fsevents", 20, vec![]).unwrap().verdict,
            Verdict::Matches { .. }
        ));

        std::fs::remove_file(&p).unwrap();
        index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert!(
            !matches!(
                search(&conn, "fsevents", 20, vec![]).unwrap().verdict,
                Verdict::Matches { .. }
            ),
            "a hit on a transcript that no longer exists offers a resume \
             handle for nothing"
        );
    }

    /// A user's punctuation is searched for, not executed as FTS5
    /// syntax.
    #[test]
    fn query_punctuation_is_not_fts_syntax() {
        let conn = db();
        conn.execute(
            "INSERT INTO claude_transcript_fts (session_id, body) VALUES ('a', 'the foo-bar thing')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_index_ledger (session_id, size_bytes, mtime_ms, truncated, indexed_at)
             VALUES ('a', 1, 1, 0, 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO claude_index_state (id, corpus_sessions, last_pass_at) VALUES (1, 1, 'now')",
            [],
        )
        .unwrap();

        // Unquoted, `foo-bar` parses as "foo NOT bar" and `C++` is a
        // syntax error. Both must come back as ordinary searches.
        assert!(matches!(
            search(&conn, "foo-bar", 20, vec![]).unwrap().verdict,
            Verdict::Matches { .. }
        ));
        assert!(search(&conn, "C++", 20, vec![]).is_ok());
        assert!(search(&conn, "NEAR", 20, vec![]).is_ok());
    }

    /// The bound is real and is REPORTED, so a partly-indexed
    /// transcript's miss is qualified rather than settled.
    #[test]
    fn a_truncated_transcript_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let d = root.join("slug");
        std::fs::create_dir_all(&d).unwrap();
        // One record per line, past the budget. The distinctive word is
        // at the END, so a reader that respected no bound would find it.
        let mut body = String::new();
        let filler = "x".repeat(4096);
        while body.len() < (INDEX_BUDGET_BYTES as usize) + 8192 {
            body.push_str(&format!(
                "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{filler}\"}}}}\n"
            ));
        }
        body.push_str(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"zzzpastthebudgetzzz\"}}\n",
        );
        std::fs::write(d.join("big.jsonl"), body).unwrap();

        let mut conn = db();
        let done = index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert_eq!(done.truncated, 1, "the bound was hit and is counted");

        let cov = coverage(&conn, vec![]).unwrap();
        assert_eq!(cov.truncated, 1, "and it reaches the coverage report");

        // The word past the bound is genuinely not searchable, which is
        // exactly why `truncated` has to be reported.
        assert!(!matches!(
            search(&conn, "zzzpastthebudgetzzz", 20, vec![])
                .unwrap()
                .verdict,
            Verdict::Matches { .. }
        ));
    }

    /// A bounded pass says how many sessions are still waiting.
    ///
    /// `remaining` is what turns "the index is not done" into a number,
    /// and a pass that indexed everything it was given must report zero
    /// rather than leaving the UI to infer it.
    #[test]
    fn a_bounded_pass_reports_what_is_left() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..3 {
            write_session(root, "slug", &format!("s{i}"), "hello");
        }
        let mut conn = db();
        let done = index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert_eq!(done.indexed, 3);
        assert_eq!(
            done.remaining, 0,
            "everything fit in one pass, and the UI must not be left to guess that"
        );

        let cov = coverage(&conn, vec![]).unwrap();
        assert_eq!(cov.indexed, 3);
        assert_eq!(cov.total, 3);
        assert!(cov.is_complete());
    }

    /// The corpus size is recorded even when nothing could be indexed.
    ///
    /// Otherwise a machine whose every transcript is unreadable reports
    /// "0 of 0", which `is_complete` would read as a finished index of
    /// an empty corpus -- a complete search over nothing, which is the
    /// most reassuring possible way to report a total failure.
    #[test]
    #[cfg(unix)]
    fn the_denominator_is_recorded_even_when_nothing_indexes() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let bad = write_session(root, "slug", "bad", "never readable");
        let scan = crate::claude::scan(root);
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut conn = db();
        let done = index_pass(&mut conn, &scan).unwrap();
        assert_eq!(done.indexed, 0);

        let cov = coverage(&conn, done.all_unreadable()).unwrap();
        assert_eq!(cov.total, 1, "the corpus size was observed and recorded");
        assert_eq!(cov.indexed, 0);
        assert!(
            !cov.is_complete(),
            "0 of 0 would read as a complete index of an empty corpus -- \
             the most reassuring possible way to report a total failure"
        );

        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// The real corpus: index build time, database growth, query
    /// latency.
    ///
    /// Committed rather than run once and discarded, for the reason
    /// `transcript::tests::real_corpus` gives: the numbers in this
    /// module's docs and in #1203's pull request are load-bearing -- the
    /// whole incremental-versus-on-demand decision rests on them -- and
    /// a measurement nobody can reproduce is just an assertion.
    ///
    /// Prints rather than asserts the figures, because the corpus grows
    /// as the machine is used. What it DOES assert is the property that
    /// must hold on any machine: an index built over the whole corpus
    /// reports itself complete, and one built over part of it does not.
    #[test]
    #[ignore = "needs the developer's own ~/.claude/projects"]
    fn real_corpus() {
        let Some(root) = crate::claude::transcript::projects_dir() else {
            eprintln!("no home directory");
            return;
        };
        let scan = crate::claude::scan(&root);
        println!("sessions found            {}", scan.sessions.len());
        println!("subagent .jsonl skipped   {}", scan.subagent_files_skipped);
        println!(
            "session bytes             {:.2} GB",
            scan.session_bytes as f64 / 1_073_741_824.0
        );
        println!("scan elapsed              {} ms", scan.elapsed_ms);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bench.db");
        let mut conn = Connection::open(&path).unwrap();
        crate::store::migrate(&conn).unwrap();

        // Every pass, to full coverage, so the figure is the whole build
        // and not one bounded slice of it.
        let mut passes = 0;
        let mut total_ms = 0u64;
        let mut truncated = 0;
        loop {
            let done = index_pass(&mut conn, &scan).unwrap();
            total_ms += done.elapsed_ms;
            truncated += done.truncated;
            passes += 1;
            if done.indexed == 0 || done.remaining == 0 {
                break;
            }
        }
        println!("index passes              {passes}");
        println!("index build total         {total_ms} ms");
        println!("truncated at 8 MB         {truncated}");

        // A steady-state pass: everything unchanged, which is what the
        // live pass actually pays every 60 seconds.
        let steady = index_pass(&mut conn, &scan).unwrap();
        println!(
            "steady-state pass         {} ms ({} unchanged)",
            steady.elapsed_ms, steady.unchanged
        );

        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").ok();
        let db_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!(
            "database size             {:.2} GB",
            db_bytes as f64 / 1_073_741_824.0
        );

        let cov = coverage(&conn, scan.unreadable_files.clone()).unwrap();
        println!("coverage                  {} of {}", cov.indexed, cov.total);

        // The third is a CONTROL: a term that must match nothing, so the
        // no-hit path is timed too.
        //
        // It is a nonsense token rather than a phrase of ordinary words.
        // A readable phrase like "no such word anywhere" is written into
        // the corpus BY THIS TEST RUN -- the transcript of the session
        // that runs it contains the source -- so it came back with hits
        // and the control measured the opposite of what it claimed. A
        // self-referential corpus is the trap here, and the literal
        // below is deliberately not a word anyone would type.
        for q in ["fsevents", "migration", "qqxzvw7unlikelytoken"] {
            let t = std::time::Instant::now();
            let a = search(&conn, q, 50, scan.unreadable_files.clone()).unwrap();
            let hits = match &a.verdict {
                Verdict::Matches { hits } => hits.len(),
                _ => 0,
            };
            println!(
                "query {:<28} {:>5} ms  {} hits",
                format!("\"{q}\""),
                t.elapsed().as_millis(),
                hits
            );
        }

        // The property, on any machine: a whole-corpus index says so.
        assert!(
            cov.is_complete() || !cov.unreadable.is_empty(),
            "a fully built index over a fully readable corpus must report \
             itself complete -- {} of {}, {} unreadable",
            cov.indexed,
            cov.total,
            cov.unreadable.len()
        );
    }

    /// The wire shape matches what the frontend's discriminated union
    /// expects (#1203).
    ///
    /// A `serde` tag rename that disagreed with `ClaudeSearchVerdict` in
    /// `types/pr.ts` would typecheck on both sides and fail only at
    /// runtime -- and the failure mode is the one this feature exists to
    /// prevent: an unmatched `kind` falls through the component's
    /// branches to the settled "No matches", which is exactly the
    /// sentence a partial index must never produce.
    #[test]
    fn the_verdict_serialises_as_the_frontend_union() {
        let partial = serde_json::to_value(Verdict::NoneYet {
            indexed: 340,
            total: 1482,
        })
        .unwrap();
        assert_eq!(partial["kind"], "none_yet");
        assert_eq!(partial["indexed"], 340);
        assert_eq!(partial["total"], 1482);

        let settled = serde_json::to_value(Verdict::None).unwrap();
        assert_eq!(settled["kind"], "none");

        let hits = serde_json::to_value(Verdict::Matches {
            hits: vec![Hit {
                session_id: "s1".into(),
                snippet: "x".into(),
                truncated: false,
            }],
        })
        .unwrap();
        assert_eq!(hits["kind"], "matches");
        assert_eq!(hits["hits"][0]["session_id"], "s1");

        // Coverage's field names, which the component reads directly.
        let cov = serde_json::to_value(Coverage::default()).unwrap();
        for key in [
            "indexed",
            "total",
            "unreadable",
            "truncated",
            "last_indexed_at",
        ] {
            assert!(cov.get(key).is_some(), "coverage is missing `{key}`");
        }
    }

    /// `pending` does not wrap when the denominator is stale.
    #[test]
    fn a_stale_denominator_is_not_a_huge_backlog() {
        let cov = Coverage {
            indexed: 10,
            total: 4,
            ..Default::default()
        };
        assert_eq!(cov.pending(), 0);
    }
}
