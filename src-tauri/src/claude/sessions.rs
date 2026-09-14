//! What the Claude Code view renders: one row per session (#917), the
//! resume command that actually works (#918), and whether each of the
//! two paths a session records is still on disk (#919).
//!
//! Reads `claude_session` and `claude_run` (migration 11), pairs each row
//! with a liveness derived from the machine ([`super::liveness`]), and
//! answers the one question the resume action turns on: does the
//! directory this session ran in still exist?
//!
//! # Two paths, two survival rates, two fields
//!
//! A session records a `cwd` AND a `transcript_path`, and they are not
//! interchangeable. Measured over 1,461 real session transcripts for
//! #919: **83.0% of cwds are gone (1,213) and 0% of transcripts are.**
//! So the cwd's state travels as [`ListRow::cwd_state`] and the
//! transcript's as [`SessionDetail::transcript_state`], as independent
//! tri-states, and neither is derived from the other. See
//! [`check_transcript`].
//!
//! # Why the cwd check is a tri-state and not a bool
//!
//! `claude --resume <id>` works from ANY directory -- verified -- and
//! adopts the **invoking** directory rather than the one the session
//! recorded. Measured on the development machine: **1,213 of 1,438
//! sessions (84.4%) have a cwd that no longer exists.**
//!
//! So a bare `claude --resume <id>` on the clipboard resurrects the
//! session pointed at whatever tree the user happens to be standing in
//! -- worse than failing, because it looks like it worked. The session
//! arrives with all its context and intentions and starts editing the
//! wrong repository.
//!
//! Three cases, and the third is not padding:
//!
//! | recorded cwd | what [`ResumeCommand`] offers |
//! |---|---|
//! | exists | `cd <cwd> && claude --resume <id>` |
//! | gone | the bare command, and says the directory is gone |
//! | could not check | the bare command, and says we could not check |
//!
//! A permission error reading the path is a different answer from "the
//! directory is gone": one means the tree may well be there and the `cd`
//! would have worked, the other means it certainly is not. Collapsing
//! them is the absent-is-not-zero mistake, and the remedies differ --
//! one is "fix the permission", the other is "expect to land somewhere
//! else".
//!
//! # `session_id` is the handle, and it never goes stale
//!
//! Verified: the id survives `--resume` and `--continue` unchanged, only
//! a fresh start mints a new one, resuming does not consume it, and it
//! still resolves after the recorded cwd has been deleted -- the
//! transcript is keyed by the id, not by the path. So the handle in a
//! copied command is good indefinitely, which is what makes copying
//! text the right action rather than a link to something that expires.
//!
//! # No terminal is spawned, deliberately
//!
//! `commands::claudify_command` already settled this for the app:
//! copying text works identically everywhere and lands the user in their
//! OWN shell, while macOS has no default-terminal concept at all. This
//! module returns strings for the clipboard for the same reason.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::liveness::{derive, Liveness, ProcessProbe, Registry, Run, SysinfoProbe};

/// Whether the directory a session ran in is still there.
///
/// Three states because `std::fs` gives three answers, and the middle
/// one matters: `NotFound` is "gone", any other error is "could not
/// look". See the module docs for why merging them produces a command
/// that lands in the wrong tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CwdState {
    /// The path exists and is of the kind the caller needs -- a
    /// directory for a cwd, a regular file for a transcript.
    Exists,
    /// The path is definitely not there. 83% of the real corpus for a
    /// cwd; 0% for a transcript (#919 measured both -- see
    /// [`check_transcript`]).
    Gone,
    /// The check itself failed -- a permission error, a stalled network
    /// mount -- so the directory may well be there.
    Unknown { why: String },
    /// The session has no recorded cwd at all. Distinct from `Gone`: we
    /// never knew where it ran, so there is no path to report as
    /// missing.
    NotRecorded,
}

/// Check whether a recorded cwd is still a directory.
///
/// `NotFound` is the ONLY error that becomes [`CwdState::Gone`]. A
/// permission error, a symlink loop or an unresponsive mount becomes
/// [`CwdState::Unknown`] carrying the reason, because each of those is
/// consistent with the directory existing.
///
/// `symlink_metadata` is deliberately NOT used: a symlink pointing at a
/// live tree is a directory the `cd` would succeed into, and reporting
/// it as gone because the link itself is a link would be wrong.
pub fn check_cwd(cwd: Option<&str>) -> CwdState {
    // A path that exists but is a FILE is not somewhere `cd` can go, so
    // it is reported as gone-for-this-purpose rather than as `Exists`,
    // which would produce a `cd` that fails in the user's shell after
    // they pasted it.
    check_path(cwd, Want::Directory)
}

/// Whether a path is expected to be a directory or a regular file.
///
/// The two checks differ in exactly one respect and share every error
/// rule, so they share an implementation. Splitting them into two
/// hand-written `match` blocks is how the `NotFound`-is-the-only-`Gone`
/// discipline drifts on one side and not the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    Directory,
    File,
}

/// Check a recorded path, with the three-state honesty [`CwdState`]
/// carries.
///
/// `NotFound` is the ONLY error that becomes [`CwdState::Gone`]; every
/// other error becomes [`CwdState::Unknown`] with the reason, because
/// each of those is consistent with the path existing.
fn check_path(path: Option<&str>, want: Want) -> CwdState {
    let Some(path) = path else {
        return CwdState::NotRecorded;
    };
    if path.is_empty() {
        return CwdState::NotRecorded;
    }
    match std::fs::metadata(path) {
        Ok(m) => {
            let right_kind = match want {
                Want::Directory => m.is_dir(),
                // `is_file` and not `!is_dir`: a socket or a fifo at the
                // transcript's path is not a file the reveal can show,
                // and calling it `Exists` would produce a button that
                // opens a Finder window on nothing.
                Want::File => m.is_file(),
            };
            if right_kind {
                CwdState::Exists
            } else {
                CwdState::Gone
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CwdState::Gone,
        Err(e) => CwdState::Unknown {
            why: format!("{e}"),
        },
    }
}

/// Check whether a session's transcript file is still on disk (#919).
///
/// # Why this is a SEPARATE check from the cwd, measured separately
///
/// The transcript is a different path from the cwd with a radically
/// different survival rate, and assuming they behave alike is what makes
/// a reveal button dead. Measured on the development machine over the
/// real corpus of 1,461 session transcripts:
///
/// ```text
/// cwd         exists=248   gone=1213  (83.0% gone)
/// transcript  exists=1461  gone=0     ( 0.0% gone)
/// ```
///
/// The asymmetry is not a coincidence: the cwds are overwhelmingly agent
/// worktrees that were deleted when the work landed, while the
/// transcripts are the files `claude --resume` itself reads, so Claude
/// Code keeps them. That is why the two states are carried as two
/// fields and never derived from one another -- a UI that gated the
/// transcript reveal on `cwd_state` would hide the button that works on
/// 83% of rows.
///
/// A transcript expects a FILE where the cwd expects a directory. Every
/// other rule -- `NotFound` alone means gone, any other error means we
/// could not look -- is shared, which is why both go through
/// [`check_path`].
pub fn check_transcript(transcript_path: Option<&str>) -> CwdState {
    check_path(transcript_path, Want::File)
}

/// The resume command to put on the clipboard, and what to say about it.
///
/// `command` is always non-empty and always correct to run -- the
/// question the three cases answer is only whether it can include the
/// `cd`, and what the user needs to be told before they paste it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCommand {
    /// The text to copy.
    pub command: String,
    /// What the user must know before pasting, or `None` when the
    /// command needs no caveat.
    ///
    /// `Some` on BOTH the gone and the could-not-check cases, with
    /// different wording: "this will resume wherever you run it" is a
    /// statement of fact, and "we could not check" is an admission. A
    /// single shared string would collapse the distinction the type
    /// above exists to keep.
    pub caveat: Option<String>,
    /// Whether the command carries its own `cd`. The UI uses it to
    /// decide whether Resume is the primary action or a secondary one;
    /// kept as data rather than re-derived from the string.
    pub anchored: bool,
}

/// Build the resume command for one session.
///
/// Shell-quoted with single quotes so a path containing a space, a `$`
/// or a `;` cannot change what the pasted line does. An embedded single
/// quote is escaped the POSIX way (`'\''`), which is the only escape a
/// single-quoted string admits.
///
/// # The ID is quoted too, and it has to be
///
/// This quoted only the path until a review pointed out that the id was
/// interpolated bare -- while the comment above promised the whole line
/// was safe. And the id is NOT a validated UUID: `transcript::extract`
/// takes it verbatim from `path.file_stem()` of any `*.jsonl` one level
/// under `~/.claude/projects/<slug>/`, with no format check anywhere.
///
/// So a file named `` `id`.jsonl `` or `$(whoami).jsonl` produced a
/// copied command carrying live shell syntax. The blast radius was small
/// -- it needs write access to `~/.claude/projects` and a paste -- but
/// the standard this function sets for itself is "does the pasted line do
/// what the button said it would", and for the id half it did not.
///
/// Quoting rather than validating, because quoting is total: it is
/// correct for every id Claude Code might legitimately adopt later,
/// whereas a UUID check would reject a future format and turn a working
/// resume into a refusal.
pub fn resume_command(session_id: &str, cwd: Option<&str>, state: &CwdState) -> ResumeCommand {
    let bare = format!("claude --resume {}", shell_quote(session_id));
    match state {
        CwdState::Exists => ResumeCommand {
            // The `cd` is the whole point of #918. Without it the
            // session resumes in whatever directory the terminal
            // happens to be in.
            command: format!("cd {} && {bare}", shell_quote(cwd.unwrap_or_default())),
            caveat: None,
            anchored: true,
        },
        CwdState::Gone => ResumeCommand {
            command: bare,
            caveat: Some(format!(
                "The directory this session ran in is gone ({}), so this will resume in \
                 whatever directory you run it from.",
                cwd.unwrap_or("no path recorded")
            )),
            anchored: false,
        },
        CwdState::Unknown { why } => ResumeCommand {
            command: bare,
            caveat: Some(format!(
                "Could not check whether {} still exists ({why}), so this omits the `cd` \
                 and will resume in whatever directory you run it from.",
                cwd.unwrap_or("the recorded directory")
            )),
            anchored: false,
        },
        CwdState::NotRecorded => ResumeCommand {
            command: bare,
            caveat: Some(
                "No working directory was recorded for this session, so this will resume in \
                 whatever directory you run it from."
                    .into(),
            ),
            anchored: false,
        },
    }
}

/// Quote a path for a POSIX shell.
///
/// Single quotes, because inside them every character except `'` is
/// literal -- so a path containing `$(rm -rf ~)` is a path and not a
/// substitution. The user is pasting this into their own shell, so the
/// bar is not "is the path trusted" but "does the pasted line do what
/// the button said it would".
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// A liveness as it travels in the LIST, with its reason interned.
///
/// The same three states as [`Liveness`] and the same information; the
/// only difference is that `why` is an index into
/// [`SessionList::reasons`] rather than the sentence itself. See
/// [`SessionList`] for why.
///
/// # Why an index rather than dropping `why`
///
/// The list RENDERS the reason -- `LivenessBadge` puts it in the row's
/// `title`, so "Not running" always carries its grounds on hover. #985
/// measured it as 18.9% of the payload and it would have been the
/// easiest thing to drop; dropping it would leave a verdict with no
/// grounds, which is the defect class epic #941 exists to remove. An
/// index preserves the sentence byte-for-byte and still pays for it
/// once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ListLiveness {
    Running { pid: u32, status: Option<String> },
    Dead { why: usize },
    Unknown { why: usize },
}

/// Interns liveness reasons while a list is assembled.
///
/// Small and linear on purpose: the measured corpus has ONE distinct
/// reason across 1,474 rows, and the ceiling is the handful of sentences
/// `liveness::derive` can produce plus one per running pid. A `HashMap`
/// here would be more machinery than the cardinality justifies.
#[derive(Default)]
struct Reasons {
    seen: std::collections::HashMap<String, usize>,
    list: Vec<String>,
}

impl Reasons {
    fn intern(&mut self, why: String) -> usize {
        if let Some(&ix) = self.seen.get(&why) {
            return ix;
        }
        let ix = self.list.len();
        self.seen.insert(why.clone(), ix);
        self.list.push(why);
        ix
    }

    /// Translate a derived [`Liveness`] into its list form.
    fn intern_liveness(&mut self, liveness: Liveness) -> ListLiveness {
        match liveness {
            Liveness::Running { pid, status } => ListLiveness::Running { pid, status },
            Liveness::Dead { why } => ListLiveness::Dead {
                why: self.intern(why),
            },
            Liveness::Unknown { why } => ListLiveness::Unknown {
                why: self.intern(why),
            },
        }
    }
}

/// One row of the session list, carrying only what the LIST needs.
///
/// # What is here, and the rule that decides
///
/// Exactly the fields the list renders, searches, filters or counts on,
/// and nothing else. Concretely: the four search fields (`name`, `cwd`,
/// `git_branch`, `session_id`), the two the chips switch on (`liveness`,
/// `cwd_state`), and the one the row prints (`last_activity_at`).
///
/// Everything else a session knows travels in [`SessionDetail`], fetched
/// for the ONE row the user selected. See [`SessionList`] for the
/// measurement that split them and for why this is not pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRow {
    pub session_id: String,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    /// The newest record in the transcript, NOT the time we scanned.
    /// `None` when nothing in the transcript carried a timestamp.
    pub last_activity_at: Option<String>,
    /// Derived every read, never stored -- for EVERY row, not just the
    /// window a cap would draw. See [`SessionList`]: keeping this true of
    /// the whole corpus is what ruled out every row-dropping approach.
    pub liveness: ListLiveness,
    pub cwd_state: CwdState,
}

/// The session list, INCLUDING what could not be read.
///
/// `registry_failure` and `registry_unreadable` are the point of the
/// type. A list rendered from a registry we could not read is a list in
/// which every liveness is `Unknown`, and the view has to say so rather
/// than show 1,438 rows that look like settled answers.
///
/// # Every row, every poll -- and a third of the bytes (#985)
///
/// Measured on the real corpus of 1,474 sessions, serialising what this
/// command actually returned:
///
/// ```text
/// resume            25.6%   253.8 B/row   <- detail only
/// liveness          18.9%   187.0 B/row   <- the same 150-char sentence, 1,474 times
/// transcript_path   15.5%   153.3 B/row   <- detail only
/// cwd                6.4%    63.3 B/row
/// session_id         5.3%    52.0 B/row
/// name               4.8%    47.1 B/row
/// last_activity_at   4.6%    46.0 B/row
/// first_seen_at      4.3%    43.0 B/row   <- detail only
/// git_branch         4.0%    39.3 B/row
/// transcript_state   3.8%    38.0 B/row   <- detail only
/// cwd_state          3.0%    29.2 B/row
/// claude_version     2.7%    27.0 B/row   <- detail only
/// runs               0.9%     9.0 B/row   <- detail only
///                          ------------
///                          990.0 B/row    1.392 MB per poll, every 10s
/// ```
///
/// The finding that settled the design: **the fields the list renders,
/// searches, filters and counts on are the CHEAP ones.** Everything
/// expensive is read by the detail pane, for the one row the user
/// selected. So the split is by FIELD, not by row -- which is why it
/// costs the feature nothing:
///
/// | invariant | why it survives |
/// |---|---|
/// | search covers the whole corpus | all four search fields are still on every row |
/// | the stated total is the true total | every row still arrives; `sessions.len()` IS the total |
/// | the chip counts are over everything | `liveness` and `cwd_state` are still on every row |
/// | liveness is no staler for old rows | every row's liveness is still derived on every poll |
/// | absent is not zero | the two registry fields are untouched |
///
/// Result: **990 -> 315 B/row, 1.392 MB -> 0.443 MB, 68.2% smaller**, and
/// 37.8 MB -> 12.0 MB at 40,000 sessions.
///
/// # What was rejected, and why
///
/// - **A server-side `LIMIT`.** The obvious fix and the wrong one. Search
///   is this list's primary navigation by explicit design, and it filters
///   over the whole corpus on four fields; a limit silently turns it into
///   "search the most recent 200" -- a worse feature that still looks like
///   it works. It also makes the stated total a separate claim that can
///   drift from the rows beside it. Splitting by field gives a bigger
///   win with neither problem.
/// - **Moving search into SQL.** Preserves the corpus but not the
///   feature: the filtering is per-keystroke and a round-trip cannot be.
/// - **An `updated_since` cursor.** Liveness is derived per read and
///   changes with NO write to the row -- a session dies without touching
///   `last_activity_at` -- so rows outside the window would keep claiming
///   "running" until they happened to be written. That is precisely the
///   staleness migration 11's missing `status` column exists to prevent.
/// - **Lengthening the poll.** Does not reduce the payload, and the poll
///   is the only thing that makes a session stop saying "running".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionList {
    pub sessions: Vec<ListRow>,
    /// Every distinct liveness reason, once. `ListLiveness`'s `why` is an
    /// index into this. One entry on the measured corpus.
    pub reasons: Vec<String>,
    /// Why the live registry could not be listed. `None` with running
    /// sessions absent means "read it, nothing is running"; `Some` means
    /// "we do not know what is running".
    pub registry_failure: Option<String>,
    /// Registry files that could not be parsed, with why. Each one hides
    /// a session whose liveness cannot be stated.
    pub registry_unreadable: Vec<String>,
}

/// What one selected session knows that the list does not carry.
///
/// The heavy half of the old row (#985): 65% of the bytes, read by the
/// detail pane for ONE session at a time. Fetched on selection rather
/// than pushed for all 1,474 rows on every poll.
///
/// `liveness` is here in FULL -- the sentence, not an index -- because
/// this answers about a single session and there is nothing to intern
/// against. It is derived by the same `derive` the list uses, on this
/// request, so the detail pane never shows a liveness older than the
/// moment it was opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session_id: String,
    pub claude_version: Option<String>,
    pub transcript_path: Option<String>,
    pub first_seen_at: String,
    /// Derived NOW, for this one session.
    pub liveness: Liveness,
    /// Whether the transcript file is still on disk (#919).
    pub transcript_state: CwdState,
    /// The command to copy, already resolved against the cwd's state.
    pub resume: ResumeCommand,
    /// How many runs of this session the hook recorded.
    pub runs: usize,
    /// Why the live registry could not be listed, for THIS read. Carried
    /// for the reason [`SessionList`] carries it: a detail pane whose
    /// liveness is `Unknown` because the registry was unreadable must be
    /// able to say so rather than present a shrug as a finding.
    pub registry_failure: Option<String>,
}

/// Read every stored session, with liveness derived.
///
/// # Why every ROW, and no pagination
///
/// Still every row, and that is the decision #985 re-examined rather
/// than reversed. Paginating in SQL would force the sort and the search
/// onto the backend, where neither can respond to a keystroke, and would
/// make "showing 200 of 1,474" a server round-trip instead of a filter.
/// The frontend caps what it RENDERS and says so; it never receives a
/// silently short list, which is the house rule (#846).
///
/// What #985 DID change is which FIELDS each row carries: the heavy ones
/// are read by the detail pane for one session and now travel in
/// [`detail`] instead. 990 -> 315 bytes per row, measured, with search,
/// the chip counts, the ordering and the stated total all still over the
/// whole corpus. [`SessionList`] carries the full breakdown.
///
/// # Liveness is derived here, not stored
///
/// One registry read and one `sysinfo` refresh for the whole list,
/// scoped to the pids that could possibly be running: the registry's
/// entries plus the un-ended runs. On the real machine that is three
/// pids against 1,438 sessions, so the probe cost is proportional to
/// what is live rather than to the history.
pub fn list(conn: &Connection) -> Result<SessionList, rusqlite::Error> {
    let registry = super::liveness::registry_dir()
        .map(|d| super::liveness::read_registry(&d))
        .unwrap_or_else(|| Registry {
            failure: Some("no home directory, so the live session registry is unreachable".into()),
            ..Default::default()
        });

    let runs = runs_by_session(conn)?;

    // Only the pids that could be alive. A refresh of the whole process
    // table would cost the same whatever the answer; this asks about the
    // handful that any row's liveness could turn on.
    let mut pids: Vec<u32> = registry.entries.values().map(|e| e.pid).collect();
    for rs in runs.values() {
        for r in rs.iter().filter(|r| r.ended_at.is_none()) {
            pids.push(r.pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    let probe = SysinfoProbe::for_pids(&pids);

    let rows = stored_rows(conn)?;
    Ok(assemble(&probe, &registry, &runs, rows))
}

/// Everything ONE session knows that the list does not carry (#985).
///
/// `Ok(None)` means the id is not in the store -- a real answer, and the
/// one a session deleted between two polls produces. Distinct from an
/// `Err`, which means the database could not be read: the caller renders
/// "this session is gone" for the first and "we could not look" for the
/// second, and collapsing them would put a confident wrong answer on the
/// screen (#846).
///
/// # Why the liveness is derived again here
///
/// Rather than read back from the list's copy. Derivation is what makes a
/// session stop saying "running" (migration 11 deliberately stores no
/// `status`), and the detail pane is the surface that shows the REASON --
/// so it states one derived at the moment it was asked, not one carried
/// over from whenever the last poll happened to land.
///
/// The probe is scoped to this session's own candidate pids, so the cost
/// is a registry read and at most a handful of process lookups.
pub fn detail(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionDetail>, rusqlite::Error> {
    let Some(stored) = stored_row(conn, session_id)? else {
        return Ok(None);
    };

    let registry = super::liveness::registry_dir()
        .map(|d| super::liveness::read_registry(&d))
        .unwrap_or_else(|| Registry {
            failure: Some("no home directory, so the live session registry is unreachable".into()),
            ..Default::default()
        });
    let runs = runs_for_session(conn, session_id)?;

    let mut pids: Vec<u32> = registry.entries.values().map(|e| e.pid).collect();
    for r in runs.iter().filter(|r| r.ended_at.is_none()) {
        pids.push(r.pid);
    }
    pids.sort_unstable();
    pids.dedup();
    let probe = SysinfoProbe::for_pids(&pids);

    let liveness = derive(&probe, &registry, session_id, &runs);
    // The same cwd precedence the list uses: a live session republishes
    // its own, and the stored one is all a dead session has. Stated in
    // both places rather than shared, because the two reads happen at
    // different moments and each must be right about its own.
    let cwd = registry
        .entries
        .get(session_id)
        .and_then(|e| e.cwd.clone())
        .or_else(|| stored.cwd.clone());
    let cwd_state = check_cwd(cwd.as_deref());

    Ok(Some(SessionDetail {
        resume: resume_command(&stored.session_id, cwd.as_deref(), &cwd_state),
        transcript_state: check_transcript(stored.transcript_path.as_deref()),
        session_id: stored.session_id,
        claude_version: stored.claude_version,
        transcript_path: stored.transcript_path,
        first_seen_at: stored.first_seen_at,
        liveness,
        runs: runs.len(),
        registry_failure: registry.failure,
    }))
}

/// A stored `claude_session` row, before liveness is attached.
struct Stored {
    session_id: String,
    name: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    claude_version: Option<String>,
    transcript_path: Option<String>,
    first_seen_at: String,
    last_activity_at: Option<String>,
}

fn stored_rows(conn: &Connection) -> Result<Vec<Stored>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        // Ordered newest-activity-first in SQL, matching the index
        // migration 11 created for exactly this query
        // (`claude_session_activity`). A session with NO recorded
        // activity sorts last rather than first: `NULL` is "we never saw
        // a timestamp", and letting it sort as the newest would put the
        // two sessions we know least about at the top of the list.
        "SELECT session_id, name, cwd, git_branch, claude_version, transcript_path,
                first_seen_at, last_activity_at
         FROM claude_session
         ORDER BY last_activity_at IS NULL, last_activity_at DESC, first_seen_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Stored {
            session_id: r.get(0)?,
            name: r.get(1)?,
            cwd: r.get(2)?,
            git_branch: r.get(3)?,
            claude_version: r.get(4)?,
            transcript_path: r.get(5)?,
            first_seen_at: r.get(6)?,
            last_activity_at: r.get(7)?,
        })
    })?;
    rows.collect()
}

/// One stored row by id, or `None` when the store does not have it.
///
/// The same columns and the same shape as [`stored_rows`], so the detail
/// read cannot disagree with the list about what a session recorded.
fn stored_row(conn: &Connection, session_id: &str) -> Result<Option<Stored>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT session_id, name, cwd, git_branch, claude_version, transcript_path,
                first_seen_at, last_activity_at
         FROM claude_session WHERE session_id = ?1",
    )?;
    let mut rows = stmt.query_map([session_id], |r| {
        Ok(Stored {
            session_id: r.get(0)?,
            name: r.get(1)?,
            cwd: r.get(2)?,
            git_branch: r.get(3)?,
            claude_version: r.get(4)?,
            transcript_path: r.get(5)?,
            first_seen_at: r.get(6)?,
            last_activity_at: r.get(7)?,
        })
    })?;
    rows.next().transpose()
}

/// One session's recorded runs, newest first.
///
/// `ORDER BY started_at DESC` exactly as [`runs_by_session`] does, and
/// that is load-bearing rather than tidiness: `derive` reads
/// `runs.first()` to decide whether the newest run crashed (#965), so a
/// different order here would give the detail pane a different verdict
/// from the list for the same session.
fn runs_for_session(conn: &Connection, session_id: &str) -> Result<Vec<Run>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT pid, pid_start_time, ended_at, end_reason
         FROM claude_run WHERE session_id = ?1 ORDER BY started_at DESC",
    )?;
    let rows = stmt.query_map([session_id], |r| {
        Ok(Run {
            pid: r.get::<_, i64>(0)? as u32,
            pid_start_time: r.get(1)?,
            ended_at: r.get(2)?,
            end_reason: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// Every recorded run, newest first, grouped by session.
fn runs_by_session(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, Vec<Run>>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        // `end_reason` since #965: it was the column `crash.rs` exists to
        // write and no production query selected it, so a run the sweep
        // had recorded as crashed reached `derive` indistinguishable from
        // one that reported a clean `SessionEnd` -- and `derive`'s
        // fall-through arm then said it "reported that it ended", which
        // for a crashed run is the one thing that is definitely false.
        "SELECT session_id, pid, pid_start_time, ended_at, end_reason
         FROM claude_run ORDER BY started_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Run {
                pid: r.get::<_, i64>(1)? as u32,
                pid_start_time: r.get(2)?,
                ended_at: r.get(3)?,
                end_reason: r.get(4)?,
            },
        ))
    })?;
    let mut out: std::collections::HashMap<String, Vec<Run>> = std::collections::HashMap::new();
    for row in rows {
        let (id, run) = row?;
        out.entry(id).or_default().push(run);
    }
    Ok(out)
}

/// Attach liveness and the cwd check to each stored row.
///
/// Split from [`list`] so it can be tested against a fake probe: the
/// `Unknown` states are the ones that matter most and a real process
/// table cannot be made to fail on demand.
///
/// The resume command and the transcript's stat are NOT built here as of
/// #985 -- they are the detail pane's, and building them for 1,474 rows
/// to render one was 41% of the payload plus one `metadata` call per row
/// on every 10-second poll. [`detail`] builds both, for the session that
/// is actually open.
fn assemble<P: ProcessProbe>(
    probe: &P,
    registry: &Registry,
    runs: &std::collections::HashMap<String, Vec<Run>>,
    stored: Vec<Stored>,
) -> SessionList {
    let empty: Vec<Run> = Vec::new();
    let mut reasons = Reasons::default();
    let sessions = stored
        .into_iter()
        .map(|s| {
            let session_runs = runs.get(&s.session_id).unwrap_or(&empty);
            // Derived for EVERY row, still. The whole point of keeping
            // every row in the response: a cap or a cursor would leave
            // the rows outside its window claiming "running" until they
            // happened to be rewritten, and this poll is the only thing
            // that ever corrects that.
            let liveness = derive(probe, registry, &s.session_id, session_runs);
            // The registry's cwd wins when the session is running: the
            // transcript's cwd is where the session STARTED, and a live
            // session republishes its own. Falls back to the stored one,
            // which is the only source for a dead session.
            let cwd = registry
                .entries
                .get(&s.session_id)
                .and_then(|e| e.cwd.clone())
                .or_else(|| s.cwd.clone());
            // Stays on the list row: the Resumable and Directory-gone
            // chips switch on it, and those counts are over the whole
            // corpus.
            let cwd_state = check_cwd(cwd.as_deref());
            ListRow {
                session_id: s.session_id,
                name: s.name,
                cwd,
                git_branch: s.git_branch,
                last_activity_at: s.last_activity_at,
                liveness: reasons.intern_liveness(liveness),
                cwd_state,
            }
        })
        .collect();
    SessionList {
        sessions,
        reasons: reasons.list,
        registry_failure: registry.failure.clone(),
        registry_unreadable: registry.unreadable.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::liveness::RegistryEntry;

    const PROC_START: &str = "Fri Sep 11 09:43:48 2026";
    const PROC_START_EPOCH: i64 = 1_789_119_828;

    struct Fake(Result<Option<i64>, String>);
    impl ProcessProbe for Fake {
        fn start_time(&self, _pid: u32) -> Result<Option<i64>, String> {
            self.0.clone()
        }
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    /// Resolve one list row's liveness back to the shape `derive`
    /// produced, by looking its reason up in the list's own table.
    ///
    /// The inverse of `Reasons::intern_liveness`, and the assertions
    /// below go through it so they keep testing the SENTENCE rather than
    /// an index -- an index that happened to be 0 would satisfy a test
    /// that only compared numbers no matter which reason it named.
    fn resolved(list: &SessionList, session_id: &str) -> Liveness {
        let row = list
            .sessions
            .iter()
            .find(|s| s.session_id == session_id)
            .unwrap_or_else(|| panic!("no row {session_id}"));
        let why = |ix: usize| -> String {
            list.reasons
                .get(ix)
                .unwrap_or_else(|| {
                    panic!(
                        "{session_id}: reason index {ix} is not in a table of {} -- an \
                            out-of-range index is a row whose verdict has no grounds",
                        list.reasons.len()
                    )
                })
                .clone()
        };
        match &row.liveness {
            ListLiveness::Running { pid, status } => Liveness::Running {
                pid: *pid,
                status: status.clone(),
            },
            ListLiveness::Dead { why: ix } => Liveness::Dead { why: why(*ix) },
            ListLiveness::Unknown { why: ix } => Liveness::Unknown { why: why(*ix) },
        }
    }

    /// A directory that really exists, on every platform.
    ///
    /// These tests used the literal `"/tmp"`, which does not exist on
    /// Windows -- so `check_cwd` correctly returned `Gone` and two tests
    /// failed on the `platform (windows-latest)` job while passing
    /// locally. `temp_dir()` is what the rest of this module's tests
    /// already use, and it is `TEMP` on Windows and `/tmp` here.
    fn real_dir() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    fn insert(conn: &Connection, id: &str, cwd: Option<&str>, last: Option<&str>) {
        conn.execute(
            "INSERT INTO claude_session
                (session_id, name, cwd, first_seen_at, last_activity_at)
             VALUES (?1, ?2, ?3, '2026-09-01T00:00:00Z', ?4)",
            rusqlite::params![id, format!("about {id}"), cwd, last],
        )
        .unwrap();
    }

    // ---- #918: the three cwd cases ----

    /// A live directory gets the `cd`, and the `cd` is the whole point.
    ///
    /// Without it, `claude --resume` adopts the INVOKING directory --
    /// verified -- so a copied bare command resurrects the session
    /// pointed at whatever tree the terminal was in.
    #[test]
    fn an_existing_cwd_produces_a_cd_prefixed_command() {
        let dir = std::env::temp_dir().join("headstate-cwd-exists-918");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let state = check_cwd(Some(&path));
        assert_eq!(state, CwdState::Exists);
        let got = resume_command("abc-123", Some(&path), &state);
        assert!(got.anchored);
        assert_eq!(got.caveat, None);
        assert!(
            got.command.starts_with("cd "),
            "an existing directory must be anchored: {}",
            got.command
        );
        assert!(
            got.command.ends_with("&& claude --resume 'abc-123'"),
            "{}",
            got.command
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A missing directory gets the bare command AND a warning.
    ///
    /// 84.4% of the real corpus (1,213 of 1,438), so this is the NORMAL
    /// presentation rather than an edge case. The command still works --
    /// resume resolves by id, not by path -- which is precisely why the
    /// warning is mandatory: a command that works but lands somewhere
    /// else is the failure #918 exists to prevent.
    #[test]
    fn a_missing_cwd_produces_a_bare_command_and_says_so() {
        let path = "/Users/acme/code/widget/.worktrees/deleted-months-ago";
        let state = check_cwd(Some(path));
        assert_eq!(state, CwdState::Gone);
        let got = resume_command("abc-123", Some(path), &state);
        assert!(!got.anchored);
        assert_eq!(got.command, "claude --resume 'abc-123'");
        let caveat = got.caveat.expect("a bare command MUST carry its caveat");
        assert!(caveat.contains("gone"), "{caveat}");
        assert!(
            caveat.contains(path),
            "the caveat names the directory, so the user can tell which tree is missing: {caveat}"
        );
    }

    /// **The sabotage test for #918.** A cwd we could not CHECK says so,
    /// and its wording differs from "gone".
    ///
    /// Collapsing the two -- treating any `metadata` error as absence --
    /// is the absent-is-not-zero mistake. The remedies differ: a
    /// permission error means the tree may well be there and the `cd`
    /// would have worked, so telling the user it is "gone" sends them
    /// looking for work that has not been lost.
    #[test]
    fn a_cwd_that_could_not_be_checked_is_not_reported_as_gone() {
        let state = CwdState::Unknown {
            why: "Operation not permitted".into(),
        };
        let got = resume_command("abc-123", Some("/private/locked/tree"), &state);
        assert!(!got.anchored);
        assert_eq!(got.command, "claude --resume 'abc-123'");
        let caveat = got.caveat.expect("could-not-check MUST carry a caveat");
        assert!(
            caveat.contains("Could not check"),
            "a failed check is an admission, not a statement that the directory is gone: {caveat}"
        );
        assert!(
            !caveat.contains("is gone"),
            "could-not-check must not be worded as gone: {caveat}"
        );
        assert!(caveat.contains("Operation not permitted"), "{caveat}");
        assert_ne!(
            caveat,
            resume_command("abc-123", Some("/private/locked/tree"), &CwdState::Gone)
                .caveat
                .unwrap(),
            "the two cases must read differently, or the distinction is only in the type"
        );
    }

    /// A NotFound error is the only one that means gone.
    ///
    /// Pinned as a property of `check_cwd` rather than only of the
    /// wording above: the mapping is where the collapse would happen.
    #[test]
    fn only_not_found_becomes_gone() {
        // A path under a FILE gives NotADirectory/NotFound depending on
        // the platform, so the assertion is on the shape rather than on
        // one errno: it must never be `Exists`.
        let file = std::env::temp_dir().join("headstate-cwd-is-a-file-918");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            check_cwd(Some(&file.to_string_lossy())),
            CwdState::Gone,
            "a FILE is not a directory `cd` can enter"
        );
        let _ = std::fs::remove_file(&file);
        assert_eq!(check_cwd(None), CwdState::NotRecorded);
        assert_eq!(check_cwd(Some("")), CwdState::NotRecorded);
    }

    /// A path with a space or a shell metacharacter is quoted, so the
    /// pasted line does what the button said.
    #[test]
    fn a_path_needing_quoting_is_quoted() {
        let got = resume_command(
            "abc",
            Some("/Users/acme/my code/$(echo pwned); rm -rf ~"),
            &CwdState::Exists,
        );
        assert_eq!(
            got.command,
            "cd '/Users/acme/my code/$(echo pwned); rm -rf ~' && claude --resume 'abc'"
        );
        // And an embedded single quote survives.
        let quoted = resume_command("abc", Some("/Users/acme/it's here"), &CwdState::Exists);
        assert_eq!(
            quoted.command,
            r"cd '/Users/acme/it'\''s here' && claude --resume 'abc'"
        );
    }

    /// **The ID is quoted too**, not just the path.
    ///
    /// `session_id` comes verbatim from a transcript's `file_stem()` with
    /// no format validation anywhere, so a file named `$(whoami).jsonl`
    /// under `~/.claude/projects/<slug>/` became a copied command carrying
    /// live shell syntax. Found by review: the path was quoted while the
    /// id was interpolated bare, under a comment promising the whole line
    /// was safe.
    #[test]
    fn the_session_id_is_quoted_as_well_as_the_path() {
        let nasty = "$(whoami)`id`; rm -rf ~";
        // Both halves, on the anchored shape.
        let anchored = resume_command(nasty, Some("/tmp/x"), &CwdState::Exists);
        assert_eq!(
            anchored.command,
            "cd '/tmp/x' && claude --resume '$(whoami)`id`; rm -rf ~'"
        );
        // And on the BARE shape, which is 84% of rows and where there is
        // no path quoting to hide behind.
        let bare = resume_command(nasty, None, &CwdState::NotRecorded);
        assert_eq!(bare.command, "claude --resume '$(whoami)`id`; rm -rf ~'");
        // Nothing outside the quotes on either.
        for cmd in [anchored.command, bare.command] {
            let after = cmd.split("--resume ").nth(1).unwrap();
            assert!(
                after.starts_with('\'') && after.ends_with('\''),
                "the id must be wholly inside single quotes: {after}"
            );
        }
        // An id containing a single quote is escaped, not terminated.
        let quoted = resume_command("a'b", None, &CwdState::NotRecorded);
        assert_eq!(quoted.command, r"claude --resume 'a'\''b'");
    }

    /// A session with no recorded cwd is its own case, not `Gone`.
    #[test]
    fn a_session_with_no_recorded_cwd_says_that_rather_than_gone() {
        let got = resume_command("abc", None, &CwdState::NotRecorded);
        let caveat = got.caveat.unwrap();
        assert!(
            caveat.contains("No working directory was recorded"),
            "{caveat}"
        );
        assert!(!caveat.contains("gone"), "{caveat}");
    }

    // ---- #919: the transcript is a DIFFERENT path from the cwd ----

    /// A transcript that is on disk is `Exists` -- and `check_cwd` on the
    /// same path is `Gone`.
    ///
    /// This is the reuse trap #919 walks into if the two checks share one
    /// function: `check_cwd` requires a DIRECTORY, so pointing it at a
    /// transcript file reports the 100%-surviving path as gone and the
    /// reveal button disappears on every row. The assertion below is the
    /// guard against someone "simplifying" the two into one call.
    #[test]
    fn a_live_transcript_exists_where_the_cwd_check_would_call_it_gone() {
        let dir = std::env::temp_dir().join("headstate-transcript-919");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c8518222.jsonl");
        std::fs::write(&file, b"{}\n").unwrap();
        let path = file.to_string_lossy().into_owned();

        assert_eq!(
            check_transcript(Some(&path)),
            CwdState::Exists,
            "a transcript on disk must be revealable"
        );
        assert_eq!(
            check_cwd(Some(&path)),
            CwdState::Gone,
            "the cwd check requires a directory, which is exactly why the transcript \
             needs its own check rather than reusing this one"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory at the transcript's path is not a transcript.
    ///
    /// The mirror of the test above, and it matters because
    /// `!metadata.is_dir()` would have passed here while `is_file()`
    /// does not. A Finder window opened on a directory we called a
    /// transcript is the silent-nothing failure in a different costume.
    #[test]
    fn a_directory_at_the_transcript_path_is_not_a_transcript() {
        let dir = std::env::temp_dir().join("headstate-transcript-isdir-919");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            check_transcript(Some(&dir.to_string_lossy())),
            CwdState::Gone
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A missing transcript is `Gone`, and a session with none recorded
    /// is `NotRecorded` -- two different facts.
    ///
    /// `NotRecorded` happens for a hook-sourced session Headstate saw
    /// start before any transcript import ran: we know the session
    /// exists and do not know where its transcript is. Rendering that as
    /// "the transcript is gone" would send the user looking for a deleted
    /// file that was never missing.
    #[test]
    fn a_missing_transcript_is_gone_and_an_unrecorded_one_is_not() {
        let missing = std::env::temp_dir().join("headstate-919-no-such-transcript.jsonl");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(
            check_transcript(Some(&missing.to_string_lossy())),
            CwdState::Gone
        );
        assert_eq!(check_transcript(None), CwdState::NotRecorded);
        assert_eq!(check_transcript(Some("")), CwdState::NotRecorded);
    }

    /// **The sabotage test for #919.** A transcript we could not CHECK
    /// reports `Unknown` with the reason, and never `Gone`.
    ///
    /// This provokes a REAL `EACCES` from the operating system rather
    /// than constructing `CwdState::Unknown` by hand -- a hand-built
    /// value proves the type has three variants, not that `check_path`
    /// ever produces the third one. Chmod 000 on the parent directory
    /// makes `metadata` on the child fail with a permission error while
    /// the file is still there, which is precisely the case that must
    /// not read as absence: the remedy is "fix the permission", not
    /// "the transcript was deleted".
    ///
    /// `#[cfg(unix)]` because Windows has no chmod and its ACL
    /// equivalent is not a one-liner. Two agents in this epic shipped
    /// Windows-only failures by assuming a Unix filesystem behaviour
    /// held on both, so this asserts nothing about Windows rather than
    /// guessing. The platform-independent half of the distinction is
    /// covered by `an_unknown_transcript_is_not_reported_as_gone` below,
    /// which runs everywhere.
    #[cfg(unix)]
    #[test]
    fn a_transcript_that_could_not_be_checked_is_not_reported_as_gone() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join("headstate-919-eacces");
        let locked = root.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let file = locked.join("c8518222.jsonl");
        std::fs::write(&file, b"{}\n").unwrap();
        let path = file.to_string_lossy().into_owned();

        // Sanity: readable BEFORE the sabotage, so a failure below is
        // the chmod and not a broken fixture.
        assert_eq!(check_transcript(Some(&path)), CwdState::Exists);

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let got = check_transcript(Some(&path));
        // Restore BEFORE asserting, so a failure cannot leak an
        // undeletable directory into the temp dir -- the pattern
        // `transcript::tests::an_unreadable_project_directory...` uses.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&root);

        // Running as root defeats the sabotage: root reads through mode
        // 000. Skip rather than assert a falsehood -- a test that only
        // passes because it was not actually sabotaged is worse than no
        // test, and CI containers do run as root.
        if got == CwdState::Exists {
            eprintln!("skipped: mode 000 did not deny this user (running as root?)");
            return;
        }

        match &got {
            CwdState::Unknown { why } => {
                assert!(
                    !why.is_empty(),
                    "an Unknown with no reason is indistinguishable from a shrug"
                );
                eprintln!("real EACCES surfaced as: Unknown {{ why: {why:?} }}");
            }
            other => panic!(
                "a permission error must NOT collapse into an absence verdict -- \
                 the file is still on disk. Got {other:?}"
            ),
        }
        assert_ne!(
            got,
            CwdState::Gone,
            "this is the whole of the absent-is-not-zero rule for #919"
        );
    }

    /// The could-not-check and gone verdicts are distinguishable by a
    /// caller on EVERY platform, not only where chmod works.
    ///
    /// The `#[cfg(unix)]` test above proves the OS error really reaches
    /// `Unknown`; this proves the two verdicts do not compare equal, so a
    /// UI matching on the state cannot render them the same way by
    /// accident.
    #[test]
    fn an_unknown_transcript_is_not_reported_as_gone() {
        let unknown = CwdState::Unknown {
            why: "Permission denied (os error 13)".into(),
        };
        assert_ne!(unknown, CwdState::Gone);
        assert_ne!(unknown, CwdState::NotRecorded);
        assert_ne!(unknown, CwdState::Exists);
        // And the reason survives, because a "could not check" with
        // nothing to act on is not materially better than "gone".
        let CwdState::Unknown { why } = &unknown else {
            unreachable!()
        };
        assert!(why.contains("Permission denied"));
    }

    /// BOTH states are carried, and a dead cwd does not drag the
    /// transcript down with it.
    ///
    /// This is the 83%-vs-0% asymmetry as an assertion: the common real
    /// row has a deleted worktree and a perfectly readable transcript,
    /// and a UI that gated the transcript's reveal on `cwd_state` would
    /// hide the one button that works.
    ///
    /// Since #985 the two states live in two tiers -- the cwd's on the
    /// list row, because the chips count it over the whole corpus, and
    /// the transcript's on the detail, because only the reveal button
    /// reads it. So this also asserts they still agree across the split:
    /// the point was never which struct holds them, it is that neither is
    /// derived from the other.
    #[test]
    fn a_gone_cwd_leaves_the_transcript_state_untouched() {
        let dir = std::env::temp_dir().join("headstate-919-both-states");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c8518222.jsonl");
        std::fs::write(&file, b"{}\n").unwrap();

        let conn = db();
        conn.execute(
            "INSERT INTO claude_session
                (session_id, cwd, transcript_path, first_seen_at)
             VALUES ('s1', '/Users/acme/code/widget/.worktrees/deleted', ?1,
                     '2026-09-01T00:00:00Z')",
            rusqlite::params![file.to_string_lossy()],
        )
        .unwrap();

        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(got.sessions[0].cwd_state, CwdState::Gone);

        let d = detail(&conn, "s1").unwrap().expect("the session is stored");
        assert_eq!(
            d.transcript_state,
            CwdState::Exists,
            "the transcript survives the worktree, which is the whole point of \
             carrying two states"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- #917: the list ----

    /// Newest activity first, and a session with no activity sorts LAST.
    ///
    /// Not start time: a session touched ten minutes ago matters more
    /// than one started earlier and abandoned. And `NULL` last-activity
    /// is "we never saw a timestamp", so sorting it as the newest would
    /// put the two rows we know least about at the top of 1,438.
    #[test]
    fn the_list_is_newest_activity_first_and_undated_rows_sort_last() {
        let conn = db();
        insert(&conn, "old", None, Some("2026-01-01T00:00:00Z"));
        insert(&conn, "undated", None, None);
        insert(&conn, "new", None, Some("2026-09-01T00:00:00Z"));
        let got = stored_rows(&conn).unwrap();
        let ids: Vec<&str> = got.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["new", "old", "undated"]);
    }

    /// A failed registry read reaches the list, and every row's liveness
    /// is `Unknown`.
    ///
    /// The registry directory is mode `0700`, so this is real. The list
    /// must still carry its rows -- a partial answer labelled partial
    /// beats an error page -- while saying nothing about what is
    /// running.
    #[test]
    fn an_unreadable_registry_reaches_the_list_and_poisons_every_liveness() {
        let conn = db();
        insert(&conn, "s1", Some(&real_dir()), Some("2026-09-01T00:00:00Z"));
        insert(&conn, "s2", None, Some("2026-09-02T00:00:00Z"));
        let registry = Registry {
            failure: Some("Permission denied".into()),
            ..Default::default()
        };
        let got = assemble(
            &Fake(Ok(Some(PROC_START_EPOCH))),
            &registry,
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(got.sessions.len(), 2, "the rows are still shown");
        assert_eq!(got.registry_failure.as_deref(), Some("Permission denied"));
        for s in &got.sessions {
            let liveness = resolved(&got, &s.session_id);
            assert!(
                matches!(liveness, Liveness::Unknown { .. }),
                "{}: {:?}",
                s.session_id,
                liveness
            );
        }
        let s0 = got.sessions.iter().find(|s| s.session_id == "s1").unwrap();
        assert_eq!(
            s0.cwd.as_deref(),
            Some(real_dir().as_str()),
            "the stored cwd is the fallback"
        );

        // The RESUME action survives the registry failure, and that
        // matters: liveness and the resume command come from different
        // sources, so a registry we could not read must not also cost the
        // user the `cd` that the stored transcript cwd already provides.
        // Rendering these rows with a bare command would be the failure
        // of #918 caused by an unrelated failure of #917.
        //
        // Asserted through `detail` since #985, which is where the resume
        // command is now built -- the real registry is readable here, so
        // this proves the FALLBACK to the stored cwd rather than the
        // registry's own, which is what the original test was about.
        let d2 = detail(&conn, "s2").unwrap().expect("s2 is stored");
        assert!(!d2.resume.anchored, "s2 has no cwd at all");
        let d1 = detail(&conn, "s1").unwrap().expect("s1 is stored");
        assert!(
            d1.resume.anchored,
            "the stored cwd must still carry the `cd`: {}",
            d1.resume.command
        );
    }

    /// A running session is reported running, with its published
    /// busy/idle refinement.
    #[test]
    fn a_running_session_carries_its_status() {
        let conn = db();
        insert(&conn, "s1", Some(&real_dir()), Some("2026-09-01T00:00:00Z"));
        let mut registry = Registry::default();
        registry.entries.insert(
            "s1".into(),
            RegistryEntry {
                pid: 14779,
                session_id: "s1".into(),
                proc_start: Some(PROC_START.into()),
                status: Some("busy".into()),
                ..Default::default()
            },
        );
        let got = assemble(
            &Fake(Ok(Some(PROC_START_EPOCH))),
            &registry,
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(
            resolved(&got, "s1"),
            Liveness::Running {
                pid: 14779,
                status: Some("busy".into())
            }
        );
        // The pid and the status travel on the list row itself rather
        // than through the reason table: a running row has no `why`, so
        // interning must not have invented one for it.
        assert!(
            got.reasons.is_empty(),
            "a running session has no reason to intern: {:?}",
            got.reasons
        );
    }

    /// The live registry's cwd wins over the transcript's, for a session
    /// the registry knows about.
    ///
    /// The transcript records where a session STARTED; a live session
    /// republishes its own. They differ after a `cd` inside the session,
    /// and the live one is the directory a resume should land in.
    #[test]
    fn the_registry_cwd_wins_for_a_session_it_knows_about() {
        let conn = db();
        insert(&conn, "s1", Some("/nonexistent/where-it-started"), None);
        let mut registry = Registry::default();
        registry.entries.insert(
            "s1".into(),
            RegistryEntry {
                pid: 1,
                session_id: "s1".into(),
                proc_start: Some(PROC_START.into()),
                cwd: Some(real_dir()),
                ..Default::default()
            },
        );
        let got = assemble(
            &Fake(Ok(Some(PROC_START_EPOCH))),
            &registry,
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(got.sessions[0].cwd.as_deref(), Some(real_dir().as_str()));
    }

    /// A transcript-imported session -- zero runs, no registry entry, and
    /// a registry we read WHOLE -- is `Dead` with `runs: 0` (#984).
    ///
    /// This is the entire historical corpus (~1,400 rows), so it is the
    /// common case, and it is why the old `Unknown` here made the
    /// tri-state carry nothing: 1,490 of 1,491 rows read "could not tell"
    /// while `overview::aggregate` called 183 of them resumable from the
    /// same registry read.
    ///
    /// `runs: 0` stays alongside it and is a DIFFERENT fact: "not running"
    /// is the verdict, "never observed" is how much was watched, and the
    /// detail pane states both.
    #[test]
    fn an_imported_session_reports_dead_and_no_runs() {
        let conn = db();
        insert(&conn, "s1", None, Some("2026-09-01T00:00:00Z"));
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        // `runs` is the detail's since #985 -- the list neither renders
        // nor counts it -- so the two facts are asserted from the two
        // tiers that now carry them.
        assert_eq!(detail(&conn, "s1").unwrap().expect("stored").runs, 0);
        match resolved(&got, "s1") {
            Liveness::Dead { why } => assert!(
                why.contains("live session registry"),
                "the verdict must state the absence it rests on: {why}"
            ),
            other => panic!("expected Dead from a complete registry listing, got {other:?}"),
        }
    }

    /// `end_reason` reaches `derive` from the database (#965).
    ///
    /// The defect was in the SQL, not in the derivation: the production
    /// query selected four columns and `end_reason` was not one of them,
    /// so `crash.rs`'s classification could not reach `derive` at all and
    /// a crashed run read as a self-reported clean exit. This asserts the
    /// plumbing over a real `claude_run` row rather than over a
    /// hand-built `Run`, because a hand-built one would pass with the old
    /// `SELECT` still in place.
    #[test]
    fn a_crashed_run_in_the_database_reaches_the_liveness_reason() {
        let conn = db();
        insert(&conn, "s1", None, Some("2026-09-01T00:00:00Z"));
        conn.execute(
            "INSERT INTO claude_run
               (session_id, pid, pid_start_time, source, end_reason, started_at, ended_at)
             VALUES ('s1', 4242, ?1, 'sweep', ?2, '2026-09-01T00:00:00Z', '2026-09-01T01:00:00Z')",
            rusqlite::params![PROC_START, crate::claude::crash::CRASHED],
        )
        .unwrap();

        let runs = runs_by_session(&conn).unwrap();
        assert_eq!(
            runs["s1"][0].end_reason.as_deref(),
            Some(crate::claude::crash::CRASHED),
            "the query must select the column crash.rs exists to write"
        );

        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &runs,
            stored_rows(&conn).unwrap(),
        );
        match resolved(&got, "s1") {
            Liveness::Dead { why } => assert!(
                why.contains("without shutting down"),
                "a run the sweep recorded as crashed must not read as a clean exit: {why}"
            ),
            other => panic!("expected Dead, got {other:?}"),
        }
        // And the DETAIL pane -- the surface that actually prints this
        // sentence -- reaches the same verdict from its own query. The
        // two reads use different SQL since #985, and `derive` decides
        // "crashed" from `runs.first()`, so a detail query that ordered
        // its runs differently would report a crash as a clean exit on
        // the one screen that shows the words.
        match detail(&conn, "s1").unwrap().expect("stored").liveness {
            Liveness::Dead { why } => assert!(
                why.contains("without shutting down"),
                "the detail must agree with the list about the crash: {why}"
            ),
            other => panic!("expected Dead from detail, got {other:?}"),
        }
    }

    /// A run that ended CLEANLY still reads as a clean exit (#965).
    ///
    /// The pair to the test above: the new reader must not turn every
    /// ended run into a crash report. Same row shape, one column
    /// different.
    #[test]
    fn a_cleanly_ended_run_in_the_database_still_reads_as_a_clean_exit() {
        let conn = db();
        insert(&conn, "s1", None, Some("2026-09-01T00:00:00Z"));
        conn.execute(
            "INSERT INTO claude_run
               (session_id, pid, pid_start_time, source, end_reason, started_at, ended_at)
             VALUES ('s1', 4242, ?1, 'hook', 'clear', '2026-09-01T00:00:00Z', '2026-09-01T01:00:00Z')",
            rusqlite::params![PROC_START],
        )
        .unwrap();

        let runs = runs_by_session(&conn).unwrap();
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &runs,
            stored_rows(&conn).unwrap(),
        );
        match resolved(&got, "s1") {
            Liveness::Dead { why } => assert!(
                !why.contains("without shutting down"),
                "`clear` is the hook's vocabulary and means a clean exit ran: {why}"
            ),
            other => panic!("expected Dead, got {other:?}"),
        }
        match detail(&conn, "s1").unwrap().expect("stored").liveness {
            Liveness::Dead { why } => assert!(
                !why.contains("without shutting down"),
                "the detail must not turn a clean exit into a crash: {why}"
            ),
            other => panic!("expected Dead from detail, got {other:?}"),
        }
    }

    /// An empty database is an empty list, and the emptiness is NOT
    /// itself reported as a failure -- which is what licenses the UI's
    /// "no sessions found" copy.
    ///
    /// Asserted through `assemble` with an explicitly clean registry
    /// rather than through `list`, which would read the developer's real
    /// `~/.claude/sessions` and make the assertion depend on the machine.
    /// (An earlier version of this test called `list` and asserted
    /// `is_none() || is_some()`, which is a tautology that would have
    /// passed whatever the code did.)
    #[test]
    fn an_empty_database_is_an_empty_list_and_not_a_failure() {
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            Vec::new(),
        );
        assert!(got.sessions.is_empty());
        assert_eq!(
            got.registry_failure, None,
            "an empty session table must not invent a registry failure: the UI \
             distinguishes the two and only one of them licenses 'no sessions'"
        );
        assert!(got.registry_unreadable.is_empty());
    }

    /// End to end against the real machine's registry and a real row.
    ///
    /// Proves the pieces compose -- the registry read, the pid probe, the
    /// cwd check and the command -- rather than asserting a count that
    /// depends on what the developer is running. Both tiers, since #985:
    /// the command moved to `detail` and the same composition has to
    /// hold there.
    #[test]
    fn list_composes_against_the_real_registry() {
        let conn = db();
        insert(&conn, "s1", Some(&real_dir()), Some("2026-09-01T00:00:00Z"));
        let got = list(&conn).unwrap();
        assert_eq!(got.sessions.len(), 1);
        assert_eq!(
            got.sessions[0].cwd_state,
            CwdState::Exists,
            "the temp directory exists on every platform"
        );

        let d = detail(&conn, "s1").unwrap().expect("the session is stored");
        assert!(d.resume.anchored);
        assert!(d.resume.command.contains("claude --resume 's1'"));
        eprintln!(
            "liveness for an unobserved session: {:?}",
            resolved(&got, "s1")
        );
    }

    // ---- #985: the two tiers ----

    /// The list carries every ROW, and the total is just how many there
    /// are.
    ///
    /// The invariant the whole change rests on. #985's trap is a
    /// server-side `LIMIT` that makes the stated total a separate claim
    /// which can drift from the rows beside it; here there is nothing to
    /// drift, because `sessions.len()` IS the total and the frontend
    /// reads it from the array it is already rendering.
    ///
    /// Boundary-tested either side of `RENDER_CAP`, which is the only cap
    /// in the feature and is a rendering budget in the client: 200 rows
    /// exactly, and 201. Both must arrive whole.
    #[test]
    fn every_row_arrives_however_many_there_are() {
        for n in [200usize, 201] {
            let conn = db();
            for i in 0..n {
                // Zero-padded so the string ordering the SQL applies is
                // the numeric one, making the assertion below about the
                // ORDER as well as the count.
                insert(
                    &conn,
                    &format!("s{i:04}"),
                    None,
                    Some(&format!("2026-09-01T00:00:{:02}Z", i % 60)),
                );
            }
            // Through `list`, NOT `assemble`: the truncation this test
            // exists to forbid would live in `list`, between the query
            // and the assembly, and a test that called `assemble` with
            // its own rows would step right over it. Proven by sabotage
            // -- a `rows.truncate(200)` in `list` passed the
            // `assemble` version of this test.
            let got = list(&conn).unwrap();
            assert_eq!(
                got.sessions.len(),
                n,
                "{n} rows stored must be {n} rows returned -- a response that \
                 dropped rows would make the frontend's stated total a lie"
            );
            let ids: std::collections::HashSet<&str> =
                got.sessions.iter().map(|s| s.session_id.as_str()).collect();
            assert_eq!(ids.len(), n, "every row is distinct");
        }
    }

    /// Every row's liveness is still derived, whatever its position.
    ///
    /// The reason no row-dropping approach was acceptable (see
    /// [`SessionList`]). Liveness changes with NO write to the row -- a
    /// session dies without touching `last_activity_at` -- so a cap or an
    /// `updated_since` cursor would leave rows outside its window
    /// claiming "running" indefinitely. This asserts the property that
    /// rules those out: the LAST row is as freshly derived as the first.
    #[test]
    fn the_last_row_is_as_freshly_derived_as_the_first() {
        let conn = db();
        for i in 0..250 {
            insert(
                &conn,
                &format!("s{i:04}"),
                None,
                Some(&format!("2026-09-01T00:00:{:02}Z", i % 60)),
            );
        }
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(got.sessions.len(), 250);
        for s in &got.sessions {
            let liveness = resolved(&got, &s.session_id);
            match liveness {
                Liveness::Dead { why } => assert!(
                    !why.is_empty(),
                    "{}: a verdict with no grounds",
                    s.session_id
                ),
                other => panic!("{}: expected a derived Dead, got {other:?}", s.session_id),
            }
        }
    }

    /// The interned reason is the SENTENCE, unchanged, once.
    ///
    /// The saving is only legitimate if nothing is lost: the list shows
    /// this string in every row's `title`, so interning has to be a
    /// transport encoding and not a truncation. Two sessions with the
    /// same verdict share one entry; the sentence each resolves to is the
    /// one `derive` produced.
    #[test]
    fn one_reason_is_carried_once_and_resolves_to_the_same_sentence() {
        let conn = db();
        insert(&conn, "s1", None, Some("2026-09-01T00:00:00Z"));
        insert(&conn, "s2", None, Some("2026-09-02T00:00:00Z"));
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(
            got.reasons.len(),
            1,
            "two identical verdicts must be carried once: {:?}",
            got.reasons
        );
        let direct = derive(&Fake(Ok(None)), &Registry::default(), "s1", &[]);
        let Liveness::Dead { why: expected } = direct else {
            panic!("expected Dead");
        };
        for id in ["s1", "s2"] {
            match resolved(&got, id) {
                Liveness::Dead { why } => assert_eq!(
                    why, expected,
                    "{id}: interning must preserve the sentence exactly -- the list \
                     renders it as the row's title"
                ),
                other => panic!("{id}: {other:?}"),
            }
        }
    }

    /// DIFFERENT verdicts get different entries, and nothing is merged.
    ///
    /// The failure interning could introduce: collapsing two distinct
    /// reasons onto one index would put one session's grounds under
    /// another session's verdict, which is worse than dropping `why`
    /// altogether because it is confidently wrong.
    #[test]
    fn two_different_reasons_are_not_merged() {
        let conn = db();
        insert(&conn, "dead", None, Some("2026-09-01T00:00:00Z"));
        insert(&conn, "unknown", None, Some("2026-09-02T00:00:00Z"));
        // A run with no comparable start time is the `Unknown` arm, so
        // this row's verdict differs from the other's by construction.
        conn.execute(
            "INSERT INTO claude_run
               (session_id, pid, source, started_at)
             VALUES ('unknown', 4242, 'hook', '2026-09-01T00:00:00Z')",
            [],
        )
        .unwrap();
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &runs_by_session(&conn).unwrap(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(
            got.reasons.len(),
            2,
            "two distinct verdicts need two entries: {:?}",
            got.reasons
        );
        let dead = resolved(&got, "dead");
        let unknown = resolved(&got, "unknown");
        assert!(matches!(dead, Liveness::Dead { .. }), "{dead:?}");
        assert!(matches!(unknown, Liveness::Unknown { .. }), "{unknown:?}");
        let (Liveness::Dead { why: a }, Liveness::Unknown { why: b }) = (&dead, &unknown) else {
            unreachable!()
        };
        assert_ne!(a, b, "each verdict keeps its own grounds");
    }

    /// An id the store does not have is `None`, not an error and not a
    /// fabricated row.
    ///
    /// What a session deleted between two polls produces. `None` and
    /// `Err` are different answers with different remedies -- "this
    /// session is gone" versus "we could not look" -- and #846 is the
    /// rule that they must not be collapsed.
    #[test]
    fn detail_for_an_unknown_id_is_none_rather_than_an_error() {
        let conn = db();
        insert(&conn, "s1", None, Some("2026-09-01T00:00:00Z"));
        assert!(
            detail(&conn, "never-stored").unwrap().is_none(),
            "an absent id is an answer, not a failure"
        );
        assert!(detail(&conn, "s1").unwrap().is_some());
    }

    /// The detail carries the fields the list gave up, for the row the
    /// list still names.
    ///
    /// The split's correctness condition: every field that left `ListRow`
    /// has to be reachable for the selected session, or the detail pane
    /// lost information rather than the transport did.
    #[test]
    fn the_detail_carries_what_the_list_no_longer_does() {
        let dir = std::env::temp_dir().join("headstate-985-detail-fields");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s1.jsonl");
        std::fs::write(&file, b"{}\n").unwrap();

        let conn = db();
        conn.execute(
            "INSERT INTO claude_session
                (session_id, name, cwd, claude_version, transcript_path,
                 first_seen_at, last_activity_at)
             VALUES ('s1', 'about s1', ?1, '2.0.1', ?2,
                     '2026-09-01T00:00:00Z', '2026-09-02T00:00:00Z')",
            rusqlite::params![real_dir(), file.to_string_lossy()],
        )
        .unwrap();

        let d = detail(&conn, "s1").unwrap().expect("stored");
        assert_eq!(d.session_id, "s1");
        assert_eq!(d.claude_version.as_deref(), Some("2.0.1"));
        assert_eq!(d.first_seen_at, "2026-09-01T00:00:00Z");
        assert_eq!(
            d.transcript_path.as_deref(),
            Some(file.to_string_lossy().as_ref())
        );
        assert_eq!(d.transcript_state, CwdState::Exists);
        assert!(d.resume.anchored, "{}", d.resume.command);
        assert!(d.resume.command.contains("claude --resume 's1'"));

        // And the list still names the same session, with the fields it
        // kept -- so the two halves describe one row rather than two.
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        let row = &got.sessions[0];
        assert_eq!(row.session_id, "s1");
        assert_eq!(row.name.as_deref(), Some("about s1"));
        assert_eq!(row.cwd.as_deref(), Some(real_dir().as_str()));
        assert_eq!(
            row.last_activity_at.as_deref(),
            Some("2026-09-02T00:00:00Z")
        );
        assert_eq!(row.cwd_state, CwdState::Exists);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The four fields search covers are on EVERY list row.
    ///
    /// #985's stated trap: search is this list's primary navigation and
    /// filters the whole corpus on `name`, `cwd`, `git_branch` and
    /// `session_id`. Any of the four missing from the list tier would
    /// silently narrow search to what happens to still carry it -- a
    /// worse feature that still looks like it works.
    #[test]
    fn every_field_search_covers_survives_the_split() {
        let conn = db();
        conn.execute(
            "INSERT INTO claude_session
                (session_id, name, cwd, git_branch, first_seen_at, last_activity_at)
             VALUES ('findable-id', 'findable name', '/findable/cwd', 'findable-branch',
                     '2026-09-01T00:00:00Z', '2026-09-02T00:00:00Z')",
            [],
        )
        .unwrap();
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        let row = &got.sessions[0];
        // Each one separately, so a failure names the field that went
        // missing rather than reporting "search broke".
        assert_eq!(row.session_id, "findable-id");
        assert_eq!(row.name.as_deref(), Some("findable name"));
        assert_eq!(row.cwd.as_deref(), Some("/findable/cwd"));
        assert_eq!(row.git_branch.as_deref(), Some("findable-branch"));
    }

    /// The WHOLE pipeline against the real corpus: import, then list.
    ///
    /// This is what the Claude Code view actually shows, measured rather
    /// than imagined -- and it is how the list's design decisions were
    /// settled. It prints the distribution rather than asserting counts,
    /// because the corpus grows every time the developer uses `claude`.
    ///
    /// `#[ignore]`d for the reason `transcript::tests::real_corpus` is:
    /// CI has no transcript corpus, so an assertion about 1,438 sessions
    /// would fail there for the right reason and the wrong outcome. Run
    /// with
    /// `cargo test --lib real_session_list -- --ignored --nocapture`.
    ///
    /// What it DOES assert are the invariants that must hold at any size:
    /// every row carries a resume command that mentions its own id, every
    /// unanchored command carries a caveat, and no row is left in a state
    /// with neither a reading nor a reason.
    #[test]
    #[ignore = "needs the developer's own ~/.claude/projects"]
    fn real_session_list() {
        let mut conn = db();
        let scan = match crate::claude::scan_default() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("no corpus to measure: {e}");
                return;
            }
        };
        let scanned = scan.sessions.len();
        let imported = crate::claude::store::import(&mut conn, scan).unwrap();
        let started = std::time::Instant::now();
        let got = list(&conn).unwrap();
        let list_ms = started.elapsed().as_millis();

        let mut running = 0;
        let mut dead = 0;
        let mut unknown = 0;
        let mut anchored = 0;
        let mut gone = 0;
        let mut uncheckable = 0;
        let mut unnamed = 0;
        let mut undated = 0;
        // #919: the transcript's survival, measured rather than assumed
        // to match the cwd's. The two columns printed side by side are
        // the evidence for carrying two fields.
        let mut t_exists = 0;
        let mut t_gone = 0;
        let mut t_uncheckable = 0;
        let mut t_unrecorded = 0;
        let mut dead_cwd_live_transcript = 0;
        for s in &got.sessions {
            match resolved(&got, &s.session_id) {
                Liveness::Running { .. } => running += 1,
                Liveness::Dead { why } => {
                    dead += 1;
                    assert!(!why.is_empty(), "{}: Dead with no reason", s.session_id);
                }
                Liveness::Unknown { why } => {
                    unknown += 1;
                    assert!(!why.is_empty(), "{}: Unknown with no reason", s.session_id);
                }
            }
            match &s.cwd_state {
                CwdState::Exists => anchored += 1,
                CwdState::Gone => gone += 1,
                CwdState::Unknown { .. } => uncheckable += 1,
                CwdState::NotRecorded => {}
            }
            // The detail for EVERY row, which is not what the app does --
            // it fetches one -- but is what proves the per-row invariants
            // still hold across the whole corpus after #985 split them
            // off the list.
            let d = detail(&conn, &s.session_id)
                .unwrap()
                .unwrap_or_else(|| panic!("{}: listed but has no detail", s.session_id));
            match &d.transcript_state {
                CwdState::Exists => t_exists += 1,
                CwdState::Gone => t_gone += 1,
                CwdState::Unknown { .. } => t_uncheckable += 1,
                CwdState::NotRecorded => t_unrecorded += 1,
            }
            if s.cwd_state == CwdState::Gone && d.transcript_state == CwdState::Exists {
                dead_cwd_live_transcript += 1;
            }
            if s.name.is_none() {
                unnamed += 1;
            }
            if s.last_activity_at.is_none() {
                undated += 1;
            }
            // The invariants that hold at any corpus size.
            assert!(
                d.resume.command.contains(&s.session_id),
                "{}: the resume command must carry its own handle",
                s.session_id
            );
            assert_eq!(
                d.resume.anchored,
                matches!(s.cwd_state, CwdState::Exists),
                "{}: the detail's anchoring must agree with the LIST's cwd check -- \
                 the two are read separately since #985 and a disagreement means one \
                 of them is describing a different moment",
                s.session_id
            );
            assert!(
                d.resume.anchored || d.resume.caveat.is_some(),
                "{}: a command with no `cd` MUST say why -- this is the whole of #918",
                s.session_id
            );
            assert!(
                !d.resume.anchored || d.resume.caveat.is_none(),
                "{}: an anchored command needs no caveat",
                s.session_id
            );
        }

        eprintln!("--- the Claude Code list, against the real corpus ---");
        eprintln!("scanned transcripts      {scanned}");
        eprintln!("stored sessions          {}", imported.sessions);
        eprintln!(
            "subagent files skipped   {}",
            imported.subagent_files_skipped
        );
        eprintln!("import elapsed           {}ms", imported.elapsed_ms);
        eprintln!("rows returned            {}", got.sessions.len());
        eprintln!("list elapsed             {list_ms}ms  (registry read + pid probe + query)");
        eprintln!("registry failure         {:?}", got.registry_failure);
        eprintln!("registry unreadable      {}", got.registry_unreadable.len());
        eprintln!("liveness  running {running}  dead {dead}  unknown {unknown}");
        let n = got.sessions.len().max(1);
        eprintln!(
            "cwd        exists {anchored}  gone {gone}  uncheckable {uncheckable}  ({:.1}% gone)",
            100.0 * gone as f64 / n as f64
        );
        // #919's headline finding: the two paths do NOT survive alike.
        eprintln!(
            "transcript exists {t_exists}  gone {t_gone}  uncheckable {t_uncheckable}  \
             unrecorded {t_unrecorded}  ({:.1}% gone)",
            100.0 * t_gone as f64 / n as f64
        );
        eprintln!(
            "rows with a DEAD cwd and a LIVE transcript: {dead_cwd_live_transcript} \
             ({:.1}%) -- each one is a reveal-transcript button that a \
             cwd-gated UI would have hidden",
            100.0 * dead_cwd_live_transcript as f64 / n as f64
        );
        eprintln!("no aiTitle name          {unnamed}");
        eprintln!("no recorded activity     {undated}");

        // #985: the PAYLOAD, which is the cost this view actually pays.
        // The server work above is 5ms; the response is what crosses the
        // pairing transport every ten seconds, and it is what the split
        // was for. Printed rather than asserted for the reason the rest
        // of this test prints: the corpus grows.
        let list_bytes = serde_json::to_vec(&got).unwrap().len();
        let one_detail = serde_json::to_vec(&detail(&conn, &got.sessions[0].session_id).unwrap())
            .unwrap()
            .len();
        eprintln!("--- #985: what the poll costs ---");
        eprintln!(
            "list payload             {list_bytes} bytes ({:.3} MB, {:.1} B/row)",
            list_bytes as f64 / 1_048_576.0,
            list_bytes as f64 / n as f64
        );
        eprintln!("distinct liveness reasons {}", got.reasons.len());
        eprintln!("one session's detail     {one_detail} bytes, fetched on selection");
        eprintln!(
            "at 40,000 sessions       {:.1} MB per poll",
            40_000.0 * (list_bytes as f64 / n as f64) / 1_048_576.0
        );
        eprintln!("--- the first ten rows as the list orders them ---");
        for s in got.sessions.iter().take(10) {
            eprintln!(
                "  {:<9} {:<52} {}",
                match &s.liveness {
                    ListLiveness::Running { status, .. } =>
                        format!("live/{}", status.as_deref().unwrap_or("?")),
                    ListLiveness::Dead { .. } => "dead".into(),
                    ListLiveness::Unknown { .. } => "unknown".into(),
                },
                s.name.as_deref().unwrap_or("(no aiTitle)"),
                detail(&conn, &s.session_id)
                    .unwrap()
                    .unwrap()
                    .resume
                    .command
            );
        }

        assert_eq!(
            got.sessions.len(),
            imported.sessions,
            "every stored session must reach the list"
        );
    }
}
