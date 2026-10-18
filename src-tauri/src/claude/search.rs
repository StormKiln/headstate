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
//! index build, cold, to full coverage    9,953 ms over 15 passes
//! steady-state index step                    3 ms
//! ADDED COST to the live pass, warm         56 ms (51 listing + 5 index)
//! database growth                         0.06 GB
//! transcripts truncated at 8 MB                17
//! query latency                           1-19 ms
//! ```
//!
//! # What the live pass pays, and what it used to (#1246)
//!
//! As #1203 shipped it, the added cost was **885 ms per tick**, and
//! only 5 ms of that was indexing. The other 880 ms was
//! `transcript::scan`, which `claude_live_pass` had no reason to call
//! before #1203 -- that is new work on that loop, and it was 99% of
//! the bill.
//!
//! #1246 removed it. The scan was never the work this module needed:
//! [`index_pass`] reads a session's `path` and `session_id` and the
//! scan's `unreadable_files`, and takes its own `fs::metadata` per file
//! because the ledger is keyed on `(size, mtime)`. `scan` opens every
//! transcript for a head read and a tail seek to recover a title, a cwd
//! and a last-activity time, then hands the file list to
//! `subagent::build`, which reads all 0.84 GB end to end. Every one of
//! those fields is discarded at the `index_pass` call.
//!
//! So the pass now calls [`super::transcript::corpus`], which walks the
//! tree and checks each transcript can be opened, but reads none of
//! them. Same 1,510 sessions, same denominator, same unreadable set --
//! **51 ms instead of 880**.
//!
//! ```text
//! added cost to the live pass, warm    #1203      #1246
//!   corpus listing / scan              880 ms     51 ms
//!   index step                           5 ms      5 ms
//!                                      ------     -----
//!                                      885 ms     56 ms
//! ```
//!
//! Measured four consecutive times with both paths warm, release
//! build: `corpus` 49-60 ms against `scan` 878-935 ms, the same 1,510
//! sessions each time. A **16x** reduction on the live pass, and the
//! remaining 56 ms is 0.09% of the 60-second tick.
//!
//! The same substitution was made in `claude_search_transcripts` and
//! `claude_index_coverage`, which each paid the same 880 ms for the
//! denominator alone -- on the path of a user waiting for a search
//! result rather than on a background timer.
//!
//! #1246's own suggestion was to share one scan with the session
//! import. It does not fit: `claude_import_transcripts` is a separate
//! user-triggered Tauri command, not part of the live pass, so there is
//! no shared tick to fold into. The issue's second suggestion, cheap
//! change detection before the walk, was aimed at the wrong cost -- the
//! walk and its stats are 15 ms of the 51, and the 829 ms that went
//! away was file CONTENT the indexer never looked at.
//!
//! # The index still advances every tick
//!
//! This does not slow the index down, which is the thing that would
//! have put the honesty property at risk. [`SESSIONS_PER_PASS`] is
//! unchanged, a pass still runs every 60 seconds, and a cold corpus
//! still reaches full coverage in about fifteen passes -- faster in
//! wall-clock terms, because each pass now spends its time indexing
//! rather than reading files it will throw away.
//!
//! # Two figures that are better than the issue assumed
//!
//! Not luck; the reason is worth stating so it is not lost.
//!
//! **0.06 GB, not 0.83 GB.** #1203 expected "a second copy of 881 MB of
//! text in the database". It is 7% of that, because [`readable_text_controlled`]
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

use crate::remote::privacy::Matching;

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
    /// [`search`] will not produce it while coverage is incomplete.
    None,
    /// No query was asked, so nothing was searched.
    ///
    /// A FOURTH state, and not a kind of empty result. An empty box is
    /// not a search that found nothing, and over a complete index the
    /// alternative would be `None` -- a confident statement that the
    /// corpus does not contain something nobody asked about, painted
    /// under an empty search box.
    ///
    /// It carries the coverage like every other verdict, so the page can
    /// still say how much is searchable before anyone types.
    NotAsked,
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
    #[serde(default)]
    pub population: Option<super::transcript::PopulationProgress>,
    /// Sessions whose content is in the index right now.
    pub indexed: u64,
    /// Session transcripts the last scan found on disk. The
    /// denominator, and NOT a guess: it is what
    /// [`super::transcript::scan`] counted.
    ///
    /// Zero is measured empty only when stored population and state agree
    /// on a completed pass. Otherwise the denominator remains unknown.
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
    /// When the last index pass started, RFC 3339, or `None` if absent.
    /// This timestamp does not prove that the pass completed.
    pub last_indexed_at: Option<String>,
}

impl Coverage {
    /// Whether the stored observations license a whole-corpus no-match.
    /// This describes the last observed corpus, never a fresh disk census.
    /// Truncation is qualified separately, both in coverage and on each hit.
    pub fn is_complete(&self) -> bool {
        self.indexed == self.total
            && self.unreadable.is_empty()
            && self.population.as_ref().is_some_and(|p| {
                p.status == super::transcript::PopulationStatus::Complete
                    && p.total_known
                    && p.total == self.total
                    && p.completed == self.total
                    && p.remaining == 0
                    && p.failed == 0
            })
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
    #[serde(default)]
    pub population: super::transcript::PopulationProgress,
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
        self.population.status != super::transcript::PopulationStatus::Complete
            || !self.unreadable.is_empty()
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
        out.extend(self.write_failures.iter().cloned());
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
///
/// Returns the text twice: as written, and masked as a phone would be
/// shown it (#1519), each string masked on its own -- see [`Readable`].
fn readable_text_controlled(
    path: &Path,
    budget: u64,
    control: &super::transcript::PopulationControl,
) -> Result<Readable, IndexReadError> {
    let file = std::fs::File::open(path).map_err(|e| {
        IndexReadError::Fault(format!("{}: could not open it: {e}", path.display()))
    })?;
    let reader = super::transcript::ControlledReader {
        inner: file,
        control,
    };
    let mut reader = BufReader::new(std::io::Read::take(reader, budget));

    let mut out = Readable::default();
    let mut read: u64 = 0;
    let mut line = String::new();
    loop {
        if control.stopped() {
            return Err(IndexReadError::Stopped(control.stop_reason().unwrap()));
        }
        line.clear();
        let n = match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(n) => n,
            // One undecodable line ends the read rather than failing the
            // session: whatever was gathered is still true text, and it
            // is recorded as truncated so the shortfall is visible.
            Err(e) => {
                if let Some(reason) = super::transcript::stopped_error(&e) {
                    return Err(IndexReadError::Stopped(reason));
                }
                if e.kind() == std::io::ErrorKind::InvalidData {
                    out.truncated = true;
                    break;
                }
                return Err(IndexReadError::Fault(format!(
                    "{}: could not read it: {e}",
                    path.display()
                )));
            }
        };
        read += n as u64;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
            collect_text(&v, &mut out);
        }
        if read >= budget {
            out.truncated = true;
            break;
        }
    }
    Ok(out)
}

#[derive(Debug)]
enum IndexReadError {
    Stopped(super::transcript::PopulationStatus),
    Fault(String),
}

/// One transcript's searchable text, as [`readable_text_controlled`] extracts it.
#[derive(Debug, Default)]
struct Readable {
    /// The text as written: what the desktop's own search matches.
    body: String,
    /// The same text as `remote::privacy::mask_text` leaves it: what a
    /// phone's search matches (#1519). Masked string by string, the way
    /// a phone is shown each block, rather than as one concatenation --
    /// a private key clamped at the end of one prompt would otherwise
    /// swallow every prompt after it.
    body_masked: String,
    /// Read only to the budget.
    truncated: bool,
}

impl Readable {
    fn push(&mut self, s: &str) {
        self.body.push_str(s);
        self.body.push('\n');
        self.body_masked
            .push_str(&crate::remote::privacy::mask_text(s).0);
        self.body_masked.push('\n');
    }
}

/// Pull the human-readable strings out of one transcript record.
///
/// Claude Code writes `message.content` as either a bare string or an
/// array of typed blocks, and the text lives in `text` fields either
/// way. Tool inputs and results are deliberately left out: they are
/// mostly file contents and command output, which would dominate the
/// index and make a search for a word the user typed return every
/// session that ever read a file containing it.
fn collect_text(v: &serde_json::Value, out: &mut Readable) {
    let Some(msg) = v.get("message") else { return };
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => out.push(s),
        Some(serde_json::Value::Array(blocks)) => {
            for b in blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(s) = b.get("text").and_then(|t| t.as_str()) {
                        out.push(s);
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
    index_pass_controlled(conn, scan, &super::transcript::PopulationControl::default())
}

pub(crate) fn index_pass_controlled(
    conn: &mut Connection,
    scan: &super::Scan,
    control: &super::transcript::PopulationControl,
) -> Result<Indexed, rusqlite::Error> {
    let generation = super::transcript::population_generation(conn, "index")?;
    let _activity = super::transcript::PopulationActivity::new(conn, "index", generation);

    let started = std::time::Instant::now();
    let mut out = Indexed {
        population: scan.population_progress(),
        // Carried through from the scan rather than dropped at this
        // boundary, for the reason the field's own doc gives: a session
        // the scan could not read is as absent from the index as one
        // this pass could not read.
        scan_unreadable: scan
            .unreadable_files
            .iter()
            .chain(&scan.unreadable_dirs)
            .cloned()
            .collect(),
        ..Default::default()
    };
    out.population.phase = super::transcript::PopulationPhase::Index;
    if let Some(reason) = scan.stop_reason {
        out.population.status = reason;
    }
    super::transcript::persist_progress(conn, "index", generation, &out.population)?;
    let now = chrono::Utc::now().to_rfc3339();

    // The corpus size this pass observed, recorded FIRST and
    // unconditionally. It is the denominator every coverage sentence
    // uses, and recording it only on a successful index would leave a
    // machine whose every transcript is unreadable reporting "0 of 0"
    // -- a complete index of nothing, which is exactly the reassuring
    // lie this module exists to prevent.
    conn.execute(
        "INSERT INTO claude_index_state (id, corpus_sessions, last_pass_at, incomplete)
         VALUES (1, ?1, ?2, 1)
         ON CONFLICT(id) DO UPDATE SET corpus_sessions = CASE WHEN ?3 THEN ?1 ELSE MAX(corpus_sessions, ?1) END, last_pass_at = ?2, incomplete = 1",
        // The denominator counts the transcripts that EXIST, not the
        // ones that parsed. A file the scan could not read is still a
        // session on disk the user may be searching for, and leaving it
        // out of the denominator would let "1 of 1 indexed" be reported
        // on a machine holding two transcripts.
        rusqlite::params![
            (scan.sessions.len() + scan.unreadable_files.len()) as i64,
            &now,
            out.population.total_known
        ],
    )?;

    // What the ledger already holds, read once rather than queried per
    // session: 1,482 point lookups against a 60-second budget is work
    // that buys nothing over one scan of a table with one row per
    // session.
    let mut known: std::collections::HashMap<String, (i64, String, String)> =
        std::collections::HashMap::new();
    {
        let mut q = conn.prepare(
            "SELECT session_id, size_bytes, mtime_ns, source_path FROM claude_index_ledger",
        )?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, size, mtime, path) = row?;
            known.insert(id, (size, mtime, path));
        }
    }

    // Which sessions need work, decided before any file is opened.
    let mut todo: Vec<(&super::Transcript, i64, String)> = Vec::new();
    for t in &scan.sessions {
        let path = Path::new(&t.path);
        // A transcript whose metadata cannot be read is a known gap, not
        // a skip: it is reported here rather than silently left out of
        // the index, because a session missing from the index for an
        // unstated reason becomes an unexplained "no matches" later.
        let (size, mtime) = match super::transcript::fingerprint(path) {
            Ok(v) => v,
            Err(e) => {
                out.unreadable
                    .push(format!("{}: could not read its size: {e}", t.path));
                continue;
            }
        };
        match known.get(&t.session_id) {
            Some((s, m, p)) if *s == size && *m == mtime && *p == t.path => out.unchanged += 1,
            _ => todo.push((t, size, mtime)),
        }
    }

    out.remaining = todo.len().saturating_sub(SESSIONS_PER_PASS);

    super::retry_queue::rotate_pending(conn, "index", &mut todo, |item| &item.0.session_id)?;

    for (t, size, mtime) in todo.into_iter().take(SESSIONS_PER_PASS) {
        if control.stopped() || !super::transcript::generation_current(conn, "index", generation)? {
            out.population.status = control
                .stop_reason()
                .unwrap_or(super::transcript::PopulationStatus::Invalidated);
            finalize_index(conn, generation, &mut out)?;
            return Ok(out);
        }
        if !super::retry_queue::checkpoint_attempt(conn, "index", generation, &t.session_id)? {
            out.population.status = super::transcript::PopulationStatus::Invalidated;
            finalize_index(conn, generation, &mut out)?;
            return Ok(out);
        }
        let text = match readable_text_controlled(Path::new(&t.path), INDEX_BUDGET_BYTES, control) {
            Ok(v) => v,
            Err(IndexReadError::Stopped(reason)) => {
                out.population.status = reason;
                finalize_index(conn, generation, &mut out)?;
                return Ok(out);
            }
            Err(IndexReadError::Fault(e)) => {
                out.unreadable.push(e);
                continue;
            }
        };
        let truncated = text.truncated;
        if truncated {
            out.truncated += 1;
        }

        // File I/O precedes writer admission. Dropping a failed transaction
        // rolls back the body and its fingerprint together.
        match super::transcript::fingerprint(Path::new(&t.path)) {
            Ok(after) if after == (size, mtime.clone()) => {}
            Ok(_) => {
                out.population.status = super::transcript::PopulationStatus::Invalidated;
                continue;
            }
            Err(error) => {
                out.unreadable.push(format!(
                    "{}: could not validate indexed source: {error}",
                    t.path
                ));
                continue;
            }
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if control.stopped() || !super::transcript::generation_current(&tx, "index", generation)? {
            out.population.status = control
                .stop_reason()
                .unwrap_or(super::transcript::PopulationStatus::Invalidated);
            drop(tx);
            finalize_index(conn, generation, &mut out)?;
            return Ok(out);
        }
        // FTS5 has no partial-row update, so a re-index deletes and
        // re-inserts. Both statements in the same transaction, so a
        // reader never sees the gap between them -- a session that
        // vanished mid-pass would otherwise read as "indexed, no
        // content", which is a false negative dressed as a real answer.
        if let Err(e) = remove_index_rows(&tx, &t.session_id) {
            out.write_failures.push(format!(
                "{}: could not clear its old index: {e}",
                t.session_id
            ));
            continue;
        }
        if let Err(e) = tx.execute(
            "INSERT INTO claude_transcript_fts (session_id, body, body_masked)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![&t.session_id, &text.body, &text.body_masked],
        ) {
            out.write_failures
                .push(format!("{}: could not index it: {e}", t.session_id));
            continue;
        }
        if let Err(e) = tx.execute(
            "INSERT INTO claude_fts_identity(rowid, session_id) VALUES (last_insert_rowid(), ?1)",
            [&t.session_id],
        ) {
            out.write_failures.push(format!(
                "{}: could not record its index identity: {e}",
                t.session_id
            ));
            continue;
        }
        if let Err(e) = tx.execute(
            "INSERT INTO claude_index_ledger
                (session_id, size_bytes, mtime_ms, truncated, indexed_at, mtime_ns, source_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(session_id) DO UPDATE SET
                size_bytes = ?2, mtime_ms = ?3, truncated = ?4, indexed_at = ?5, mtime_ns = ?6, source_path = ?7",
            rusqlite::params![&t.session_id, size, (mtime.parse::<u128>().unwrap_or_default() / 1_000_000) as i64, truncated as i64, &now, &mtime, &t.path],
        ) {
            out.write_failures.push(format!(
                "{}: could not record that it is indexed: {e}",
                t.session_id
            ));
            continue;
        }
        out.indexed += 1;
        out.population.completed = (out.indexed + out.unchanged) as u64;
        super::transcript::persist_progress(&tx, "index", generation, &out.population)?;
        tx.commit()?;
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
    let mut discovery_current = scan.stop_reason.is_none();
    for (path, size, modified) in &scan.source_fingerprints {
        match super::transcript::discovery_fingerprint(path, control) {
            Ok(actual) if actual == (*size, modified.clone()) => {}
            Ok(_) => {
                discovery_current = false;
            }
            Err(error) => {
                discovery_current = false;
                if let Some(reason) = super::transcript::stopped_error(&error) {
                    out.population.status = reason;
                    break;
                }
                out.scan_unreadable
                    .push(format!("{}: discovery validation: {error}", path.display()));
            }
        }
    }
    if !discovery_current && out.population.status == super::transcript::PopulationStatus::Complete
    {
        out.population.status = super::transcript::PopulationStatus::Invalidated;
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if control.stopped() || !super::transcript::generation_current(&tx, "index", generation)? {
        out.population.status = control
            .stop_reason()
            .unwrap_or(super::transcript::PopulationStatus::Invalidated);
        drop(tx);
        finalize_index(conn, generation, &mut out)?;
        return Ok(out);
    }
    for id in stale
        .into_iter()
        .filter(|_| discovery_current && !scan.is_partial() && scan.absent_root.is_none())
    {
        remove_index_rows(&tx, &id)?;
        tx.execute(
            "DELETE FROM claude_index_ledger WHERE session_id = ?1",
            rusqlite::params![&id],
        )?;
    }

    tx.execute(
        "UPDATE claude_index_state SET incomplete=?1 WHERE id=1",
        [i64::from(out.is_partial() || out.remaining > 0)],
    )?;
    tx.commit()?;
    out.elapsed_ms = started.elapsed().as_millis() as u64;
    finalize_index(conn, generation, &mut out)?;
    Ok(out)
}

/// Called only inside the caller's fenced transaction, alongside body/ledger
/// publication. Lookup is indexed and retains historical duplicate identities.
fn remove_index_rows(conn: &Connection, session_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM claude_transcript_fts WHERE rowid IN
            (SELECT rowid FROM claude_fts_identity WHERE session_id = ?1)",
        [session_id],
    )?;
    conn.execute(
        "DELETE FROM claude_fts_identity WHERE session_id = ?1",
        [session_id],
    )?;
    Ok(())
}

fn finalize_index(conn: &Connection, generation: i64, out: &mut Indexed) -> rusqlite::Result<()> {
    out.population.completed = (out.indexed + out.unchanged) as u64;
    out.population.unchanged = out.unchanged as u64;
    out.population.failed = (out.unreadable.len() + out.write_failures.len()) as u64;
    out.population.finish();
    super::transcript::persist_progress(conn, "index", generation, &out.population)
}

/// Reuse caller-owned transactions without ending them. Standalone reads own
/// a deferred transaction so every SELECT, including FTS, sees one commit.
fn read_snapshot<T>(
    conn: &Connection,
    read: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    if conn.is_autocommit() {
        let tx =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)?;
        let result = read(&tx)?;
        tx.commit()?;
        Ok(result)
    } else {
        read(conn)
    }
}

/// What the index currently covers.
///
/// Read separately from a query so a UI can state coverage before a
/// search is run -- "1,482 sessions searchable" is worth saying on an
/// empty search box, and a number that only appears alongside results
/// cannot be shown then.
pub fn coverage(conn: &Connection, unreadable: Vec<String>) -> Result<Coverage, rusqlite::Error> {
    read_snapshot(conn, |conn| coverage_in_snapshot(conn, unreadable))
}

fn coverage_in_snapshot(conn: &Connection, unreadable: Vec<String>) -> rusqlite::Result<Coverage> {
    let indexed: i64 =
        conn.query_row("SELECT COUNT(*) FROM claude_index_ledger", [], |r| r.get(0))?;
    let truncated: i64 = conn.query_row(
        "SELECT COALESCE(SUM(truncated), 0) FROM claude_index_ledger",
        [],
        |r| r.get(0),
    )?;
    let state: Option<(i64, Option<String>, bool)> = conn
        .query_row(
            "SELECT corpus_sessions, last_pass_at, incomplete FROM claude_index_state WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (total, last_indexed_at, incomplete) = state.unwrap_or((0, None, true));
    let mut population = super::transcript::stored_progress(conn, "index")?;
    if let Some(p) = &mut population {
        // A complete progress row can coexist with unfinished state, for
        // example between commits of a new background pass. Qualify that
        // readback without inventing file failures or writing a new pass.
        if p.status == super::transcript::PopulationStatus::Complete
            && (incomplete
                || indexed != total
                || p.total != total as u64
                || !p.total_known
                || p.completed != p.total
                || p.failed > 0
                || p.remaining > 0)
        {
            p.status = super::transcript::PopulationStatus::Deferred;
        }
    }

    Ok(Coverage {
        population,
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

/// The index column a query is confined to, and its position for
/// `snippet()`, by [`Matching`] (#1519).
///
/// Confined explicitly both ways: an unfiltered FTS5 query searches
/// EVERY indexed column, so the desktop's query would also hit the
/// marker words (`hidden`, `api`, `key`) in `body_masked`.
fn column(matching: Matching) -> (&'static str, i64) {
    match matching {
        Matching::Unmasked => ("body", 1),
        Matching::Masked => ("body_masked", 2),
    }
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
///
/// `matching` picks the text the query is matched against (#1519): the
/// real text for the desktop's window, the masked copy for a phone, so a
/// phone's hit or miss says nothing about a secret. Coverage is the same
/// either way -- both columns are written in one row, by one pass.
pub fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
    unreadable: Vec<String>,
    matching: Matching,
) -> Result<SearchAnswer, rusqlite::Error> {
    read_snapshot(conn, |conn| {
        search_in_snapshot(conn, query, limit, unreadable, matching)
    })
}

fn search_in_snapshot(
    conn: &Connection,
    query: &str,
    limit: usize,
    unreadable: Vec<String>,
    matching: Matching,
) -> Result<SearchAnswer, rusqlite::Error> {
    let cov = coverage_in_snapshot(conn, unreadable)?;

    let Some(expr) = fts_query(query) else {
        // An empty query is not a search that found nothing. It is not a
        // search at all, so it gets its own verdict: over a COMPLETE
        // index, `none_verdict` here would return `Verdict::None` and
        // the page would paint "No matches. All 1,494 sessions were
        // searched." under an empty box -- a confident answer to a
        // question nobody asked. The coverage still travels, so the page
        // can say how much is searchable before anyone types.
        return Ok(SearchAnswer {
            verdict: Verdict::NotAsked,
            coverage: cov,
        });
    };

    let (col, col_index) = column(matching);
    let expr = format!("{{{col}}} : ({expr})");
    let mut q = conn.prepare(
        "SELECT f.session_id,
                snippet(claude_transcript_fts, ?3, '[', ']', ' … ', 24),
                COALESCE(l.truncated, 0)
           FROM claude_transcript_fts f
           LEFT JOIN claude_index_ledger l ON l.session_id = f.session_id
          WHERE claude_transcript_fts MATCH ?1
          ORDER BY rank
          LIMIT ?2",
    )?;
    let rows = q.query_map(rusqlite::params![&expr, limit as i64, col_index], |r| {
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

    /// Manually seeded complete-index fixtures need the same persisted
    /// completion evidence as a real pass, not merely matching row counts.
    fn completed_progress(conn: &Connection, total: u64) {
        use super::super::transcript::{
            persist_progress, population_generation, PopulationPhase, PopulationProgress,
        };
        let generation = population_generation(conn, "index").unwrap();
        persist_progress(
            conn,
            "index",
            generation,
            &PopulationProgress {
                phase: PopulationPhase::Index,
                total,
                total_known: true,
                completed: total,
                ..Default::default()
            },
        )
        .unwrap();
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

    // Synchronize at preparation of the SECOND SELECT: COUNT has already
    // stepped and returned, so a deferred transaction has acquired its snapshot.
    // An authorizer on the first SELECT would only test BEGIN, not a snapshot.
    fn writer_after_first_read(
        reader: &Connection,
        writer: impl FnOnce() + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        let (start_tx, start_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            start_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            writer();
            done_tx.send(()).unwrap();
        });
        let mut fired = false;
        reader
            .authorizer(Some(move |ctx: AuthContext<'_>| {
                if !fired
                    && matches!(
                        ctx.action,
                        AuthAction::Read {
                            table_name: "claude_index_ledger",
                            column_name: "truncated"
                        }
                    )
                {
                    fired = true;
                    start_tx.send(()).unwrap();
                    done_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap();
                }
                Authorization::Allow
            }))
            .unwrap();
        handle
    }

    #[test]
    fn stored_readback_uses_one_snapshot_and_next_read_sees_writer() {
        for mode in ["search", "coverage", "blank", "masked", "caller"] {
            let dir = tempfile::tempdir().unwrap();
            let corpus = tempfile::tempdir().unwrap();
            let path = dir.path().join("index.sqlite");
            let mut reader = Connection::open(&path).unwrap();
            crate::store::migrate(&reader).unwrap();
            reader.pragma_update(None, "journal_mode", "WAL").unwrap();
            write_session(corpus.path(), "project", "old", "needle oldversion");
            index_pass(&mut reader, &crate::claude::corpus(corpus.path())).unwrap();
            let before = coverage(&reader, vec![]).unwrap();
            reader.pragma_update(None, "query_only", true).unwrap();
            if mode == "caller" {
                reader.execute_batch("BEGIN").unwrap();
            }
            let writer = writer_after_first_read(&reader, move || {
                let mut writer = Connection::open(path).unwrap();
                write_session(corpus.path(), "project", "new", "needle newversion");
                index_pass(&mut writer, &crate::claude::corpus(corpus.path())).unwrap();
                writer
                    .execute("UPDATE claude_index_ledger SET truncated=1", [])
                    .unwrap();
                // All readback below must remain independent of these sources.
                corpus.close().unwrap();
            });
            let matching = if mode == "masked" {
                Matching::Masked
            } else {
                Matching::Unmasked
            };
            let cov = if mode == "coverage" {
                coverage(&reader, vec![]).unwrap()
            } else {
                let answer = search(
                    &reader,
                    if mode == "blank" { "" } else { "needle" },
                    20,
                    vec![],
                    matching,
                )
                .unwrap();
                if mode == "blank" {
                    assert_eq!(answer.verdict, Verdict::NotAsked);
                } else {
                    let Verdict::Matches { hits } = answer.verdict else {
                        panic!("stored match missing")
                    };
                    assert_eq!(
                        hits.len(),
                        1,
                        "{mode}: hits must share the coverage snapshot"
                    );
                    assert_eq!(hits[0].session_id, "old");
                    assert!(!hits[0].truncated);
                }
                answer.coverage
            };
            writer.join().unwrap();
            assert_eq!(
                cov, before,
                "{mode}: every coverage field must share the first SELECT snapshot"
            );
            if mode == "caller" {
                assert!(!reader.is_autocommit());
                assert_eq!(coverage(&reader, vec![]).unwrap(), before);
                reader.execute_batch("ROLLBACK").unwrap();
            }
            assert!(reader.is_autocommit());
            let after = search(&reader, "needle", 20, vec![], matching).unwrap();
            assert_eq!(after.coverage.indexed, 2);
            assert_eq!(after.coverage.total, 2);
            assert_eq!(after.coverage.truncated, 2);
            assert_eq!(after.coverage.population.unwrap().completed, 2);
            let Verdict::Matches { hits } = after.verdict else {
                panic!("new match missing")
            };
            assert_eq!(hits.len(), 2);
            assert!(hits.iter().all(|h| h.truncated));
        }
    }

    #[test]
    fn stored_readback_preserves_caller_transaction_on_success_and_error() {
        let conn = db();
        conn.execute_batch("BEGIN; INSERT INTO claude_index_state(id,corpus_sessions,last_pass_at) VALUES(1,7,'caller');").unwrap();
        assert_eq!(coverage(&conn, vec![]).unwrap().total, 7);
        search(&conn, "needle", 20, vec![], Matching::Unmasked).unwrap();
        assert!(!conn.is_autocommit());
        conn.execute_batch("DROP TABLE claude_transcript_fts")
            .unwrap();
        assert!(search(&conn, "needle", 20, vec![], Matching::Unmasked).is_err());
        assert!(!conn.is_autocommit());
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(coverage(&conn, vec![]).unwrap().total, 0);
        conn.execute_batch("DROP TABLE claude_transcript_fts")
            .unwrap();
        assert!(search(&conn, "needle", 20, vec![], Matching::Unmasked).is_err());
        assert!(
            conn.is_autocommit(),
            "owned failed reads must release their snapshot"
        );
    }

    #[test]
    fn stored_readback_requires_state_and_progress_agreement() {
        use super::super::transcript::PopulationStatus::*;
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "project", "old", "needle");
        let mut conn = db();
        let initial = coverage(&conn, vec![]).unwrap();
        assert!(!initial.is_complete());
        assert!(
            initial.unreadable.is_empty(),
            "unknown coverage is not an unreadable transcript"
        );
        index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        let complete = coverage(&conn, vec![]).unwrap();
        assert!(complete.is_complete());
        let progress = complete.population.unwrap();
        let set_progress = |p: &super::super::transcript::PopulationProgress| {
            conn.execute(
                "UPDATE claude_population_state SET payload=?1 WHERE lane='index'",
                [serde_json::to_string(p).unwrap()],
            )
            .unwrap();
        };
        for status in [Running, Deferred, Cancelled, TimedOut, Failed, Invalidated] {
            let mut p = progress.clone();
            p.status = status;
            set_progress(&p);
            let answer = search(&conn, "absent", 20, vec![], Matching::Masked).unwrap();
            assert!(
                matches!(answer.verdict, Verdict::NoneYet { .. }),
                "{status:?}"
            );
            assert!(answer.coverage.unreadable.is_empty());
        }
        for case in [
            "unknown",
            "failed",
            "remaining",
            "mismatched_total",
            "incomplete_state",
            "retained_rows",
            "missing_state",
            "legacy",
        ] {
            set_progress(&progress);
            conn.execute(
                "INSERT INTO claude_index_state(id,corpus_sessions,incomplete,last_pass_at) VALUES(1,1,0,'fixture') ON CONFLICT(id) DO UPDATE SET incomplete=0,corpus_sessions=1",
                [],
            )
            .unwrap();
            let mut p = progress.clone();
            match case {
                "unknown" => p.total_known = false,
                "failed" => p.failed = 1,
                "remaining" => p.remaining = 1,
                "mismatched_total" => p.total = 2,
                "incomplete_state" => {
                    conn.execute("UPDATE claude_index_state SET incomplete=1", [])
                        .unwrap();
                }
                "retained_rows" => {
                    conn.execute("UPDATE claude_index_state SET corpus_sessions=0", [])
                        .unwrap();
                    p.total = 0;
                    p.completed = 0;
                }
                "missing_state" => {
                    conn.execute("DELETE FROM claude_index_state", []).unwrap();
                }
                "legacy" => {}
                _ => unreachable!(),
            }
            set_progress(&p);
            if case == "legacy" {
                conn.execute("DELETE FROM claude_population_state", [])
                    .unwrap();
            }
            let cov = coverage(&conn, vec![]).unwrap();
            assert!(!cov.is_complete(), "{case}");
            assert!(
                cov.unreadable.is_empty(),
                "{case}: do not fabricate file failures"
            );
            assert_eq!(cov.pending(), 0);
        }
    }

    #[test]
    fn stored_readback_distinguishes_measured_empty_from_uninitialized() {
        let mut conn = db();
        assert!(!coverage(&conn, vec![]).unwrap().is_complete());
        let dir = tempfile::tempdir().unwrap();
        index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        let answer = search(&conn, "absent", 20, vec![], Matching::Unmasked).unwrap();
        assert!(answer.coverage.is_complete());
        assert_eq!(answer.verdict, Verdict::None);
    }

    #[test]
    fn stopped_discovery_preserves_last_total_and_never_licenses_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        for id in ["first", "second"] {
            write_session(dir.path(), "project", id, "accepted");
        }
        let mut conn = db();
        index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        assert!(coverage(&conn, Vec::new()).unwrap().is_complete());
        write_session(dir.path(), "project", "new", "unseenneedle");
        for reason in [
            super::super::transcript::PopulationStatus::Cancelled,
            super::super::transcript::PopulationStatus::TimedOut,
        ] {
            let scan = crate::claude::Scan {
                stop_reason: Some(reason),
                ..Default::default()
            };
            let done = index_pass_controlled(
                &mut conn,
                &scan,
                &super::super::transcript::PopulationControl::discovery(),
            )
            .unwrap();
            assert_eq!(done.population.status, reason);
            assert!(!done.population.total_known);
            let cov = coverage(&conn, Vec::new()).unwrap();
            assert_eq!(cov.total, 2);
            assert!(!cov.is_complete());
            let answer = search(&conn, "unseenneedle", 20, Vec::new(), Matching::Unmasked).unwrap();
            assert!(!matches!(answer.verdict, Verdict::None));
        }
    }

    #[test]
    fn bounded_corpus_converges_without_reporting_background_failures() {
        use crate::background::{IndexOutcome, Task};
        let dir = tempfile::tempdir().unwrap();
        for n in 0..205 {
            write_session(dir.path(), "project", &format!("s{n:03}"), "needle");
        }
        let scan = crate::claude::corpus(dir.path());
        let mut conn = db();
        let task = Task::new("fixture-index");
        let control = super::super::transcript::PopulationControl::discovery();
        for pass in 1..=3 {
            let done = index_pass_controlled(&mut conn, &scan, &control).unwrap();
            let outcome = IndexOutcome::from_index(Ok(&done));
            assert_eq!(
                outcome,
                if pass < 3 {
                    IndexOutcome::Deferred
                } else {
                    IndexOutcome::Healthy
                }
            );
            assert!(!outcome.record_health(&task, pass));
            assert_eq!(task.snapshot().total_failures, 0);
            assert_eq!(task.snapshot().last_success_ms, Some(pass));
        }
        assert!(coverage(&conn, Vec::new()).unwrap().is_complete());
    }

    #[test]
    fn stopped_discovery_is_neutral_but_prior_real_scan_fault_is_not() {
        use crate::background::IndexOutcome;
        let dir = tempfile::tempdir().unwrap();
        let control = super::super::transcript::PopulationControl::discovery();
        control.cancel();
        let mut scan = super::super::transcript::corpus_controlled(dir.path(), &control);
        assert!(scan.is_partial());
        assert!(scan.unreadable_dirs.is_empty());
        let mut conn = db();
        let done = index_pass_controlled(&mut conn, &scan, &control).unwrap();
        assert_eq!(IndexOutcome::from_index(Ok(&done)), IndexOutcome::Cancelled);
        scan.unreadable_dirs
            .push("project: permission denied".into());
        let done = index_pass_controlled(&mut conn, &scan, &control).unwrap();
        assert!(matches!(
            IndexOutcome::from_index(Ok(&done)),
            IndexOutcome::Failed(_)
        ));
    }

    #[test]
    fn stale_listing_discovery_race_cannot_delete_newer_index() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "project", "old", "oldneedle");
        let old = crate::claude::corpus(dir.path());
        write_session(dir.path(), "project", "new", "newneedle");
        let mut conn = db();
        index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        let result = index_pass(&mut conn, &old).unwrap();
        assert!(result.is_partial());
        assert_eq!(
            crate::background::IndexOutcome::from_index(Ok(&result)),
            crate::background::IndexOutcome::Invalidated
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM claude_index_ledger WHERE session_id='new'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn cancellation_retains_accepted_index_and_persists_incomplete_coverage() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "project", "old", "acceptedneedle");
        let mut conn = db();
        let scan = crate::claude::corpus(dir.path());
        index_pass(&mut conn, &scan).unwrap();
        let control = super::super::transcript::PopulationControl::default();
        control.cancel();
        let result = index_pass_controlled(&mut conn, &scan, &control).unwrap();
        assert!(result.is_partial());
        assert!(!coverage(&conn, Vec::new()).unwrap().is_complete());
        let resumed = index_pass(&mut conn, &scan).unwrap();
        assert_eq!(resumed.unchanged, 1);
        assert_eq!(resumed.indexed, 0);
        assert!(coverage(&conn, Vec::new()).unwrap().is_complete());
    }

    #[test]
    fn partial_discovery_preserves_prior_index_and_coverage() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "project", "old", "retainedneedle");
        let mut conn = db();
        index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        let partial = crate::claude::Scan {
            unreadable_dirs: vec!["project: permission denied".into()],
            ..Default::default()
        };
        let result = index_pass(&mut conn, &partial).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM claude_transcript_fts WHERE claude_transcript_fts MATCH 'retainedneedle'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
        assert!(!coverage(&conn, result.all_unreadable())
            .unwrap()
            .is_complete());
    }

    #[test]
    fn failed_prefix_does_not_starve_index_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..101 {
            write_session(dir.path(), "project", &format!("s{n:03}"), "fairneedle");
        }
        let mut scan = crate::claude::corpus(dir.path());
        scan.sessions
            .sort_by(|a, b| a.session_id.cmp(&b.session_id));
        let database = dir.path().join("fair.db");
        let mut conn = crate::store::open_db(&database).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_prefix BEFORE INSERT ON claude_index_ledger WHEN NEW.session_id <> 's100' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
        // Count/restart fairness is independent of wall-clock runner speed.
        let control = super::super::transcript::PopulationControl::discovery();
        let first = index_pass_controlled(&mut conn, &scan, &control).unwrap();
        assert_eq!(first.indexed, 0);
        assert_eq!(first.write_failures.len(), 100);
        drop(conn);
        let mut conn = crate::store::open_db(&database).unwrap();
        let second = index_pass_controlled(&mut conn, &scan, &control).unwrap();
        assert_eq!(
            second.indexed, 1,
            "a failed prefix must not hide the stable last file after restart"
        );
        assert!(second.is_partial());
        conn.execute_batch("DROP TRIGGER fail_prefix").unwrap();
        index_pass_controlled(&mut conn, &scan, &control).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM claude_index_ledger", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            101
        );
    }

    #[test]
    fn failed_ledger_write_rolls_back_body_and_commits_neighbor() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "project", "old", "retainedneedle");
        let mut conn = db();
        index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        write_session(dir.path(), "project", "old", "replacementneedle longer");
        write_session(dir.path(), "project", "neighbor", "neighborneedle");
        conn.execute_batch("CREATE TRIGGER reject_ledger BEFORE UPDATE ON claude_index_ledger WHEN NEW.session_id = 'old' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        let result = index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        assert_eq!(result.indexed, 1);
        assert_eq!(result.write_failures.len(), 1);
        let body: String = conn
            .query_row(
                "SELECT body FROM claude_transcript_fts WHERE session_id = 'old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(body.contains("retainedneedle"), "{body}");
    }

    #[test]
    fn identity_failure_rolls_back_body_and_ledger() {
        let dir = tempfile::tempdir().unwrap();
        write_session(dir.path(), "project", "old", "retainedneedle");
        let mut conn = db();
        index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        let identity: i64 = conn
            .query_row(
                "SELECT rowid FROM claude_fts_identity WHERE session_id='old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        write_session(dir.path(), "project", "old", "replacementneedle longer");
        conn.execute_batch("CREATE TRIGGER reject_identity BEFORE INSERT ON claude_fts_identity BEGIN SELECT RAISE(ABORT, 'synthetic identity failure'); END;").unwrap();
        let result = index_pass(&mut conn, &crate::claude::corpus(dir.path())).unwrap();
        assert_eq!(result.indexed, 0);
        assert_eq!(result.write_failures.len(), 1);
        assert_eq!(
            conn.query_row(
                "SELECT rowid FROM claude_fts_identity WHERE session_id='old'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            identity
        );
        let body: String = conn
            .query_row(
                "SELECT body FROM claude_transcript_fts WHERE rowid=?1",
                [identity],
                |r| r.get(0),
            )
            .unwrap();
        assert!(body.contains("retainedneedle"));
        conn.execute_batch("DROP TRIGGER reject_identity").unwrap();
        assert_eq!(
            index_pass(&mut conn, &crate::claude::corpus(dir.path()))
                .unwrap()
                .indexed,
            1
        );
    }

    #[test]
    fn direct_identity_deletion_removes_duplicates_but_not_neighbors() {
        let mut conn = db();
        for (id, body) in [("same", "first"), ("same", "second"), ("neighbor", "safe")] {
            conn.execute(
                "INSERT INTO claude_transcript_fts(session_id, body) VALUES (?1, ?2)",
                [id, body],
            )
            .unwrap();
            conn.execute("INSERT INTO claude_fts_identity(rowid, session_id) VALUES(last_insert_rowid(), ?1)", [id]).unwrap();
        }
        let tx = conn.transaction().unwrap();
        remove_index_rows(&tx, "same").unwrap();
        tx.commit().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM claude_fts_identity", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT body FROM claude_transcript_fts", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "safe"
        );
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

        let answer = search(&conn, "kubernetes", 20, vec![], Matching::Unmasked).unwrap();

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

        completed_progress(&conn, 2);

        let answer = search(&conn, "kubernetes", 20, vec![], Matching::Unmasked).unwrap();
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
        let partial = search(&conn, "nothing", 20, vec![], Matching::Unmasked)
            .unwrap()
            .verdict;

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
        completed_progress(&conn, 1);
        let complete = search(&conn, "nothing", 20, vec![], Matching::Unmasked)
            .unwrap()
            .verdict;

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
        let answer = search(&conn, "anything", 20, vec![], Matching::Unmasked).unwrap();
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

        let answer = search(&conn, "kubernetes", 20, done.unreadable, Matching::Unmasked).unwrap();
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
            search(
                &conn,
                "kubernetes",
                20,
                done.all_unreadable(),
                Matching::Unmasked
            )
            .unwrap()
            .verdict,
            Verdict::NoneYet { .. }
        ));

        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// The cheap listing and the full scan agree about the corpus
    /// (#1246).
    ///
    /// THE test for the substitution. The live pass now feeds
    /// `index_pass` a `corpus()` listing rather than a `scan()`, and
    /// the only thing that makes that safe is that the two agree about
    /// the three things `index_pass` reads: the session ids, the paths,
    /// and the unreadable set.
    ///
    /// Asserted as an equality between the two functions rather than
    /// against a literal count, so a future change to either walk has
    /// to keep them in step or fail here. A listing that found fewer
    /// sessions would shrink the denominator, and a shrunken
    /// denominator is precisely how an incomplete index starts calling
    /// itself complete.
    #[test]
    fn the_listing_and_the_scan_agree_about_the_corpus() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(root, "alpha", "s1", "fsevents stream died");
        write_session(root, "alpha", "s2", "kubernetes rollout");
        write_session(root, "beta", "s3", "retry backoff");
        // A subagent transcript, which neither must count.
        let nested = root.join("alpha").join("s1").join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("agent-x.jsonl"), "{}\n").unwrap();

        let listed = crate::claude::corpus(root);
        let scanned = crate::claude::scan(root);

        let ids = |s: &crate::claude::Scan| {
            let mut v: Vec<_> = s.sessions.iter().map(|t| t.session_id.clone()).collect();
            v.sort();
            v
        };
        let paths = |s: &crate::claude::Scan| {
            let mut v: Vec<_> = s.sessions.iter().map(|t| t.path.clone()).collect();
            v.sort();
            v
        };
        assert_eq!(ids(&listed), ids(&scanned), "the same session ids");
        assert_eq!(paths(&listed), paths(&scanned), "the same paths");
        assert_eq!(listed.unreadable_files, scanned.unreadable_files);
        assert_eq!(
            listed.subagent_files_skipped, scanned.subagent_files_skipped,
            "and the same exclusion, so the denominator cannot drift"
        );
        assert_eq!(listed.sessions.len(), 3);
    }

    /// The listing indexes to exactly the same coverage as the scan
    /// (#1246).
    ///
    /// One level up from the test above: not just that the two walks
    /// agree, but that feeding either into `index_pass` produces the
    /// same searchable index and the same `Coverage`. This is the
    /// property the live pass actually rests on.
    #[test]
    fn indexing_from_the_listing_matches_indexing_from_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(root, "alpha", "s1", "fsevents stream died");
        write_session(root, "beta", "s2", "kubernetes rollout");

        let mut from_listing = db();
        let mut from_scan = db();
        index_pass(&mut from_listing, &crate::claude::corpus(root)).unwrap();
        index_pass(&mut from_scan, &crate::claude::scan(root)).unwrap();

        let a = coverage(&from_listing, vec![]).unwrap();
        let b = coverage(&from_scan, vec![]).unwrap();
        assert_eq!(a.indexed, b.indexed);
        assert_eq!(a.total, b.total);
        assert!(a.is_complete() && b.is_complete());

        // And the content is really there, not merely counted.
        for conn in [&from_listing, &from_scan] {
            assert!(matches!(
                search(conn, "fsevents", 20, vec![], Matching::Unmasked)
                    .unwrap()
                    .verdict,
                Verdict::Matches { .. }
            ));
        }
    }

    /// A transcript the LISTING could not read is still a gap (#1246).
    ///
    /// The honesty property's third clause survives the cheaper walk.
    /// `corpus()` stops before opening any transcript, so the worry is
    /// that a permission-walled file would now sail through as an
    /// ordinary session and let the index call itself complete over a
    /// hole. It does not: the listing takes `fs::metadata` on every
    /// session file, which is the same call whose failure `extract`
    /// reports, so the file lands in `unreadable_files` either way.
    #[test]
    #[cfg(unix)]
    fn the_listing_still_reports_an_unreadable_transcript() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(root, "slug", "good", "fsevents stream died");
        let bad = write_session(root, "slug", "bad", "never readable");
        // A directory nobody can traverse, so `fs::metadata` on the file
        // inside it fails. Revoking the FILE's own bits is not enough:
        // `stat(2)` does not need read permission on its target.
        let walled = root.join("walled");
        std::fs::create_dir_all(&walled).unwrap();
        std::fs::write(walled.join("x.jsonl"), "{}\n").unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();

        let listed = crate::claude::corpus(root);
        let scanned = crate::claude::scan(root);

        // Whatever the platform makes unreadable, both walks must agree
        // -- that is the invariant, not a particular count.
        assert_eq!(
            listed.unreadable_files.len(),
            scanned.unreadable_files.len(),
            "the listing must report exactly the gaps the scan does"
        );

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
        let sub_hit = search(&conn, "zzzsubagentonlyzzz", 20, vec![], Matching::Unmasked).unwrap();
        assert!(
            !matches!(sub_hit.verdict, Verdict::Matches { .. }),
            "a subagent transcript must not be searchable: it has no \
             session id and offers a resume handle for something that was \
             never a session"
        );

        let parent = search(&conn, "fsevents", 20, vec![], Matching::Unmasked).unwrap();
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

    /// #1519's tests, on the corpus search: a secret is not findable by
    /// any of its pieces from a phone's masked matching, is findable
    /// from the desktop's (and a revealing phone's) unmasked matching,
    /// and ordinary words are still found with the secret masked in the
    /// snippet. Each matching is confined to its own column, so the
    /// desktop does not hit the marker words in the masked copy.
    #[test]
    fn a_secret_is_searchable_only_by_unmasked_matching() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_session(
            root,
            "slug",
            "s1",
            "deploy the widget with sk-ant-api03-SECRETsecret0123456789 today",
        );
        write_session(root, "slug", "s2", "then check the gadget");
        let mut conn = db();
        index_pass(&mut conn, &crate::claude::scan(root)).unwrap();

        let ids = |q: &str, m: Matching| -> Vec<String> {
            match search(&conn, q, 20, vec![], m).unwrap().verdict {
                Verdict::Matches { hits } => hits.into_iter().map(|h| h.session_id).collect(),
                // A complete index: an empty answer is a plain "none".
                Verdict::None => Vec::new(),
                other => panic!("{q}: {other:?}"),
            }
        };
        for piece in ["SECRETsecret0123456789", "api03"] {
            assert_eq!(ids(piece, Matching::Unmasked), vec!["s1"], "{piece}");
            assert!(ids(piece, Matching::Masked).is_empty(), "{piece}");
        }

        let Verdict::Matches { hits } = search(&conn, "widget", 20, vec![], Matching::Masked)
            .unwrap()
            .verdict
        else {
            panic!("an ordinary word is found by masked matching");
        };
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("hidden"), "{}", hits[0].snippet);
        assert!(!hits[0].snippet.contains("SECRET"), "{}", hits[0].snippet);
        assert_eq!(ids("gadget", Matching::Masked), vec!["s2"]);

        // The marker's words are in the masked column only.
        assert_eq!(ids("hidden", Matching::Masked), vec!["s1"]);
        assert!(ids("hidden", Matching::Unmasked).is_empty());
    }

    /// A transcript that GREW is re-indexed, so a session stays
    /// findable by what was added to it after it was first indexed.
    ///
    /// The case the incremental design is most likely to get wrong: the
    /// ledger exists to SKIP work, and a skip keyed on the wrong thing
    /// would leave every long-running session searchable only by its
    /// opening minutes. That failure is invisible -- the session is in
    /// the index, the coverage reads complete, and the search simply
    /// does not find the thing the user remembers saying.
    #[test]
    fn a_transcript_that_grew_is_reindexed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let p = write_session(root, "slug", "s1", "the first thing said");

        let mut conn = db();
        index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert!(!matches!(
            search(&conn, "zzzsaidlaterzzz", 20, vec![], Matching::Unmasked)
                .unwrap()
                .verdict,
            Verdict::Matches { .. }
        ));

        // Appended, as a live session's transcript is.
        let mut body = std::fs::read_to_string(&p).unwrap();
        body.push_str(
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"zzzsaidlaterzzz\"}}\n",
        );
        std::fs::write(&p, body).unwrap();

        let second = index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert_eq!(second.indexed, 1, "the grown transcript was re-read");
        assert_eq!(second.unchanged, 0);

        let found = search(&conn, "zzzsaidlaterzzz", 20, vec![], Matching::Unmasked).unwrap();
        let Verdict::Matches { hits } = found.verdict else {
            panic!("a session must be findable by what was appended to it");
        };
        assert_eq!(hits[0].session_id, "s1");

        // And the OLD content is still findable: a re-index replaces the
        // row rather than leaving two, and rather than losing the head.
        assert!(matches!(
            search(
                &conn,
                "the first thing said",
                20,
                vec![],
                Matching::Unmasked
            )
            .unwrap()
            .verdict,
            Verdict::Matches { .. }
        ));
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
            search(&conn, "fsevents", 20, vec![], Matching::Unmasked)
                .unwrap()
                .verdict,
            Verdict::Matches { .. }
        ));

        std::fs::remove_file(&p).unwrap();
        index_pass(&mut conn, &crate::claude::scan(root)).unwrap();
        assert!(
            !matches!(
                search(&conn, "fsevents", 20, vec![], Matching::Unmasked)
                    .unwrap()
                    .verdict,
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
            search(&conn, "foo-bar", 20, vec![], Matching::Unmasked)
                .unwrap()
                .verdict,
            Verdict::Matches { .. }
        ));
        assert!(search(&conn, "C++", 20, vec![], Matching::Unmasked).is_ok());
        assert!(search(&conn, "NEAR", 20, vec![], Matching::Unmasked).is_ok());
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
            search(&conn, "zzzpastthebudgetzzz", 20, vec![], Matching::Unmasked)
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
        let _home = crate::auth::test_home::real_for_a_live_probe();
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

        // A steady-state pass: everything unchanged.
        let steady = index_pass(&mut conn, &scan).unwrap();
        println!(
            "steady-state index        {} ms ({} unchanged)",
            steady.elapsed_ms, steady.unchanged
        );

        // What the LIVE PASS actually pays every 60 seconds, which is
        // the corpus listing PLUS the index step -- not the index step
        // alone. Quoting only the 3 ms index would be picking the
        // flattering half of a number the reader is using to judge
        // whether this belongs on a 60-second timer.
        //
        // BOTH are measured, because #1246's whole claim is the
        // difference between them: the pass called `scan` as #1203
        // shipped it and calls `corpus` now, and a reader should be
        // able to reproduce the saving rather than take the module
        // docs' word for it.
        let t = std::time::Instant::now();
        let warm = crate::claude::corpus(&root);
        let warm_corpus_ms = t.elapsed().as_millis();
        let warm_index = index_pass(&mut conn, &warm).unwrap();
        println!(
            "live-pass added cost      {} ms ({} ms listing + {} ms index), warm  <- #1246",
            warm_corpus_ms as u64 + warm_index.elapsed_ms,
            warm_corpus_ms,
            warm_index.elapsed_ms
        );

        let t = std::time::Instant::now();
        let warm_scan = crate::claude::scan(&root);
        let warm_scan_ms = t.elapsed().as_millis();
        let warm_scan_index = index_pass(&mut conn, &warm_scan).unwrap();
        println!(
            "  was, via scan()         {} ms ({} ms scan + {} ms index), warm  <- #1203",
            warm_scan_ms as u64 + warm_scan_index.elapsed_ms,
            warm_scan_ms,
            warm_scan_index.elapsed_ms
        );

        // The saving is only real if the two agree about the corpus.
        // A cheaper listing that found fewer sessions would shrink the
        // denominator, and a shrunken denominator is exactly how an
        // incomplete index starts reporting itself complete.
        assert_eq!(
            warm.sessions.len(),
            warm_scan.sessions.len(),
            "the listing and the scan must find the same sessions, or the \
             cheaper one has changed the denominator"
        );

        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").ok();
        let db_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!(
            "database size             {:.2} GB",
            db_bytes as f64 / 1_073_741_824.0
        );

        // What #1519's masked copy costs: its share of the stored text,
        // and the masking itself, timed apart from the pass it rides in.
        let (body_bytes, masked_bytes): (i64, i64) = conn
            .query_row(
                "SELECT COALESCE(SUM(length(body)), 0), COALESCE(SUM(length(body_masked)), 0)
                   FROM claude_transcript_fts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let bodies: Vec<String> = conn
            .prepare("SELECT body FROM claude_transcript_fts")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let started = std::time::Instant::now();
        let hidden: usize = bodies
            .iter()
            .map(|b| crate::remote::privacy::mask_text(b).1)
            .sum();
        println!(
            "indexed text              {:.1} MB, masked copy {:.1} MB  <- #1519",
            body_bytes as f64 / 1_048_576.0,
            masked_bytes as f64 / 1_048_576.0
        );
        println!(
            "masking it all            {} ms, {hidden} spans  <- #1519",
            started.elapsed().as_millis()
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
        // The third is a CONTROL: a term that must match nothing, so the
        // no-hit path is timed too.
        //
        // RANDOM per run, and that is not belt-and-braces. The corpus is
        // self-referential -- the transcript of the session running this
        // test lands in `~/.claude/projects` and contains this file's
        // source -- so any fixed token is searchable on the NEXT run,
        // and the control then measures the opposite of what it claims.
        //
        // Observed twice, not feared: a readable phrase ("no such word
        // anywhere") returned 2 hits, and so did a nonsense literal once
        // the run that introduced it had been written to disk. Splitting
        // the literal across a `format!` did not help either, because
        // the assembled string is what gets indexed. A value that did
        // not exist when the corpus was written is the only thing that
        // can be absent from it.
        let control = format!(
            "zzctl{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        for q in ["fsevents", "migration", &control] {
            let t = std::time::Instant::now();
            let a = search(
                &conn,
                q,
                50,
                scan.unreadable_files.clone(),
                Matching::Unmasked,
            )
            .unwrap();
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

    /// An empty query does not produce a settled "no matches".
    ///
    /// It is not a search that found nothing; it is not a search. Over a
    /// COMPLETE index the honest-looking `Verdict::None` is exactly the
    /// wrong answer here -- it is a confident statement that the corpus
    /// does not contain something nobody asked about, and it would paint
    /// "No matches. All 1,494 sessions were searched." under an empty
    /// box.
    ///
    /// The frontend hook also declines to run an empty query, but this
    /// is asserted at the boundary that OWNS the claim rather than
    /// relying on a caller to never ask.
    #[test]
    fn an_empty_query_is_not_a_search_that_found_nothing() {
        let conn = db();
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

        completed_progress(&conn, 1);

        // The premise: this index IS complete, so nothing about coverage
        // is holding the settled answer back.
        assert!(coverage(&conn, vec![]).unwrap().is_complete());

        for empty in ["", "   ", "\t"] {
            let answer = search(&conn, empty, 20, vec![], Matching::Unmasked).unwrap();
            assert_eq!(
                answer.verdict,
                Verdict::NotAsked,
                "an empty query must not be answered as though it were a search"
            );
        }
    }

    /// `pending` does not wrap when the denominator is stale.
    #[test]
    fn a_stale_denominator_is_not_a_huge_backlog() {
        let cov = Coverage {
            population: None,
            indexed: 10,
            total: 4,
            ..Default::default()
        };
        assert_eq!(cov.pending(), 0);
    }
}
