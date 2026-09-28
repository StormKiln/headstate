//! Claude Code sessions: what Headstate knows about them and where it
//! learned it.
//!
//! Epic #910. Today this holds the transcript importer (#914), which is
//! the source that works retroactively: `~/.claude/projects` already
//! contains every session that ever ran on the machine, so the Claude
//! Code view opens with real history rather than an empty list waiting
//! for a hook to fire.
//!
//! The hook path (#912, #913) is the other source and will land beside
//! this one. They meet in [`store`], which upserts on `session_id` and
//! resolves the overlap per field by which source actually knows the
//! answer -- see [`store::import`].
//!
//! [`handoff`] consumes what the hook appends (#913), and [`registry`]
//! reads `~/.claude/sessions/`, the live session registry -- a THIRD
//! source the epic missed, and the best of the three for liveness: it
//! carries the pid, `procStart`, `sessionId`, `cwd` and `name` with no
//! hook installed at all. A registry file whose pid is dead is a
//! positive crash signal, which [`crash`] records; see its module docs
//! for why that is strictly better than inferring a crash from a missing
//! `SessionEnd`.
//!
//! `~/.claude` is read-only by design: those transcripts are Claude
//! Code's data and the files `claude --resume` depends on. There are
//! exactly TWO exceptions, and each is narrow for its own reason.
//!
//! [`handoff::consume`] truncates the handoff file after committing the
//! records it read. That file is Headstate's OWN -- the hook writes it,
//! nothing else reads it -- and rotation is the only side that knows
//! which records are already stored.
//!
//! [`install`] (#915) appends two hook matchers to
//! `~/.claude/settings.json`, a file shared with other tools, and refuses
//! to touch anything it cannot parse.
//!
//! Nothing else in here writes to `~/.claude` at all. In particular the
//! registry under `~/.claude/sessions/` is Claude Code's, and one of its
//! files is rewritten by its owner every few seconds.
//!
//! [`overview`] (#921) is the aggregate layer for the overview page. It
//! counts over the rows [`store`] holds and derives no liveness of its
//! own -- #917's `liveness` module owns that, and two answers to one
//! question disagree the first time either changes. Since #1534 it reads
//! each row's verdict off the session list itself; the `live.rs` seam it
//! used before #917 landed is gone.
//!
//! They DID disagree, and #984 is the instance: `overview::aggregate`
//! counts a session resumable the moment it is absent from the running set,
//! while `liveness::derive` returned `Unknown` for every session the hook
//! had never observed -- 1,490 of 1,491 rows. One page said "183 are ready
//! to resume", the other said it could not tell whether any of them was
//! running, off the same registry read. `derive` adopted the overview's
//! reading -- a registry listing read whole that does not name a session
//! is positive evidence -- and
//! `tests::the_two_pages_agree_about_the_same_rows` now pins the agreement
//! rather than leaving it to two comments.
//!
//! [`install`] (#915) is the ONE exception to the read-only rule below, and
//! it is narrow on purpose: it appends two hook matchers to
//! `~/.claude/settings.json` and refuses to touch anything it cannot parse.
//! Nothing else in here writes to `~/.claude`. The transcripts in
//! particular are read-only by design -- they are Claude Code's data and the
//! files `claude --resume` depends on.

/// A content-free nudge when a running session's transcript changes
/// (#1477): session id, byte size, sequence number -- no path, no text.
pub mod activity;
pub mod cli;
/// Silently-broken agent configuration, swept across repositories (#1217).
pub mod confighealth;
/// What the app has READ, against what it HOLDS (#1212).
///
/// A read-only aggregate over columns the modules around it already
/// own, so the coverage caveats each of them argues in prose reach the
/// screen ONCE rather than as a footnote per panel.
pub mod coverage;
pub mod crash;
pub mod definitions;
/// A compact, content-free status per session, for the phone's
/// best-effort notifications (#1486). Carries no transcript text.
pub mod digest;
pub mod events;
/// The restart list: every running session's resume command, as text to
/// save before a reboot (#1071).
pub mod export;
/// Generated transcripts shaped like real ones, for measuring (#1487).
/// Test-only: written at test or bench time, never committed.
#[cfg(test)]
pub(crate) mod fixtures;
pub mod handoff;
pub mod hook;
pub mod install;
/// Opening the user's configured terminal on a built command (#1126).
pub mod launch;
/// Pull request links written since the last import, read incrementally
/// on the live pass (#1557). Read-only on `~/.claude`.
pub mod linkscan;
pub mod liveness;
/// Every MCP server configured on this machine, and which scope defines
/// it (#1216). Reads `~/.claude.json` -- Claude Code's LIVE state file --
/// read-only and bounded, and treats a parse failure as a refusal rather
/// than an empty inventory.
pub mod mcp;
pub mod overview;
/// Which permission rules in `settings.json` are Headstate's (#1199).
///
/// The THIRD writer question, answered before the first write rather
/// than after the first support report. Its ledger is Headstate's own
/// file and lives in Headstate's own data directory -- so the rule above
/// still holds: `install` is the only thing in here that writes to
/// `~/.claude`, and #1199 did not make it two.
pub mod permissions;
/// Installed plugins, and what they were actually used for (#1075).
pub mod plugins;
/// The tail of one transcript, as conversation rather than JSONL (#982).
pub mod preview;
/// What the transcript reads cost against those fixtures (#1487).
#[cfg(test)]
mod read_bench;
pub mod registry;
pub mod search;
pub mod sessions;
pub mod settings;
/// Compaction pressure, stated agent types, and who is waiting on you
/// (#1065, #1066, #1067).
pub mod signals;
/// Proposing and carrying out a stop of a live session (#1219).
///
/// The ONE place in this tree that signals a Claude Code process, and it
/// does not write to `~/.claude` at all -- see its header for why
/// signalling is not what this module's read-only rule governs, and for
/// the SIGTERM-first and re-derived-pid constraints it ships under.
pub mod stop;
pub mod store;
/// Which sessions are subagents, and which session spawned each (#1002).
pub mod subagent;
/// Which model and how much autonomy a launched session starts on
/// (#1214). A closed vocabulary rendered to argv by Rust, never a
/// command string from the caller.
pub mod terms;
pub mod transcript;
/// A transcript as stable, render-ready messages: ids, turns and the
/// full record allowlist (#1475).
pub mod transcript_model;
/// Paged reads of a transcript: bounded pages before or after a cursor,
/// at any position in any size of file (#1220).
pub mod transcript_page;
/// Streaming a record too large to hold, without holding it (#1220).
pub mod transcript_skim;
/// Per-message token usage, summed per session (#959).
pub mod usage;

// Re-exported so `commands.rs` names the operation rather than the module it
// happens to live in. Dropping these breaks the CALL SITE rather than the
// module, which is how a merge has eaten them twice in this epic -- the error
// names a function in a module that still contains it, and five CI checks
// fail for one missing line. Do not remove them to "tidy" a conflict.
pub use transcript::{corpus, corpus_default, scan, scan_default, Scan, Transcript};

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::liveness::{ProcessProbe, Registry, RegistryEntry, UnnamedRecord};
    use super::sessions::ListLiveness;

    const PROC_START: &str = "Fri Sep 11 09:43:48 2026";
    const PROC_START_RFC3339: &str = "2026-09-11T09:43:48+00:00";
    const PROC_START_EPOCH: i64 = 1_789_119_828;

    /// A process table the test chooses: each live pid's start time and
    /// working directory.
    struct Table(HashMap<u32, PathBuf>);
    impl ProcessProbe for Table {
        fn start_time(&self, pid: u32) -> Result<Option<i64>, String> {
            Ok(self.0.get(&pid).map(|_| PROC_START_EPOCH))
        }
        fn cwd(&self, pid: u32) -> Result<Option<PathBuf>, String> {
            Ok(self.0.get(&pid).cloned())
        }
    }

    /// The session list and the overview page agree about the same rows
    /// (#984, #1534).
    ///
    /// The defect was never a wrong number on either page; it was that the
    /// two answered the same question differently from one database. #984
    /// was the list hedging where the overview did not. #1534 was the
    /// reverse: the overview counted from `live.rs`, which knew only "which
    /// ids are positively running", so a row the list called "could not
    /// tell" -- a terminal-launched session running in its folder -- was
    /// offered for resumption, and resuming a live session starts a second
    /// copy of it.
    ///
    /// This is the guard, and it lives HERE rather than in either module
    /// because it is about the pair. It drives the overview's real path,
    /// `overview::report_with`, and the SAME list it reads, over one
    /// registry and one process table, and asserts row by row that the
    /// overview offers Resume only on a row the list calls `dead`.
    ///
    /// The rows cover every way the two have disagreed:
    ///
    /// | row | evidence | list | overview |
    /// |---|---|---|---|
    /// | `running-one` | registry entry, pid alive | running | running |
    /// | `unwatched-live-dir` | nothing, folder exists | dead | resumable |
    /// | `unwatched-dead-dir` | nothing, folder gone | dead | archived |
    /// | `hedged` | a live `.key`-only process in its folder | unknown | neither |
    /// | `named` | a `.key`-only process its run names | running | running |
    /// | `neighbour` | same folder as `named`, no runs | dead | resumable |
    ///
    /// Sabotage: counting a list `Unknown` as stopped in `overview::verdicts`
    /// puts `hedged` in the resumable list and fails the first assertion
    /// block; dropping the `.key` naming leaves `named` and `neighbour`
    /// hedged and fails the second.
    #[test]
    fn the_two_pages_agree_about_the_same_rows() {
        let root = tempfile::TempDir::new().unwrap();
        let dir = |name: &str| {
            let d = root.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let live_dir = dir("live");
        let hedged_dir = dir("hedged");
        let named_dir = dir("named");

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let gone = root.path().join("deleted-worktree");
        for (id, cwd) in [
            ("running-one", &live_dir),
            ("unwatched-live-dir", &live_dir),
            ("unwatched-dead-dir", &gone),
            ("hedged", &hedged_dir),
            ("named", &named_dir),
            ("neighbour", &named_dir),
        ] {
            conn.execute(
                "INSERT INTO claude_session (session_id, cwd, first_seen_at, last_activity_at)
                 VALUES (?1, ?2, '2026-09-01T00:00:00Z', '2026-09-12T00:00:00Z')",
                rusqlite::params![id, cwd.display().to_string()],
            )
            .unwrap();
        }
        // The hook saw `named` start in pid 6161, and the consumer stored
        // the `.key`'s confirmed start time, as RFC 3339.
        conn.execute(
            "INSERT INTO claude_run (session_id, pid, pid_start_time, started_at, ended_at)
             VALUES ('named', 6161, ?1, '2026-09-11T09:43:49Z', NULL)",
            [PROC_START_RFC3339],
        )
        .unwrap();

        // ONE registry read: one named session, and two terminal launches
        // that published only a `.key`.
        let key = |pid: u32| UnnamedRecord {
            pid,
            proc_start: Some(PROC_START.into()),
            path: format!("/fixture/.claude/sessions/{pid}.0123abcd.key"),
        };
        let mut registry = Registry {
            unnamed: vec![key(5151), key(6161)],
            ..Default::default()
        };
        registry.entries.insert(
            "running-one".into(),
            RegistryEntry {
                pid: 4242,
                session_id: "running-one".into(),
                proc_start: Some(PROC_START.into()),
                ..Default::default()
            },
        );
        let table = || {
            Table(HashMap::from([
                (4242, live_dir.clone()),
                (5151, hedged_dir.clone()),
                (6161, named_dir.clone()),
            ]))
        };

        let list = super::sessions::list_probed(&conn, &registry, |_| table()).unwrap();
        let report =
            super::overview::report_with(&conn, &registry, |_| table(), chrono::Utc::now())
                .unwrap();

        let verdict = |id: &str| {
            list.sessions
                .iter()
                .find(|r| r.session_id == id)
                .map(|r| r.liveness.clone())
                .unwrap()
        };

        // The #1534 row: the list cannot tell, so the overview must not
        // offer it -- and must count it rather than drop it.
        assert!(
            matches!(verdict("hedged"), ListLiveness::Unknown { .. }),
            "{:?}",
            verdict("hedged")
        );
        let offered: Vec<&str> = report
            .overview
            .resumable
            .iter()
            .map(|r| r.session_id.as_str())
            .collect();
        assert!(!offered.contains(&"hedged"), "{offered:?}");
        assert_eq!(report.overview.counts.liveness_unknown, 1);

        // The narrowing: the `.key` in `named_dir` IS `named`, so that row
        // runs and its neighbour is settled again.
        assert!(
            matches!(verdict("named"), ListLiveness::Running { pid: 6161, .. }),
            "{:?}",
            verdict("named")
        );
        assert!(
            matches!(verdict("neighbour"), ListLiveness::Dead { .. }),
            "{:?}",
            verdict("neighbour")
        );

        // The overview's own figures, which are the list's by construction.
        let c = &report.overview.counts;
        assert_eq!(c.running, 2, "running-one and named");
        assert_eq!(c.resumable, 2, "unwatched-live-dir and neighbour");
        assert_eq!(c.archived, 1);
        assert_eq!(c.cwd_unknown, 0);
        let mut offered_sorted = offered.clone();
        offered_sorted.sort_unstable();
        assert_eq!(offered_sorted, ["neighbour", "unwatched-live-dir"]);

        // "Running is at least N": the one process nothing named is
        // reported beside the count, and the named one is not.
        assert_eq!(report.live_unnamed.len(), 1, "{:?}", report.live_unnamed);
        assert!(report.live_unnamed[0].contains("5151"));
        assert_eq!(report.live_unnamed, list.registry_unnamed);

        // The invariant, stated once: every row the overview OFFERS for
        // resurrection is a row the list calls stopped.
        for id in &offered {
            assert!(
                matches!(verdict(id), ListLiveness::Dead { .. }),
                "{id} is offered as resumable, so the list must call it stopped"
            );
        }
    }
}
