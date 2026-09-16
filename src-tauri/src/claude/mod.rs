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
//! question disagree the first time either changes. [`live`] is the seam
//! between them until #917 lands, and its own comment says so.
//!
//! They DID disagree, and #984 is the instance: `overview::aggregate`
//! counts a session resumable the moment it is absent from the running set,
//! while `liveness::derive` returned `Unknown` for every session the hook
//! had never observed -- 1,490 of 1,491 rows. One page said "183 are ready
//! to resume", the other said it could not tell whether any of them was
//! running, off the same registry read. `derive` adopted the overview's
//! reading, which is the one `live.rs`'s own
//! `a_missing_registry_is_a_settled_empty_answer` states, and
//! `tests::the_two_pages_agree_about_the_same_rows` now pins the agreement
//! rather than leaving it to two comments.
//!
//! [`install`] (#915) is the ONE exception to the read-only rule below, and
//! it is narrow on purpose: it appends two hook matchers to
//! `~/.claude/settings.json` and refuses to touch anything it cannot parse.
//! Nothing else in here writes to `~/.claude`. The transcripts in
//! particular are read-only by design -- they are Claude Code's data and the
//! files `claude --resume` depends on.

pub mod cli;
pub mod crash;
pub mod handoff;
pub mod hook;
pub mod install;
pub mod live;
pub mod liveness;
pub mod overview;
/// The tail of one transcript, as conversation rather than JSONL (#982).
pub mod preview;
pub mod registry;
pub mod sessions;
/// Compaction pressure, stated agent types, and who is waiting on you
/// (#1065, #1066, #1067).
pub mod signals;
pub mod store;
/// Which sessions are subagents, and which session spawned each (#1002).
pub mod subagent;
pub mod transcript;
/// Per-message token usage, summed per session (#959).
pub mod usage;

// Re-exported so `commands.rs` names the operation rather than the module it
// happens to live in. Dropping these breaks the CALL SITE rather than the
// module, which is how a merge has eaten them twice in this epic -- the error
// names a function in a module that still contains it, and five CI checks
// fail for one missing line. Do not remove them to "tidy" a conflict.
pub use transcript::{scan, scan_default, Scan, Transcript};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    /// The session list and the overview page agree about the same rows
    /// (#984).
    ///
    /// The defect was not a wrong number on either page; it was that the
    /// two answered the same question differently from one database.
    /// `overview::aggregate` classified a session as resumable exactly when
    /// it was not in the running set, and `liveness::derive` returned
    /// `Unknown` -- "could not tell whether it is running" -- for every
    /// session the hook had never observed, which was 1,490 of 1,491 real
    /// rows. So one page offered 183 sessions for resurrection while the
    /// other declined to say whether any of them was alive.
    ///
    /// This is the guard, and it lives HERE rather than in either module
    /// because neither can see the other: `overview` deliberately takes the
    /// running set as a parameter (`aggregate`'s doc says why) and
    /// `liveness` never reads the aggregate. A test inside either one would
    /// pin half a contract.
    ///
    /// It asserts the SHAPE rather than the corpus's numbers, over three
    /// rows chosen to cover the disagreement: one live, one unwatched with
    /// a directory that exists, one unwatched with a directory that is
    /// gone. Every row the overview is willing to offer for resurrection
    /// must be a row the list calls `dead` -- not `unknown`, because "could
    /// not tell" beside a green Resume tile is the contradiction, and not
    /// `running`, because resuming a live session starts a second copy.
    #[test]
    fn the_two_pages_agree_about_the_same_rows() {
        use super::liveness::{derive, Liveness, ProcessProbe, Registry, RegistryEntry};
        use super::overview::aggregate;

        const PROC_START: &str = "Fri Sep 11 09:43:48 2026";
        const PROC_START_EPOCH: i64 = 1_789_119_828;

        struct Alive;
        impl ProcessProbe for Alive {
            fn start_time(&self, _pid: u32) -> Result<Option<i64>, String> {
                Ok(Some(PROC_START_EPOCH))
            }
        }

        // A directory that really exists on every platform. A hardcoded
        // `/tmp` was one of this epic's four Windows-only failures.
        let live_dir = std::env::temp_dir().display().to_string();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        for (id, cwd) in [
            ("running-one", live_dir.as_str()),
            ("unwatched-live-dir", live_dir.as_str()),
            ("unwatched-dead-dir", "/nonexistent/deleted-worktree"),
        ] {
            conn.execute(
                "INSERT INTO claude_session (session_id, cwd, first_seen_at, last_activity_at)
                 VALUES (?1, ?2, '2026-09-01T00:00:00Z', '2026-09-12T00:00:00Z')",
                rusqlite::params![id, cwd],
            )
            .unwrap();
        }

        // ONE registry read, listed cleanly, naming only the live session.
        // This is the state both pages were reasoning from when they
        // disagreed, and the premise the new inference rests on.
        let mut registry = Registry::default();
        registry.entries.insert(
            "running-one".into(),
            RegistryEntry {
                pid: 4242,
                session_id: "running-one".into(),
                proc_start: Some(PROC_START.into()),
                ..Default::default()
            },
        );
        assert_eq!(registry.failure, None, "the premise: a complete listing");
        assert!(registry.unreadable.is_empty());

        let running: HashSet<String> = registry.entries.keys().cloned().collect();
        let over = aggregate(&conn, &running, chrono::Utc::now()).unwrap();

        // The overview's classification of the three rows.
        assert_eq!(over.counts.running, 1);
        assert_eq!(
            over.counts.resumable, 1,
            "the one that is not live and whose directory still exists"
        );
        assert_eq!(over.counts.archived, 1);
        assert_eq!(over.counts.cwd_unknown, 0);
        let offered: Vec<&str> = over
            .resumable
            .iter()
            .map(|r| r.session_id.as_str())
            .collect();
        assert_eq!(offered, ["unwatched-live-dir"]);

        // And the list's, row by row. The `unknown` arm is what made the
        // two pages contradict each other, so it is asserted against
        // explicitly rather than merely not expected.
        for id in ["running-one", "unwatched-live-dir", "unwatched-dead-dir"] {
            let liveness = derive(&Alive, &registry, id, &[]);
            match (id, &liveness) {
                ("running-one", Liveness::Running { .. }) => {}
                ("unwatched-live-dir" | "unwatched-dead-dir", Liveness::Dead { .. }) => {}
                (id, other) => panic!(
                    "{id} reads as {other:?} -- a session the overview counts as resumable \
                     or archived must read as 'not running' on the list, and 'could not \
                     tell' beside a green Resume tile is the contradiction #984 is about"
                ),
            }
        }

        // The invariant, stated once rather than as three cases: every row
        // the overview actually OFFERS for resurrection is a row the list
        // says is not running.
        for id in &offered {
            assert!(
                matches!(derive(&Alive, &registry, id, &[]), Liveness::Dead { .. }),
                "{id} is offered as resumable, so the list must not hedge about it"
            );
        }
    }
}
