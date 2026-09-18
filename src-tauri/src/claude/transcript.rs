//! Reading the Claude Code transcripts already on disk (#914, epic #910).
//!
//! Claude Code writes one JSONL transcript per session under
//! `~/.claude/projects/<project-slug>/<session-id>.jsonl`. Those files are
//! the ONLY source of history for every session that started before
//! Headstate existed -- measured 1,430 of them on the development machine
//! -- so this module is what makes the Claude Code view open with real
//! content instead of an empty list.
//!
//! Read-only, and deliberately so. Nothing here writes, renames or
//! deletes inside `~/.claude`: those transcripts are Claude Code's data
//! and the file `claude --resume <id>` depends on. Losing one destroys
//! exactly the capability #910 exists to provide.
//!
//! # Three measured facts that shape every line below
//!
//! Each was run against the real corpus on the development machine; each
//! is a bug in the obvious implementation.
//!
//! ## 1. Half the `.jsonl` files are not sessions
//!
//! ```text
//! find ~/.claude/projects -name '*.jsonl'                       -> 2800
//! find ~/.claude/projects -name '*.jsonl' -path '*/subagents/*' -> 1370
//! <slug>/<uuid>.jsonl  (one level down, the real sessions)       -> 1430
//! ```
//!
//! (#914 quotes 2,797 / 1,368 / 1,433 from an earlier run on the same
//! machine. The corpus grows as sessions are used; the RATIO is the
//! finding and it is unchanged -- just under half the files are not
//! sessions.)
//!
//! Subagent transcripts live at `<slug>/<session-id>/subagents/agent-*.jsonl`
//! and deeper. They have no session id of their own and `claude --resume`
//! cannot open them. The obvious `**/*.jsonl` glob therefore roughly
//! DOUBLES the list with rows offering a resume handle for something that
//! was never a session.
//!
//! [`session_files`] reads exactly one level below each project slug, so
//! nothing under a nested directory can be reached at all --
//! `subagent_transcripts_are_not_sessions` proves it against the real
//! layout rather than trusting the depth arithmetic.
//!
//! ## 2. The metadata is never in the first record
//!
//! `cwd`, `gitBranch` and `version` sit behind bookkeeping records --
//! `queue-operation`, `last-prompt`, `mode`, `permission-mode`,
//! `atis-latch`. Measured first-appearance of `cwd`, over all 1,430
//! sessions:
//!
//! ```text
//! record 1:     0      <- NOT ONE FILE
//! record 3:  1354
//! record 4:    27
//! record 5:    21
//! record 6:    28      <- deepest observed
//! ```
//!
//! Zero. An importer that reads record 1 returns nothing for EVERY
//! session while appearing to work perfectly: the file opens, the JSON
//! parses, the loop completes, no error is raised anywhere. It ships as a
//! view full of blank rows and looks like a UI bug.
//!
//! So [`extract`] scans forward to the first record BEARING `cwd`, and
//! `metadata_is_not_in_the_first_record` is the regression test, written
//! so that a reader restricted to record 1 fails it.
//!
//! ## 3. The 40-record bound has to cover the title too
//!
//! The design's bound of 40 records was set for `cwd`, whose deepest
//! observed position is 6. But the human-readable name lives in an
//! `ai-title` record, which is written only once Claude has produced one
//! -- measured median position 10, p95 18, **deepest 33**, present for
//! 1,434 of 1,436 sessions.
//!
//! 40 therefore has 7 records of headroom over the worst real case and
//! covers both fields in one pass. It is a bound rather than a whole-file
//! read because the corpus is 881 MB: reading every file to the end would
//! turn a sub-second scan into a disk-bound one for two fields that are
//! always near the top.
//!
//! The two titleless sessions get [`None`], not their UUID dressed up as
//! a name -- naming is the caller's decision and a fabricated one here
//! would be indistinguishable from a real one.
//!
//! # Why a full rescan, and no incremental machinery
//!
//! Measured by `tests::real_corpus` against the real tree, release build:
//!
//! ```text
//! sessions found            1436
//! subagent .jsonl skipped   1370
//! elapsed              192-252 ms warm, ~1040 ms cold
//! metadata beyond record 1  1436
//! deepest cwd record           6
//! with an ai-title name     1434
//! with a last-activity time 1436   <- every one, after the tail retry
//! ```
//!
//! (The session count climbs as the machine is used -- it was 1,430 a few
//! hours earlier, and #914 quotes 1,433. Only the ratios and the
//! per-session facts are stable, which is why `real_corpus` asserts those
//! and prints the counts rather than asserting them.)
//!
//! Both numbers are stated because only one of them is the honest answer
//! to "what will the user feel". 192-252 ms is the warm-page-cache figure
//! over three consecutive runs; the FIRST scan after boot reads 881 MB of
//! cold file tails and takes about a second. The startup rescan is
//! therefore a ~1s background cost once per boot, not 200 ms, and quoting
//! only the warm figure would be picking the flattering measurement.
//!
//! Either way it is fast because each file costs a bounded head read plus
//! one 16 KB tail seek (rarely a second, wider one -- see
//! [`TAIL_BYTES_RETRY`]) and -- see the pre-filter in [`extract`] -- most
//! records are never handed to the JSON parser at all. At that price a
//! complete rescan at startup and behind a button is simpler AND more
//! correct than any cache: there is no stored offset to invalidate, no
//! watcher to fail silently, and a transcript edited behind our back is
//! picked up on the next pass. `notify` is not a dependency of this crate
//! and this module is the argument for not making it one.
//!
//! One correction to the design, which claimed 0.23s flat: the first cut
//! of this module measured **738 ms warm**, three times that, because the
//! first 40 records of the corpus are 260 MB -- an `assistant` record
//! carries whole message bodies -- and parsing all of them to read three
//! short strings dominates everything else. The pre-filter is what makes
//! the design's number real rather than aspirational, and `real_corpus`
//! prints the figure so the claim stays checkable on another machine
//! instead of resting on this comment.
//!
//! # Absent is not zero
//!
//! A [`Scan`] carries what it could not read as data, not as a log line.
//! An unreadable project directory, a file whose metadata could not be
//! extracted, a `~/.claude/projects` that is missing entirely -- each is
//! a distinct, counted, reportable outcome, because rendering any of them
//! as "no sessions" tells the user their history is gone when in fact we
//! failed to look. That is the defect class `caches/mod.rs:550` states
//! the house rule for and #841 shipped the fail-open version of.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How far into a transcript to look for `cwd` and the title.
///
/// See the module docs: `cwd` is deepest at record 6 and `ai-title` at
/// record 33 across all 1,430 real sessions, so 40 clears the worst
/// observed case for both with headroom, in one pass, without reading
/// into an 881 MB corpus.
const HEAD_RECORDS: usize = 40;

/// How much of the tail to read for the last-activity timestamp, on the
/// first attempt.
///
/// Transcript records run from a few hundred bytes to a few KB, so 16 KB
/// reaches back several records -- enough that the trailing ones carrying
/// no `timestamp` (`atis-latch`, `ai-title`, `last-prompt`) do not hide
/// the ones that do. It dates all but 3 of the 1,436 real sessions.
///
/// The other 3 are why [`TAIL_BYTES_RETRY`] exists; see it for the
/// measurement.
const TAIL_BYTES: u64 = 16 * 1024;

/// The second, wider tail read, for a transcript whose last timestamped
/// record is enormous.
///
/// ONE record can be far larger than the whole first window. Measured on
/// the real corpus, the 3 sessions that a 16 KB tail cannot date:
///
/// ```text
/// record  -1  atis-latch     83 B     no timestamp
/// record  -2  ai-title      122 B     no timestamp
/// record  -3  last-prompt   343 B     no timestamp
/// record  -4  attachment  92897 B     HAS the timestamp, 93 KB from the end
/// ```
///
/// A 92 KB `attachment` pushes the newest usable timestamp past any
/// window a first read would sensibly use. The design attributed these 3
/// to having no timestamp at all; they have one, and reading 16 KB is
/// simply not enough to see it.
///
/// That distinction matters rather than being a curiosity: an undated
/// session sorts last and reads as ancient, so the cost of giving up too
/// early is three real sessions looking dead in the list. 256 KB clears
/// the observed worst case by better than 2x and is paid only by the
/// ~0.2% of files that need it -- the common case still does one 16 KB
/// read.
///
/// Bounded, not unbounded, because the honest answer for a transcript
/// whose tail is bigger than this is still [`None`]: the corpus is 881 MB
/// and a whole-file read to date one row is the wrong trade.
const TAIL_BYTES_RETRY: u64 = 256 * 1024;

/// Whether one JSON line looks like a Claude transcript record at all.
///
/// Only used to avoid parsing lines that cannot possibly carry what we
/// want; the parse still decides.
fn looks_interesting(line: &str) -> bool {
    // `"type":"user"` rather than `"user"` alone (#1133): the bare word
    // appears in `"role":"user"` and `"userType"` on records this gate
    // is meant to skip, and widening it to those would parse most of the
    // head for nothing.
    line.contains("\"cwd\"") || line.contains("\"aiTitle\"") || line.contains("\"type\":\"user\"")
}

/// The text of a `user` record, if it carries one.
///
/// Handles both content shapes. Every first-user record in the 60 real
/// transcripts sampled carries a plain string, but `preview.rs` already
/// models the block-array form and a shape this does not understand must
/// yield `None` rather than a fragment of JSON.
fn user_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    // An array of blocks: join the `text` ones, ignore tool results.
    let parts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// How much of the opening prompt to keep.
///
/// Enough for a second line under a title, and bounded because a pasted
/// stack trace is a legitimate first prompt: the session list carries
/// one of these per row across 1,438 rows, and the transport split
/// `ClaudeCodePage` documents exists to keep that payload small.
const PROMPT_CHARS: usize = 300;

/// Clamp on a CHARACTER boundary, never a byte one.
///
/// `&s[..300]` panics mid-codepoint, and a prompt containing an emoji or
/// any non-ASCII text is ordinary rather than exotic.
fn clamp_prompt(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(PROMPT_CHARS) {
        Some((i, _)) => format!("{}…", &trimmed[..i]),
        None => trimmed.to_string(),
    }
}

/// The fields a transcript can tell us about a session.
///
/// Every one is optional because every one is genuinely absent from some
/// real transcript, and a default would be a lie the UI cannot
/// distinguish from a reading.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    /// The `claude --resume` handle. Taken from the FILENAME, not the
    /// record body -- see [`extract`].
    pub session_id: String,
    pub path: String,
    /// Working directory the session ran in, from the first record
    /// bearing one.
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub claude_version: Option<String>,
    /// Claude's own generated name for the session, from an `ai-title`
    /// record. `None` for the sessions that never got one -- never the
    /// UUID in disguise.
    pub name: Option<String>,
    /// The first thing the user asked, clamped (#1133).
    ///
    /// The ASK, which a generated title cannot carry: 286 of 1,438 real
    /// sessions share their `aiTitle` with another, so a list showing
    /// only titles cannot tell two sessions apart at the moment someone
    /// is choosing which to resume.
    ///
    /// Measured before relying on the head window: across 40 real
    /// transcripts the first `user` record sits at depth 3, 8 or 11 --
    /// well inside [`HEAD_RECORDS`], so this costs no extra read.
    ///
    /// `None` renders as NOTHING. Never the title repeated, never the
    /// UUID: a fabricated stand-in cannot be told from a real prompt,
    /// which is the rule this module already states about `name`.
    pub opening_prompt: Option<String>,
    /// RFC 3339, the earliest timestamp seen in the head.
    pub first_seen_at: Option<String>,
    /// RFC 3339, the newest timestamp in the tail. The time the session
    /// was last ACTIVE, not the time we scanned it.
    pub last_activity_at: Option<String>,
    /// Which record (1-based) first carried `cwd`.
    ///
    /// Kept because it is the evidence for finding 2 in the module docs,
    /// and because a future Claude Code release moving the metadata past
    /// [`HEAD_RECORDS`] would show up here as a rising number long before
    /// it showed up as empty rows.
    pub cwd_record: Option<usize>,
}

/// What a scan of the transcript tree found, INCLUDING what it could not
/// read.
///
/// The unreadable counts are the point of the type. A caller that gets
/// `sessions: []` must be able to tell "you have no Claude sessions" from
/// "we could not read your transcripts", because those two have opposite
/// remedies and the first one is alarming if it is false.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    pub sessions: Vec<Transcript>,
    /// `.jsonl` files under a `subagents/` directory or otherwise nested
    /// below `<slug>/<file>`. Not failures -- correctly excluded work,
    /// counted so the exclusion is visible and testable rather than
    /// invisible.
    pub subagent_files_skipped: usize,
    /// Bytes across the session transcripts (#1135).
    ///
    /// The corpus this app reads most was the one footprint it never
    /// reported, while worktrees, artifacts, venvs, Docker and packages
    /// all had one. Measured on the real corpus at 916 MB across 1,502
    /// session files, with one file at 76.7 MB.
    ///
    /// Taken from the metadata the walk already holds, so it costs no
    /// extra I/O.
    pub session_bytes: u64,
    /// Bytes across the subagent transcripts, kept apart.
    ///
    /// Roughly half the `.jsonl` files on disk are subagent transcripts
    /// -- 1,370 of 2,800 measured -- and the two mean different things:
    /// one is sessions you can resume, the other is work they delegated.
    pub subagent_bytes: u64,
    /// Files whose size could not be read, making both totals FLOORS.
    ///
    /// A size we could not take is not a size of zero.
    pub unsized_files: usize,
    /// Project directories that could not be listed, with why.
    ///
    /// A permission error here hides an unknown number of sessions, so it
    /// travels as a message, not a boolean.
    pub unreadable_dirs: Vec<String>,
    /// Transcripts found but not readable, with why.
    pub unreadable_files: Vec<String>,
    /// How many sessions carried `cwd` somewhere other than record 1.
    ///
    /// On the real corpus this equals `sessions.len()` -- see finding 2.
    /// A build in which this drops to zero has either lost the forward
    /// scan or found an entirely different transcript format, and both
    /// are worth noticing.
    pub metadata_beyond_first_record: usize,
    /// Milliseconds the scan took, so the "no incremental machinery"
    /// decision stays checkable on someone else's machine instead of
    /// resting on this module's docs.
    pub elapsed_ms: u64,
    /// Which session spawned each agent worktree (#1002).
    ///
    /// Built during this same walk, because it needs an unbounded read of
    /// every transcript and this is the only pass that already opens them
    /// all. See [`super::subagent`] for the measurement that rules out
    /// doing it in the session list's poll.
    pub subagents: super::subagent::Map,
    /// The transcript root, when it does not exist at all (#970).
    ///
    /// # Why this is not `unreadable_dirs`
    ///
    /// It was, and that put an amber "0 sessions read, but 1 could not be
    /// -- this list is incomplete by an unknown amount" above the empty
    /// list on every machine that has never run Claude Code. Nothing
    /// could not be read: there is nothing there. That is the app's first
    /// statement to a new user about this feature, and it was false.
    ///
    /// `ENOENT` on the root and `EACCES` on the root are opposite facts
    /// with opposite remedies, and one `Err` arm collapsed them. This is
    /// the `NotFound` half, carried in a field [`Scan::is_partial`] does
    /// not consult -- `live.rs`'s `read_registry` already draws exactly
    /// this line for `~/.claude/sessions` and
    /// `a_missing_registry_is_a_settled_empty_answer` is its test, with
    /// the comment "absent is the answer, not an error". The two halves of
    /// `~/.claude` now agree.
    ///
    /// **A permission error on the root is still `unreadable_dirs`**, and
    /// still loud: that user's history genuinely IS hidden and the remedy
    /// is theirs to apply. Collapsing both into "nothing here" would be
    /// #846 in the opposite direction.
    ///
    /// The PATH is kept rather than a bare boolean, because
    /// `scan_default`'s reason for reporting this at all was so the UI can
    /// say which path it looked at -- a new user learning that Headstate
    /// looked in `~/.claude/projects` learns where sessions come from.
    pub absent_root: Option<String>,
}

impl Scan {
    /// Whether anything at all could not be read.
    ///
    /// The UI's cue for "this list may be incomplete". Deliberately NOT a
    /// reason to discard the sessions that WERE read: a partial answer
    /// labelled partial beats both a silent truncation and an error page.
    ///
    /// [`Scan::absent_root`] is deliberately NOT consulted (#970): a root
    /// that does not exist makes the list EMPTY, not short. A machine with
    /// no Claude Code history has a complete list of nothing, and telling
    /// its owner the list is "incomplete by an unknown amount" accuses the
    /// app of a failure that did not happen.
    pub fn is_partial(&self) -> bool {
        !self.unreadable_dirs.is_empty() || !self.unreadable_files.is_empty()
    }
}

/// `~/.claude/projects`, or `None` when there is no home directory.
pub fn projects_dir() -> Option<PathBuf> {
    crate::auth::home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Every session transcript under `root`, and the nested files skipped.
///
/// A session transcript is `<root>/<project-slug>/<name>.jsonl` and
/// nothing else. The walk descends exactly one level, so a file under
/// `<slug>/<session-id>/subagents/` is unreachable BY CONSTRUCTION rather
/// than by a path filter that a later refactor could drop -- which is
/// what finding 1 in the module docs is about.
///
/// Returns [`Walk`]. An unreadable project directory is reported, never
/// silently treated as empty: it may hold any number of sessions.
fn session_files(root: &Path) -> Walk {
    let mut out = Walk::default();

    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        // The root does not exist: the honest answer for a machine that
        // has never run Claude Code, and NOT a failure to read (#970). It
        // travels in its own field so `is_partial()` stays false and the
        // page says "no sessions" rather than "incomplete by an unknown
        // amount". `read_registry` draws the same line for the other half
        // of `~/.claude`.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out.absent_root = Some(root.display().to_string());
            return out;
        }
        Err(e) => {
            // Any OTHER error on the whole tree -- a permission wall
            // above all -- hides an unknown number of sessions and stays
            // loud. Reported to the caller as a failure to READ, which is
            // a different statement from "you have no sessions" -- the
            // distinction this module exists to keep.
            out.unreadable.push(format!("{}: {e}", root.display()));
            return out;
        }
    };

    for slug in entries.flatten() {
        let dir = slug.path();
        // `file_type` rather than `metadata` so a symlinked project
        // directory is not followed into an arbitrary tree.
        if !slug.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let inner = match std::fs::read_dir(&dir) {
            Ok(i) => i,
            Err(e) => {
                out.unreadable.push(format!("{}: {e}", dir.display()));
                continue;
            }
        };
        for f in inner.flatten() {
            let p = f.path();
            let Ok(ft) = f.file_type() else { continue };
            if ft.is_dir() {
                // A per-session directory: `<slug>/<session-id>/` holding
                // `subagents/`. Counting what is underneath is what makes
                // the exclusion a number the tests can assert on, so it
                // cannot regress into an invisible behaviour.
                let (found, bytes) = count_jsonl(&p);
                out.nested += found;
                out.subagent_bytes += bytes;
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                // The size, on the metadata this walk already has in
                // hand (#1135). A file whose size cannot be read
                // contributes nothing rather than failing the walk: the
                // total becomes a floor, which `Scan::bytes_partial`
                // below carries.
                match f.metadata() {
                    Ok(m) => out.session_bytes += m.len(),
                    Err(_) => out.unsized_files += 1,
                }
                out.files.push(p);
            }
        }
    }
    out
}

/// What one walk of the transcript tree found.
///
/// A named struct since #970, because the walk now reports FOUR things and
/// two of them are absences that must not be confused: `unreadable` is
/// "we could not read this", `absent_root` is "there is nothing to read".
/// A fourth positional element in a tuple is exactly how the two would get
/// swapped at a call site.
#[derive(Default)]
struct Walk {
    files: Vec<PathBuf>,
    /// `.jsonl` files below `<slug>/<file>`, correctly excluded.
    nested: usize,
    /// Bytes across the session transcripts in `files` (#1135).
    session_bytes: u64,
    /// Bytes across the SUBAGENT transcripts counted in `nested`.
    ///
    /// Kept apart from `session_bytes` because roughly half the `.jsonl`
    /// files on disk are subagent transcripts -- 1,370 of 2,800 measured
    /// -- and the two mean different things: one is sessions you can
    /// resume, the other is work they delegated.
    subagent_bytes: u64,
    /// Directories that could not be LISTED, with why.
    unreadable: Vec<String>,
    /// The root itself, when it does not exist. See [`Scan::absent_root`].
    absent_root: Option<String>,
    /// Files whose size could not be read (#1135).
    ///
    /// Non-zero makes every byte total a FLOOR. A size we could not take
    /// is not a size of zero, which is this module's own rule one field
    /// over.
    unsized_files: usize,
}

/// Every `.jsonl` at or below `dir`, for the skipped-file count.
///
/// Failures are not reported: this counts files we are deliberately NOT
/// importing, so being unable to count one costs a slightly low number in
/// a diagnostic, not a missing session.
fn count_jsonl(dir: &Path) -> (usize, u64) {
    let mut n = 0;
    let mut bytes = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() && p.extension().and_then(|x| x.to_str()) == Some("jsonl") => {
                    n += 1;
                    // A size that cannot be read contributes nothing and
                    // the file is still counted: this is a diagnostic
                    // total, and a low byte figure beside a correct file
                    // count is the honest shape.
                    if let Ok(m) = e.metadata() {
                        bytes += m.len();
                    }
                }
                _ => {}
            }
        }
    }
    (n, bytes)
}

/// Pull a string field out of a record, treating empty as absent.
///
/// Claude Code writes `"gitBranch": ""` for a detached HEAD or a
/// non-repository directory. Storing that empty string would render as a
/// branch named nothing; `None` renders as no branch, which is the truth.
fn field(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Everything one transcript can tell us, from a bounded head read and a
/// tail seek.
///
/// # The session id comes from the FILENAME
///
/// Measured across the corpus: the filename stem equals the `sessionId`
/// in the body for every one of 1,430 files, and no id appears in two
/// project directories. The stem is used anyway, because it is the name
/// `claude --resume` takes and it is knowable without parsing anything.
/// A transcript too corrupt to parse still yields a resumable id.
///
/// # Errors
///
/// Only when the file cannot be OPENED. A file that opens but whose
/// records are unparseable yields a [`Transcript`] with `None` fields --
/// which is honest ("we read it and learned nothing") and distinct from
/// the unreadable case ("we could not read it"), and keeps the resume
/// handle available either way.
pub fn extract(path: &Path) -> Result<Transcript, String> {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("{}: unreadable file name", path.display()))?
        .to_string();

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("{}: could not open it: {e}", path.display()))?;
    let size = file
        .metadata()
        .map_err(|e| format!("{}: could not read its size: {e}", path.display()))?
        .len();

    let mut out = Transcript {
        session_id,
        path: path.display().to_string(),
        ..Default::default()
    };

    // --- head: the first HEAD_RECORDS records, for metadata and title.
    //
    // One BufReader pass, stopping at the bound. The two fields are
    // gathered independently because they live at different depths
    // (record 3-6 for `cwd`, up to 33 for the title) and the first
    // `cwd` must not stop a scan that has not yet found a name.
    {
        let mut reader = BufReader::new(&mut file);
        let mut line = String::new();
        for i in 0..HEAD_RECORDS {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                // A single undecodable line ends the head scan rather
                // than the whole extraction: whatever was already found
                // is still true, and the tail seek is independent of it.
                Err(_) => break,
            }
            // `opening_prompt` joins the set (#1133): without it, a
            // session whose cwd, title and timestamp all land before the
            // first user record would exit the loop having never seen
            // one.
            if out.cwd.is_some()
                && out.name.is_some()
                && out.first_seen_at.is_some()
                && out.opening_prompt.is_some()
            {
                break;
            }
            // Skip the PARSE, not the record, when this line cannot carry
            // anything still missing.
            //
            // This is the difference between a fast scan and a slow one,
            // and it is a measurement: the first 40 records of the real
            // 1,430-session corpus total 260 MB -- averaging 182 KB per
            // head, with the largest single head at 941 KB -- because an
            // `assistant` or `attachment` record carries whole message
            // bodies. Handing all of that to `serde_json` builds a full
            // `Value` tree per record to read at most three short strings
            // out of it, and measured 738 ms for the corpus.
            //
            // A substring test over the raw line first drops that to
            // 192-252 ms (`real_corpus`, release, warm) for
            // byte-identical output -- same session count, same titles,
            // same timestamps -- because a record with no
            // `"cwd"`, no `"aiTitle"` and no needed `"timestamp"` is
            // never parsed at all.
            //
            // It is a pre-filter and not the decision: a line that passes
            // is still parsed, and the parse still decides. A false
            // positive costs one wasted parse; a false negative is
            // impossible, since the keys we look for are exactly the
            // substrings tested.
            if !looks_interesting(&line)
                && !(out.first_seen_at.is_none() && line.contains("\"timestamp\""))
            {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else {
                // An unparseable line mid-file is not a failure of the
                // file. Claude Code appends concurrently and a truncated
                // line is a real possibility.
                continue;
            };
            if out.first_seen_at.is_none() {
                out.first_seen_at = field(&rec, "timestamp");
            }
            if out.cwd.is_none() {
                if let Some(cwd) = field(&rec, "cwd") {
                    out.cwd = Some(cwd);
                    out.git_branch = field(&rec, "gitBranch");
                    out.claude_version = field(&rec, "version");
                    // 1-based, so it reads the way the measurements in
                    // the module docs are written.
                    out.cwd_record = Some(i + 1);
                }
            }
            if out.name.is_none() {
                out.name = field(&rec, "aiTitle");
            }
            // The FIRST user record only (#1133). `is_none()` is what
            // makes it the opening ask rather than whichever prompt
            // happened to land last inside the window.
            if out.opening_prompt.is_none()
                && rec.get("type").and_then(|t| t.as_str()) == Some("user")
            {
                out.opening_prompt = user_text(&rec)
                    .map(|t| clamp_prompt(&t))
                    .filter(|t| !t.is_empty());
            }
        }
    }

    // --- tail: the newest timestamp, from a seek rather than a whole
    // read, because the corpus is 881 MB.
    //
    // Two attempts, because one record can be bigger than the first
    // window -- see [`TAIL_BYTES_RETRY`]. The retry is skipped entirely
    // when the file is already no larger than the first window, since
    // re-reading the same bytes cannot produce a different answer.
    out.last_activity_at = newest_timestamp(&mut file, size, TAIL_BYTES);
    if out.last_activity_at.is_none() && size > TAIL_BYTES {
        out.last_activity_at = newest_timestamp(&mut file, size, TAIL_BYTES_RETRY);
    }

    Ok(out)
}

/// The newest `timestamp` within the last `window` bytes of `file`.
///
/// Returns [`None`] when no COMPLETE record in that window carries one --
/// which is a statement about the window, not about the file, and is why
/// [`extract`] retries with a wider one before believing it.
fn newest_timestamp(file: &mut std::fs::File, size: u64, window: u64) -> Option<String> {
    let start = size.saturating_sub(window);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;

    let text = String::from_utf8_lossy(&buf);
    let mut lines: &str = &text;
    if start > 0 {
        // The seek almost certainly landed mid-record. Drop everything up
        // to the first newline: a half record is unparseable anyway, and
        // keeping it would mean reasoning about partial JSON.
        lines = match lines.find('\n') {
            Some(nl) => &lines[nl + 1..],
            None => "",
        };
    }

    // Last timestamped record wins. Trailing records without one
    // (`atis-latch`, `ai-title`, `last-prompt`) are skipped rather than
    // making the session look undated.
    for line in lines.lines().rev() {
        // Same pre-filter as the head scan, for the same reason: a record
        // with no timestamp is not worth a full parse.
        if !line.contains("\"timestamp\"") {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(ts) = field(&rec, "timestamp") {
            return Some(ts);
        }
    }
    None
}

/// Scan every session transcript under `root`.
///
/// The full-rescan entry point. Reports what it could not read rather
/// than presenting a short list as a complete one -- see the module docs.
pub fn scan(root: &Path) -> Scan {
    let t0 = std::time::Instant::now();
    let walk = session_files(root);

    let mut out = Scan {
        subagent_files_skipped: walk.nested,
        session_bytes: walk.session_bytes,
        subagent_bytes: walk.subagent_bytes,
        unsized_files: walk.unsized_files,
        unreadable_dirs: walk.unreadable,
        absent_root: walk.absent_root,
        ..Default::default()
    };

    for f in &walk.files {
        match extract(f) {
            Ok(t) => {
                if t.cwd_record.is_some_and(|r| r > 1) {
                    out.metadata_beyond_first_record += 1;
                }
                out.sessions.push(t);
            }
            Err(e) => out.unreadable_files.push(e),
        }
    }

    // The parent map (#1002), built HERE and nowhere else.
    //
    // This is the one place in the app that already reads every
    // transcript, and the map needs an UNBOUNDED read of each -- measured,
    // #959's 8 MB budget finds only 15 of 52 agent ids on the largest real
    // parent, because a spawn happens once at whatever moment the parent
    // delegated rather than on every assistant record. `subagent.rs`
    // carries the offset table.
    //
    // Measured cost on the real corpus, release build: 969 ms over 1,523
    // transcripts / 0.86 GB. That is affordable once, at startup and
    // behind the rescan button, and would be ruinous in the session
    // list's 10-second poll -- which is exactly why the poll reads the
    // stored answer instead of deriving one.
    out.subagents = super::subagent::build(&walk.files);

    // Newest first: the session someone wants is almost always the one
    // they were just in. A session with no timestamp sorts last rather
    // than first, because an unknown time must not outrank a known recent
    // one.
    out.sessions.sort_by(|a, b| {
        b.last_activity_at
            .as_deref()
            .cmp(&a.last_activity_at.as_deref())
    });

    out.elapsed_ms = t0.elapsed().as_millis() as u64;
    out
}

/// Scan the real `~/.claude/projects`.
///
/// # Errors
///
/// Only when there is no home directory to look in. A MISSING
/// `~/.claude/projects` is not an error here -- it is the honest answer
/// for a machine that has never run Claude Code, and it arrives as an
/// empty [`Scan`] with the directory named in [`Scan::absent_root`] so
/// the UI can still say which path it looked at.
///
/// It used to arrive in `unreadable_dirs`, which `is_partial()` reads, so
/// the first thing a new user was told about this feature was "0 sessions
/// read, but 1 could not be -- this list is incomplete by an unknown
/// amount" (#970). The path is still named; the channel is one that does
/// not claim a failure.
pub fn scan_default() -> Result<Scan, String> {
    let root = projects_dir().ok_or("could not find your home directory")?;
    Ok(scan(&root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A throwaway directory, removed on drop.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "headstate-transcript-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, lines: &[&str]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    /// The real head of a real transcript, abridged to the record types
    /// that matter. Copied from the shape measured on disk: bookkeeping
    /// first, metadata at record 5, title at 6.
    fn realistic() -> Vec<&'static str> {
        vec![
            r#"{"type":"last-prompt","sessionId":"s1","timestamp":"2026-09-01T10:00:00Z"}"#,
            r#"{"type":"mode","mode":"default","sessionId":"s1"}"#,
            r#"{"type":"permission-mode","sessionId":"s1"}"#,
            r#"{"type":"atis-latch","sessionId":"s1"}"#,
            r#"{"type":"attachment","cwd":"/Users/acme/code/widget","gitBranch":"feat/x","version":"2.1.270","sessionId":"s1","timestamp":"2026-09-01T10:00:05Z"}"#,
            r#"{"type":"ai-title","aiTitle":"Fix the retry backoff","sessionId":"s1"}"#,
            r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-09-01T11:30:00Z"}"#,
        ]
    }

    /// Finding 2, the bug that ships looking like it works.
    ///
    /// The metadata is at record 5, behind four bookkeeping records. This
    /// test is written so that a reader restricted to record 1 CANNOT
    /// pass it -- see `a_first_record_only_reader_finds_nothing` directly
    /// below, which runs that implementation against this same fixture
    /// and gets `None` for every field.
    #[test]
    fn metadata_is_not_in_the_first_record() {
        let t = Tmp::new("record5");
        let f = t.path().join("-Users-acme-code-widget").join("s1.jsonl");
        write(&f, &realistic());

        let got = extract(&f).unwrap();
        assert_eq!(got.cwd.as_deref(), Some("/Users/acme/code/widget"));
        assert_eq!(got.git_branch.as_deref(), Some("feat/x"));
        assert_eq!(got.claude_version.as_deref(), Some("2.1.270"));
        assert_eq!(got.cwd_record, Some(5), "cwd is at record 5, not record 1");
        assert_eq!(got.name.as_deref(), Some("Fix the retry backoff"));
        assert_eq!(got.session_id, "s1");
    }

    /// The sabotage half of the test above: what the naive importer does.
    ///
    /// Reading only the first record is the obvious implementation and it
    /// returns nothing for every field WITHOUT erroring -- the file
    /// opens, the JSON parses, the function returns Ok. That silence is
    /// why the bound-scan exists, and asserting it here means the pair of
    /// tests documents the failure instead of only the fix.
    #[test]
    fn a_first_record_only_reader_finds_nothing() {
        let t = Tmp::new("naive");
        let f = t.path().join("-Users-acme-code-widget").join("s1.jsonl");
        write(&f, &realistic());

        // The naive implementation, verbatim: parse record 1, take cwd.
        let first = std::fs::read_to_string(&f).unwrap();
        let rec: serde_json::Value =
            serde_json::from_str(first.lines().next().unwrap()).expect("record 1 parses fine");
        assert_eq!(
            rec.get("cwd"),
            None,
            "record 1 parses and simply has no cwd -- the silent-empty bug"
        );
        assert_eq!(rec.get("aiTitle"), None);
        assert_eq!(rec.get("gitBranch"), None);
        // And it looks like a valid record, which is what makes it deadly.
        assert_eq!(
            rec.get("type").and_then(|v| v.as_str()),
            Some("last-prompt")
        );
    }

    /// Finding 1: `subagents/` files are not sessions.
    ///
    /// The tree mirrors the real layout -- two sessions beside a
    /// per-session directory holding three subagent transcripts, one of
    /// them nested a second level as the real corpus has. A `**/*.jsonl`
    /// glob would return 5; this must return 2 and account for the other
    /// 3.
    #[test]
    fn subagent_transcripts_are_not_sessions() {
        let t = Tmp::new("subagents");
        let slug = t.path().join("-Users-acme-code-widget");
        write(&slug.join("s1.jsonl"), &realistic());
        write(&slug.join("s2.jsonl"), &realistic());
        write(
            &slug.join("s1").join("subagents").join("agent-aaa.jsonl"),
            &realistic(),
        );
        write(
            &slug.join("s1").join("subagents").join("agent-bbb.jsonl"),
            &realistic(),
        );
        write(
            &slug
                .join("s1")
                .join("subagents")
                .join("nested")
                .join("agent-ccc.jsonl"),
            &realistic(),
        );

        let got = scan(t.path());
        assert_eq!(
            got.sessions.len(),
            2,
            "only the top-level files are sessions"
        );
        assert_eq!(got.subagent_files_skipped, 3);
        let mut ids: Vec<_> = got.sessions.iter().map(|s| s.session_id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["s1", "s2"]);
        assert!(
            !got.sessions
                .iter()
                .any(|s| s.session_id.starts_with("agent-")),
            "a subagent id must never be offered as a resume handle"
        );
    }

    /// Absent is not zero: a directory we cannot read is REPORTED.
    ///
    /// Chmod 000 on a project directory hides an unknown number of
    /// sessions. The scan must say so rather than return a short list
    /// that looks complete.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_project_directory_is_reported_not_zero() {
        use std::os::unix::fs::PermissionsExt;
        let t = Tmp::new("noperm");
        let open = t.path().join("-Users-acme-readable");
        write(&open.join("s1.jsonl"), &realistic());
        let shut = t.path().join("-Users-acme-secret");
        write(&shut.join("s9.jsonl"), &realistic());
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();

        let got = scan(t.path());

        // Restore before any assertion can panic and leak an
        // undeletable directory into the temp dir.
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            got.is_partial(),
            "a partial scan must know that it is partial"
        );
        assert_eq!(got.unreadable_dirs.len(), 1);
        assert!(
            got.unreadable_dirs[0].contains("secret"),
            "the report must name the directory: {:?}",
            got.unreadable_dirs
        );
        // The readable half still comes back. A partial answer labelled
        // partial beats discarding what we did read.
        assert_eq!(got.sessions.len(), 1);
    }

    /// A missing root is a SETTLED empty answer, and names the path (#970).
    ///
    /// This test previously asserted `is_partial()` and one
    /// `unreadable_dirs` entry, which is what put "0 sessions read, but 1
    /// could not be -- this list is incomplete by an unknown amount" above
    /// the empty list on every machine that has never run Claude Code.
    /// Nothing could not be read there; there is nothing to read.
    ///
    /// `live.rs`'s `a_missing_registry_is_a_settled_empty_answer` is the
    /// same assertion about the other half of `~/.claude`, and the two
    /// halves now agree -- which is the inconsistency #970 is about.
    ///
    /// The path is still named, because `scan_default`'s reason for
    /// reporting this at all is that the UI can say where it looked.
    #[test]
    fn a_missing_root_is_a_settled_empty_answer_that_still_names_the_path() {
        let t = Tmp::new("missing");
        let root = t.path().join("does-not-exist");
        let got = scan(&root);
        assert!(got.sessions.is_empty());
        assert!(
            !got.is_partial(),
            "a machine with no history has a COMPLETE list of nothing, not a short one"
        );
        assert!(
            got.unreadable_dirs.is_empty(),
            "absent is the answer, not an error: {:?}",
            got.unreadable_dirs
        );
        assert!(
            got.absent_root
                .as_deref()
                .is_some_and(|p| p.contains("does-not-exist")),
            "the path must still be named: {:?}",
            got.absent_root
        );
    }

    /// A root that EXISTS and holds nothing says nothing at all (#970).
    ///
    /// The pair to the test above, and the one that keeps `absent_root`
    /// from becoming a second way of saying "empty". A user who ran
    /// `claude` once and then cleared their history has a real
    /// `~/.claude/projects`; the page has no path to explain to them and
    /// must not claim the directory is missing.
    #[test]
    fn an_empty_but_present_root_reports_no_absent_root() {
        let t = Tmp::new("present-empty");
        std::fs::create_dir_all(t.path()).unwrap();
        let got = scan(t.path());
        assert!(got.sessions.is_empty());
        assert!(!got.is_partial());
        assert_eq!(
            got.absent_root, None,
            "the directory is there -- it is its contents that are empty"
        );
    }

    /// A root we cannot READ is still loud (#970).
    ///
    /// The half that must not regress. `ENOENT` and `EACCES` on the root
    /// produce the same empty list and have opposite remedies: one is "you
    /// have no history", the other is "your history is behind a permission
    /// wall and we cannot see it". Only the second means the list is short
    /// by an unknown amount, so only the second may set `is_partial()`.
    /// Collapsing both into "nothing here" is #846 in the opposite
    /// direction, and #846 is the bug that cost a shipped release.
    ///
    /// `#[cfg(unix)]` because the gate is a Unix mode bit: on Windows a
    /// directory at mode `0o000` is still listable, so the `read_dir`
    /// would succeed and the assertion would fail for a reason that is
    /// about the platform rather than about this code. Four Windows-only
    /// failures in this epic were all a test encoding one platform's
    /// behaviour as universal.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_root_is_still_partial_and_not_reported_as_absent() {
        use std::os::unix::fs::PermissionsExt;
        let t = Tmp::new("root-denied");
        let root = t.path().join("walled");
        std::fs::create_dir_all(root.join("slug")).unwrap();
        let original = std::fs::metadata(&root).unwrap().permissions();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();

        let got = scan(&root);

        // Restored BEFORE any assertion can panic, so a failure here does
        // not leave an undeletable directory behind for `Tmp`'s drop.
        std::fs::set_permissions(&root, original).unwrap();

        // Running as root defeats the mode bit entirely, and a test that
        // silently passes for the wrong reason is worse than one that
        // skips. The listing succeeding is the signal.
        if got.absent_root.is_none() && !got.is_partial() {
            eprintln!("skipped: mode 0o000 did not block the listing (running as root?)");
            return;
        }
        assert!(
            got.is_partial(),
            "a permission wall genuinely hides sessions: {got:?}"
        );
        assert_eq!(
            got.absent_root, None,
            "the directory exists -- we just cannot see into it"
        );
        assert_eq!(got.unreadable_dirs.len(), 1);
        assert!(got.unreadable_dirs[0].contains("walled"));
    }

    /// Last activity comes from the END of the file, not the head.
    ///
    /// A session's row should show when it was last used. Taking the head
    /// timestamp would date every long session to its first minute.
    #[test]
    fn last_activity_is_the_newest_timestamp_in_the_tail() {
        let t = Tmp::new("tail");
        let f = t.path().join("slug").join("s1.jsonl");
        let mut lines = realistic();
        lines.push(r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-09-05T18:00:00Z"}"#);
        // A trailing record with no timestamp must not hide the one above.
        lines.push(r#"{"type":"ai-title","aiTitle":"renamed later","sessionId":"s1"}"#);
        write(&f, &lines);

        let got = extract(&f).unwrap();
        assert_eq!(
            got.last_activity_at.as_deref(),
            Some("2026-09-05T18:00:00Z")
        );
        assert_eq!(got.first_seen_at.as_deref(), Some("2026-09-01T10:00:00Z"));
    }

    /// The tail seek must work on a file larger than the seek window.
    ///
    /// This is the case the 881 MB corpus is made of, and the case where
    /// a mid-record seek landing point has to be handled: the partial
    /// first line inside the window is dropped rather than parsed.
    #[test]
    fn the_tail_seek_handles_a_file_bigger_than_the_window() {
        let t = Tmp::new("bigtail");
        let f = t.path().join("slug").join("s1.jsonl");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        let mut fh = std::fs::File::create(&f).unwrap();
        for l in realistic() {
            writeln!(fh, "{l}").unwrap();
        }
        // Well past TAIL_BYTES of filler, each record carrying a
        // timestamp so the LAST one is the answer.
        let filler = "x".repeat(900);
        for i in 0..60 {
            writeln!(
                fh,
                r#"{{"type":"assistant","sessionId":"s1","timestamp":"2026-09-0{}T0{}:00:00Z","pad":"{filler}"}}"#,
                1 + i % 5,
                i % 10
            )
            .unwrap();
        }
        writeln!(
            fh,
            r#"{{"type":"assistant","sessionId":"s1","timestamp":"2026-12-31T23:59:59Z"}}"#
        )
        .unwrap();
        drop(fh);

        assert!(std::fs::metadata(&f).unwrap().len() > TAIL_BYTES);
        let got = extract(&f).unwrap();
        assert_eq!(
            got.last_activity_at.as_deref(),
            Some("2026-12-31T23:59:59Z")
        );
        // And the head fields still came from the head, unaffected.
        assert_eq!(got.cwd.as_deref(), Some("/Users/acme/code/widget"));
    }

    /// A 92 KB record does not make a dated session look undated.
    ///
    /// The real case, reproduced from the 3 sessions of 1,430 that a 16 KB
    /// tail could not date. Their shape, measured:
    ///
    /// ```text
    /// record -1  atis-latch     83 B   no timestamp
    /// record -2  ai-title      122 B   no timestamp
    /// record -3  last-prompt   343 B   no timestamp
    /// record -4  attachment  92897 B   HAS the timestamp
    /// ```
    ///
    /// The design called these three "no timestamp anywhere". They have
    /// one; 16 KB simply cannot see past a 92 KB record. Giving up would
    /// sort three live sessions to the bottom of the list as undated,
    /// which is the absent-is-not-zero failure in miniature -- an unread
    /// value rendered as a fact about the session.
    #[test]
    fn a_huge_record_does_not_hide_the_last_activity() {
        let t = Tmp::new("hugerecord");
        let f = t.path().join("slug").join("s1.jsonl");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        let mut fh = std::fs::File::create(&f).unwrap();
        for l in realistic() {
            writeln!(fh, "{l}").unwrap();
        }
        // The timestamped giant, larger than TAIL_BYTES all by itself.
        let pad = "z".repeat(92_000);
        writeln!(
            fh,
            r#"{{"type":"attachment","sessionId":"s1","timestamp":"2026-09-12T17:30:22Z","pad":"{pad}"}}"#
        )
        .unwrap();
        // The three untimestamped trailers that follow it in the real files.
        writeln!(fh, r#"{{"type":"last-prompt","sessionId":"s1"}}"#).unwrap();
        writeln!(
            fh,
            r#"{{"type":"ai-title","aiTitle":"Late title","sessionId":"s1"}}"#
        )
        .unwrap();
        writeln!(fh, r#"{{"type":"atis-latch","sessionId":"s1"}}"#).unwrap();
        drop(fh);

        let got = extract(&f).unwrap();
        assert_eq!(
            got.last_activity_at.as_deref(),
            Some("2026-09-12T17:30:22Z"),
            "the retry window must reach past a 92 KB record"
        );
        // And the giant really was beyond the first window, so this test
        // exercises the retry rather than passing by accident.
        assert!(std::fs::metadata(&f).unwrap().len() > TAIL_BYTES + 16 * 1024);
    }

    /// Past the RETRY window, undated is the honest answer.
    ///
    /// The retry is bounded on purpose: the corpus is 881 MB and reading a
    /// whole file to date one row is the wrong trade. So this asserts the
    /// giving-up point exists and is not silently unbounded.
    #[test]
    fn past_the_retry_window_undated_is_honest() {
        let t = Tmp::new("pastretry");
        let f = t.path().join("slug").join("s1.jsonl");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        let mut fh = std::fs::File::create(&f).unwrap();
        writeln!(
            fh,
            r#"{{"type":"user","cwd":"/tmp/x","sessionId":"s1","timestamp":"2026-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        // Bigger than TAIL_BYTES_RETRY, and carrying no timestamp itself.
        let pad = "z".repeat(TAIL_BYTES_RETRY as usize + 4096);
        writeln!(
            fh,
            r#"{{"type":"attachment","sessionId":"s1","pad":"{pad}"}}"#
        )
        .unwrap();
        drop(fh);

        let got = extract(&f).unwrap();
        assert_eq!(
            got.last_activity_at, None,
            "beyond the bound, undated is honest -- not a fabricated time"
        );
        // The head still found what it could: the fields are independent.
        assert_eq!(got.cwd.as_deref(), Some("/tmp/x"));
        assert_eq!(got.first_seen_at.as_deref(), Some("2026-01-01T00:00:00Z"));
    }

    /// A file that opens but says nothing is not an error.
    ///
    /// The resume handle is in the filename, so an unparseable transcript
    /// still yields a resumable session with honest `None` fields --
    /// distinct from the unreadable case, which is an error.
    #[test]
    fn an_unparseable_transcript_still_yields_its_resume_handle() {
        let t = Tmp::new("garbage");
        let f = t.path().join("slug").join("11112222-3333.jsonl");
        write(&f, &["not json at all", "{ broken", ""]);

        let got = extract(&f).unwrap();
        assert_eq!(got.session_id, "11112222-3333");
        assert_eq!(got.cwd, None);
        assert_eq!(got.name, None);
        assert_eq!(got.cwd_record, None);

        let s = scan(t.path());
        assert!(
            s.unreadable_files.is_empty(),
            "readable-but-empty is not unreadable"
        );
        assert_eq!(s.sessions.len(), 1);
        assert!(!s.is_partial());
    }

    /// An empty `gitBranch` is absent, not a branch named "".
    #[test]
    fn an_empty_string_field_reads_as_absent() {
        let t = Tmp::new("emptybranch");
        let f = t.path().join("slug").join("s1.jsonl");
        write(
            &f,
            &[
                r#"{"type":"mode","sessionId":"s1"}"#,
                r#"{"type":"user","cwd":"/tmp/x","gitBranch":"","version":"2.1.270","sessionId":"s1","timestamp":"2026-09-01T10:00:00Z"}"#,
            ],
        );
        let got = extract(&f).unwrap();
        assert_eq!(got.cwd.as_deref(), Some("/tmp/x"));
        assert_eq!(got.git_branch, None, "a detached HEAD has no branch name");
    }

    /// The bound is a bound: metadata past it is not found.
    ///
    /// Asserted rather than left implicit so the number in
    /// [`HEAD_RECORDS`] is a decision with a test behind it. If a future
    /// Claude Code release pushes `cwd` past record 40, this is the test
    /// that has to be updated -- and `Scan::metadata_beyond_first_record`
    /// is the field that would show it happening in production.
    #[test]
    fn metadata_past_the_bound_is_not_found() {
        let t = Tmp::new("bound");
        let f = t.path().join("slug").join("s1.jsonl");
        let mut lines: Vec<String> = (0..HEAD_RECORDS)
            .map(|i| format!(r#"{{"type":"mode","n":{i},"sessionId":"s1"}}"#))
            .collect();
        lines.push(r#"{"type":"user","cwd":"/tmp/too-deep","sessionId":"s1"}"#.into());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        write(&f, &refs);

        let got = extract(&f).unwrap();
        assert_eq!(got.cwd, None, "beyond the bound is honestly unknown");
        // The deepest cwd measured on 1,430 real sessions is record 6 and
        // the deepest ai-title is 33, so 40 has real headroom. A const
        // block so lowering the bound below the worst observed case is a
        // COMPILE error rather than a test that has to be run.
        const { assert!(HEAD_RECORDS >= 34) };
    }

    /// Newest first, and an undated session does not jump the queue.
    #[test]
    fn sessions_sort_newest_first_with_undated_last() {
        let t = Tmp::new("sort");
        let slug = t.path().join("slug");
        write(
            &slug.join("old.jsonl"),
            &[r#"{"cwd":"/a","timestamp":"2026-01-01T00:00:00Z"}"#],
        );
        write(
            &slug.join("new.jsonl"),
            &[r#"{"cwd":"/b","timestamp":"2026-08-01T00:00:00Z"}"#],
        );
        write(&slug.join("undated.jsonl"), &[r#"{"cwd":"/c"}"#]);

        let got = scan(t.path());
        let ids: Vec<_> = got.sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "old", "undated"]);
    }

    /// A probe against the REAL `~/.claude/projects`, for #914's
    /// verification requirement.
    ///
    /// `#[ignore]`d because it depends on the developer's own machine: CI
    /// has no transcript corpus, and an assertion about 1,430 sessions
    /// would fail there for the right reason and the wrong outcome. Run
    /// with `cargo test --lib real_corpus -- --ignored --nocapture`.
    ///
    /// Committed rather than run once and discarded because the
    /// measurements in this module's docs are load-bearing -- the
    /// full-rescan-with-no-cache decision rests on them -- and a
    /// measurement nobody can reproduce is just an assertion.
    #[test]
    #[ignore = "needs the developer's own ~/.claude/projects"]
    fn real_corpus() {
        let Some(root) = projects_dir() else {
            eprintln!("no home directory; nothing to measure");
            return;
        };
        if !root.is_dir() {
            eprintln!("{} does not exist; nothing to measure", root.display());
            return;
        }
        let got = scan(&root);
        let titled = got.sessions.iter().filter(|s| s.name.is_some()).count();
        let deepest = got.sessions.iter().filter_map(|s| s.cwd_record).max();
        let dated = got
            .sessions
            .iter()
            .filter(|s| s.last_activity_at.is_some())
            .count();
        println!("root                      {}", root.display());
        println!("sessions found            {}", got.sessions.len());
        println!("subagent .jsonl skipped   {}", got.subagent_files_skipped);
        println!("elapsed                   {} ms", got.elapsed_ms);
        println!(
            "metadata beyond record 1  {}",
            got.metadata_beyond_first_record
        );
        println!("deepest cwd record        {deepest:?}");
        println!("with an ai-title name     {titled}");
        println!("with a last-activity time {dated}");
        println!("unreadable dirs           {:?}", got.unreadable_dirs);
        println!("unreadable files          {}", got.unreadable_files.len());

        // The exclusion, asserted rather than eyeballed: subagent files
        // must be a real, non-trivial population that did NOT become
        // sessions.
        assert!(
            got.subagent_files_skipped > 0,
            "this corpus has no subagent files, so it cannot prove the exclusion"
        );
        assert!(
            !got.sessions
                .iter()
                .any(|s| s.session_id.starts_with("agent-")),
            "a subagent transcript reached the session list"
        );
        // Finding 2, on the real corpus: NOT ONE session carries `cwd` at
        // record 1, so a record-1 reader returns nothing for all of them.
        assert_eq!(
            got.metadata_beyond_first_record,
            got.sessions.len(),
            "every real session carries cwd past record 1"
        );
    }

    /// A non-`.jsonl` file in a project directory is not a session.
    #[test]
    fn only_jsonl_files_are_sessions() {
        let t = Tmp::new("ext");
        let slug = t.path().join("slug");
        write(&slug.join("s1.jsonl"), &realistic());
        write(&slug.join("notes.md"), &["# not a transcript"]);
        write(&slug.join("s2.json"), &[r#"{"cwd":"/x"}"#]);
        let got = scan(t.path());
        assert_eq!(got.sessions.len(), 1);
        assert_eq!(got.sessions[0].session_id, "s1");
    }
    /// #1133: the opening ask, which a generated title cannot carry --
    /// 286 of 1,438 real sessions share their `aiTitle` with another.
    #[test]
    fn the_first_user_prompt_is_captured() {
        let tmp = Tmp::new("prompt");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"system","cwd":"/code/w","timestamp":"2026-09-01T10:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":"fix the flaky test"}}}}"#
        )
        .unwrap();
        drop(f);

        let t = extract(&path).unwrap();
        assert_eq!(t.opening_prompt.as_deref(), Some("fix the flaky test"));
    }

    /// The FIRST one. A session's later prompts are not the ask that
    /// started it.
    #[test]
    fn only_the_first_prompt_is_kept() {
        let tmp = Tmp::new("firstprompt");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        for text in ["the opening ask", "a later follow-up"] {
            writeln!(
                f,
                r#"{{"type":"user","message":{{"role":"user","content":"{text}"}}}}"#
            )
            .unwrap();
        }
        drop(f);

        assert_eq!(
            extract(&path).unwrap().opening_prompt.as_deref(),
            Some("the opening ask")
        );
    }

    /// Clamped on a CHARACTER boundary. `&s[..300]` panics mid-codepoint,
    /// and a prompt containing an emoji is ordinary rather than exotic.
    #[test]
    fn a_long_multibyte_prompt_clamps_without_panicking() {
        let tmp = Tmp::new("clamp");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        let long = "é".repeat(500);
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":"{long}"}}}}"#
        )
        .unwrap();
        drop(f);

        let got = extract(&path).unwrap().opening_prompt.unwrap();
        assert!(got.ends_with('…'), "a clamped prompt says it was clamped");
        assert_eq!(got.chars().count(), 301, "300 characters plus the ellipsis");
    }

    /// A session with no user record has NO opening prompt. `None`
    /// renders as nothing -- never the title repeated, never the UUID,
    /// because a fabricated stand-in cannot be told from a real prompt.
    #[test]
    fn a_session_with_no_user_record_has_no_prompt() {
        let tmp = Tmp::new("noprompt");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"system","cwd":"/code/w"}}"#).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"content":"hello"}}}}"#
        )
        .unwrap();
        drop(f);

        assert_eq!(extract(&path).unwrap().opening_prompt, None);
    }

    /// The block-array content shape. Every first-user record in the 60
    /// real transcripts sampled carries a plain string, but `preview.rs`
    /// already models this form and a shape we do not understand must
    /// yield `None` rather than a fragment of JSON.
    #[test]
    fn a_block_array_prompt_is_read() {
        let tmp = Tmp::new("blocks");
        let path = tmp.0.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"from a block"}}]}}}}"#
        )
        .unwrap();
        drop(f);

        assert_eq!(
            extract(&path).unwrap().opening_prompt.as_deref(),
            Some("from a block")
        );
    }

    /// #1135: the corpus this app reads most was the one footprint it
    /// never reported, while worktrees, artifacts, venvs, Docker and
    /// packages all had one.
    #[test]
    fn session_and_subagent_bytes_are_counted_apart() {
        let tmp = Tmp::new("bytes");
        let root = tmp.0.join("projects");
        let slug = root.join("slug");
        std::fs::create_dir_all(slug.join("s1").join("subagents")).unwrap();

        // A session transcript, and a subagent one under it.
        std::fs::write(slug.join("s1.jsonl"), "x".repeat(100)).unwrap();
        std::fs::write(
            slug.join("s1").join("subagents").join("a1.jsonl"),
            "y".repeat(40),
        )
        .unwrap();

        let got = scan(&root);
        assert_eq!(got.session_bytes, 100, "the session transcript");
        assert_eq!(
            got.subagent_bytes, 40,
            "and the subagent one, kept apart -- the two mean different things"
        );
        assert_eq!(got.subagent_files_skipped, 1);
        assert_eq!(got.unsized_files, 0, "everything was measurable");
    }

    /// An empty corpus reports zero bytes and zero unsized -- a real
    /// answer, distinguishable from a corpus we could not measure.
    #[test]
    fn an_empty_corpus_reports_zero_rather_than_nothing() {
        let tmp = Tmp::new("emptybytes");
        let root = tmp.0.join("projects");
        std::fs::create_dir_all(root.join("slug")).unwrap();

        let got = scan(&root);
        assert_eq!(got.session_bytes, 0);
        assert_eq!(got.unsized_files, 0);
    }
}
