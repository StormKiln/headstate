//! What the Claude Code view renders: one row per session (#917), and
//! the resume command that actually works (#918).
//!
//! Reads `claude_session` and `claude_run` (migration 11), pairs each row
//! with a liveness derived from the machine ([`super::liveness`]), and
//! answers the one question the resume action turns on: does the
//! directory this session ran in still exist?
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
    /// The path exists and is a directory.
    Exists,
    /// The path is definitely not there. 84% of the real corpus.
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
    let Some(path) = cwd else {
        return CwdState::NotRecorded;
    };
    if path.is_empty() {
        return CwdState::NotRecorded;
    }
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => CwdState::Exists,
        // A path that exists but is a FILE is not somewhere `cd` can go.
        // Reported as gone-for-this-purpose with the reason said out
        // loud rather than as `Exists`, which would produce a `cd` that
        // fails in the user's shell after they pasted it.
        Ok(_) => CwdState::Gone,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CwdState::Gone,
        Err(e) => CwdState::Unknown {
            why: format!("{e}"),
        },
    }
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

/// One row of the session list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRow {
    /// The `claude --resume` handle, and the row's identity everywhere.
    pub session_id: String,
    /// Claude's own `aiTitle`, when the transcript had one. `None` for
    /// the two sessions in 1,438 that never got one -- never the UUID in
    /// disguise, because a fabricated name is indistinguishable from a
    /// real one. The UI decides what to show instead.
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub claude_version: Option<String>,
    pub transcript_path: Option<String>,
    pub first_seen_at: String,
    /// The newest record in the transcript, NOT the time we scanned.
    /// `None` when nothing in the transcript carried a timestamp.
    pub last_activity_at: Option<String>,
    /// Derived every read, never stored. Three states.
    pub liveness: Liveness,
    pub cwd_state: CwdState,
    /// The command to copy, already resolved against `cwd_state`.
    ///
    /// Built on the backend rather than in the component because the
    /// existence check is a filesystem read the frontend cannot do, and
    /// splitting the check from the string it produces is how the two
    /// drift into a command whose caveat no longer matches it.
    pub resume: ResumeCommand,
    /// How many runs of this session the hook recorded. `0` for every
    /// transcript-imported session, which is the whole historical corpus
    /// -- shown so a reader can tell "never observed" from "observed and
    /// ended", which is also the difference between two `Liveness`
    /// answers.
    pub runs: usize,
}

/// The session list, INCLUDING what could not be read.
///
/// `registry_failure` and `registry_unreadable` are the point of the
/// type. A list rendered from a registry we could not read is a list in
/// which every liveness is `Unknown`, and the view has to say so rather
/// than show 1,438 rows that look like settled answers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionList {
    pub sessions: Vec<SessionRow>,
    /// Why the live registry could not be listed. `None` with running
    /// sessions absent means "read it, nothing is running"; `Some` means
    /// "we do not know what is running".
    pub registry_failure: Option<String>,
    /// Registry files that could not be parsed, with why. Each one hides
    /// a session whose liveness cannot be stated.
    pub registry_unreadable: Vec<String>,
}

/// Read every stored session, with liveness and resume command derived.
///
/// # Why every row, and no pagination
///
/// The whole list is 1,438 rows of small strings -- measured, and the
/// figure the truncation decision rests on. Paginating in SQL would
/// force the sort and the search onto the backend, where neither can
/// respond to a keystroke, and would make "showing 200 of 1,438" a
/// server round-trip instead of a filter. The frontend caps what it
/// RENDERS and says so; it never receives a silently short list, which
/// is the house rule (#846).
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

/// Every recorded run, newest first, grouped by session.
fn runs_by_session(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, Vec<Run>>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT session_id, pid, pid_start_time, ended_at
         FROM claude_run ORDER BY started_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Run {
                pid: r.get::<_, i64>(1)? as u32,
                pid_start_time: r.get(2)?,
                ended_at: r.get(3)?,
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

/// Attach liveness, the cwd check and the resume command to each stored
/// row.
///
/// Split from [`list`] so it can be tested against a fake probe: the
/// `Unknown` states are the ones that matter most and a real process
/// table cannot be made to fail on demand.
fn assemble<P: ProcessProbe>(
    probe: &P,
    registry: &Registry,
    runs: &std::collections::HashMap<String, Vec<Run>>,
    stored: Vec<Stored>,
) -> SessionList {
    let empty: Vec<Run> = Vec::new();
    let sessions = stored
        .into_iter()
        .map(|s| {
            let session_runs = runs.get(&s.session_id).unwrap_or(&empty);
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
            let cwd_state = check_cwd(cwd.as_deref());
            let resume = resume_command(&s.session_id, cwd.as_deref(), &cwd_state);
            SessionRow {
                session_id: s.session_id,
                name: s.name,
                cwd,
                git_branch: s.git_branch,
                claude_version: s.claude_version,
                transcript_path: s.transcript_path,
                first_seen_at: s.first_seen_at,
                last_activity_at: s.last_activity_at,
                liveness,
                cwd_state,
                resume,
                runs: session_runs.len(),
            }
        })
        .collect();
    SessionList {
        sessions,
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
            assert!(
                matches!(s.liveness, Liveness::Unknown { .. }),
                "{}: {:?}",
                s.session_id,
                s.liveness
            );
        }
        // The RESUME action survives the registry failure, and that
        // matters: liveness and the resume command come from different
        // sources, so a registry we could not read must not also cost the
        // user the `cd` that the stored transcript cwd already provides.
        // Rendering these rows with a bare command would be the failure
        // of #918 caused by an unrelated failure of #917.
        let s1 = got.sessions.iter().find(|s| s.session_id == "s2").unwrap();
        assert!(!s1.resume.anchored, "s2 has no cwd at all");
        let s0 = got.sessions.iter().find(|s| s.session_id == "s1").unwrap();
        assert_eq!(
            s0.cwd.as_deref(),
            Some(real_dir().as_str()),
            "the stored cwd is the fallback"
        );
        assert!(
            s0.resume.anchored,
            "an unreadable registry must not cost the `cd`: {}",
            s0.resume.command
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
            got.sessions[0].liveness,
            Liveness::Running {
                pid: 14779,
                status: Some("busy".into())
            }
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

    /// A transcript-imported session -- zero runs, no registry entry --
    /// is `Unknown` with `runs: 0`.
    ///
    /// This is the entire historical corpus (~1,400 rows), so it is the
    /// common case. `runs: 0` is what lets the UI say "never observed"
    /// rather than implying we watched and lost it.
    #[test]
    fn an_imported_session_reports_unknown_and_no_runs() {
        let conn = db();
        insert(&conn, "s1", None, Some("2026-09-01T00:00:00Z"));
        let got = assemble(
            &Fake(Ok(None)),
            &Registry::default(),
            &Default::default(),
            stored_rows(&conn).unwrap(),
        );
        assert_eq!(got.sessions[0].runs, 0);
        assert!(matches!(got.sessions[0].liveness, Liveness::Unknown { .. }));
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
    /// depends on what the developer is running.
    #[test]
    fn list_composes_against_the_real_registry() {
        let conn = db();
        insert(&conn, "s1", Some(&real_dir()), Some("2026-09-01T00:00:00Z"));
        let got = list(&conn).unwrap();
        assert_eq!(got.sessions.len(), 1);
        let row = &got.sessions[0];
        assert_eq!(
            row.cwd_state,
            CwdState::Exists,
            "the temp directory exists on every platform"
        );
        assert!(row.resume.anchored);
        assert!(row.resume.command.contains("claude --resume 's1'"));
        eprintln!("liveness for an unobserved session: {:?}", row.liveness);
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
        for s in &got.sessions {
            match &s.liveness {
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
            if s.name.is_none() {
                unnamed += 1;
            }
            if s.last_activity_at.is_none() {
                undated += 1;
            }
            // The invariants that hold at any corpus size.
            assert!(
                s.resume.command.contains(&s.session_id),
                "{}: the resume command must carry its own handle",
                s.session_id
            );
            assert_eq!(
                s.resume.anchored,
                matches!(s.cwd_state, CwdState::Exists),
                "{}: anchored must agree with the cwd check",
                s.session_id
            );
            assert!(
                s.resume.anchored || s.resume.caveat.is_some(),
                "{}: a command with no `cd` MUST say why -- this is the whole of #918",
                s.session_id
            );
            assert!(
                !s.resume.anchored || s.resume.caveat.is_none(),
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
        eprintln!(
            "cwd       exists {anchored}  gone {gone}  uncheckable {uncheckable}  ({:.1}% gone)",
            100.0 * gone as f64 / got.sessions.len().max(1) as f64
        );
        eprintln!("no aiTitle name          {unnamed}");
        eprintln!("no recorded activity     {undated}");
        eprintln!("--- the first ten rows as the list orders them ---");
        for s in got.sessions.iter().take(10) {
            eprintln!(
                "  {:<9} {:<52} {}",
                match &s.liveness {
                    Liveness::Running { status, .. } =>
                        format!("live/{}", status.as_deref().unwrap_or("?")),
                    Liveness::Dead { .. } => "dead".into(),
                    Liveness::Unknown { .. } => "unknown".into(),
                },
                s.name.as_deref().unwrap_or("(no aiTitle)"),
                s.resume.command
            );
        }

        assert_eq!(
            got.sessions.len(),
            imported.sessions,
            "every stored session must reach the list"
        );
    }
}
