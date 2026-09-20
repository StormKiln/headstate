//! What the app has READ, against what it HOLDS (#1212, epic #1121).
//!
//! # Why this module exists at all
//!
//! Every number Headstate shows about the Claude corpus carries a
//! coverage caveat, and until now each caveat was re-stated at its own
//! call site. [`usage::Profile`] names the three ways a token total goes
//! short; [`sessions::SubagentRollup`] separates "read it and it had
//! none" from "could not read it"; [`overview::Counts::never_observed`]
//! is called "the honest companion to `orphaned_runs`" because without
//! it "a reader cannot tell 'nothing crashed' from 'nothing was
//! watched'". Each argument is good and each lives alone, so as panels
//! land the footnotes multiply and drift.
//!
//! This is the one place that states the scope. It computes NOTHING new:
//! every figure here is a count over a column another module already
//! owns, and where that module exposes the aggregate this calls it
//! rather than re-deriving it. A second derivation of the same number is
//! how #984 shipped, where the overview and the session list disagreed
//! about the same rows off the same read.
//!
//! # Why this is not a score
//!
//! `CLAUDE.md`'s rule is "qualify, or suppress". Qualifying means "28 of
//! 37", not "76%" and never a grade. The figures below are deliberately
//! NOT combinable: sessions whose cost was measured, sessions a hook
//! watched, and sessions whose directory still exists are three
//! different questions over the same denominator, and averaging them
//! would produce a number that answers none of them while looking like
//! it answers all three.
//!
//! A grade is precisely the fabrication these counts exist to prevent.
//! So this module ships counts with their denominator attached, in a
//! shape that makes a rolled-up figure awkward to construct: there is no
//! total, no percentage and no ordering by severity anywhere in it.
//!
//! # Why absent coverage is scope and not damage
//!
//! [`overview::Counts::archived`] carries the house precedent: the
//! archived sessions are the majority of the corpus, and its doc insists
//! that is "the NORMAL state and must not be rendered as damage". The
//! same is true of every figure here. A session recorded before the
//! usage importer existed has no cost row, and that is not a fault in
//! the session, the importer or the machine -- it is the boundary of
//! what was being measured at the time.
//!
//! So [`Reach`] distinguishes the boundary from the fault. `Measured`
//! and `OutOfScope` are both settled, expected answers; only `Unread` is
//! a thing that went wrong, and it is the smallest category here. A
//! panel that renders all three alike turns a statement of scope into a
//! defect list, which is the failure mode this module is written to
//! avoid.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// The three ways a session can stand against one measurement.
///
/// Three and not two, because collapsing any pair loses the distinction
/// the caveats exist to preserve -- the same three
/// [`crate::claude::sessions::SubagentRollup`] keeps apart per child,
/// stated once for the corpus.
///
/// The variants are NOT ordered by severity and must not be summed. Two
/// of them are settled answers and one is a failure, which is a
/// difference in kind rather than in degree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    /// Sessions this measurement was actually taken over.
    ///
    /// The only figure here that licenses a total. A zero in the sums
    /// that hang off this is a MEASURED zero and may be rendered as one.
    pub measured: u64,
    /// Sessions the measurement does not cover, with nothing wrong.
    ///
    /// A session that predates the importer, a hook that was not
    /// installed when it ran, a transcript carrying no usage block at
    /// all. Each is a settled answer -- we looked, or we know we never
    /// looked, and either way there is nothing to retry.
    ///
    /// This is the BIGGEST number on most machines and it is not a
    /// problem. See the module docs and
    /// [`crate::claude::overview::Counts::archived`].
    pub out_of_scope: u64,
    /// Sessions whose read was attempted and did not complete.
    ///
    /// The only failure in the struct, and the reason it cannot be
    /// folded into `out_of_scope`: a total short by this many is short
    /// by an UNKNOWN amount, where one short by `out_of_scope` is short
    /// by a known and expected one. `caches/mod.rs:550`'s rule.
    pub unread: u64,
}

impl Reach {
    /// Every session this measurement was considered against.
    ///
    /// The denominator, carried as a method rather than a field so it
    /// cannot drift from its parts. A figure rendered without it is the
    /// defect this module exists to prevent.
    pub fn total(&self) -> u64 {
        self.measured + self.out_of_scope + self.unread
    }

    /// Whether anything was measured at all.
    ///
    /// The absent-is-not-zero gate, matching
    /// [`crate::claude::sessions::SubagentRollup::observed`]. `false`
    /// means the sums this qualifies must render as "not measured" and
    /// never as zero.
    pub fn observed(&self) -> bool {
        self.measured > 0
    }
}

/// One measurement's reach, with the words that say what it covers.
///
/// The label and the noun travel WITH the counts rather than living in
/// the view, for [`crate::store::scans::CachedScan::stale`]'s reason:
/// carried rather than re-derived at each call site, so the Rust side
/// and the view cannot come to disagree about what a figure counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    /// A stable identifier, e.g. `cost`. Never shown.
    pub id: String,
    /// What this measures, as a heading: "Token and cost totals".
    pub label: String,
    /// What the denominator counts, singular: "session".
    ///
    /// Carried so the view writes "of 1,453 sessions" without knowing
    /// which noun each row takes -- a future measurement over files or
    /// runs says so here rather than in a branch in the component.
    pub unit: String,
    /// Why the out-of-scope rows are out of scope, in one clause.
    ///
    /// Required, not optional. A count of uncovered rows with no reason
    /// beside it reads as a defect list, which is exactly what this
    /// panel must not be. Phrased as a boundary ("recorded before ...")
    /// and never as a fault ("missing", "failed").
    pub scope_note: String,
    pub reach: Reach,
}

/// What the app has read, against what it holds.
///
/// # Why `sessions` sits at the top rather than per row
///
/// Every [`Reach`] here has the same denominator today, and stating it
/// once is how a reader sees that the rows are comparable at all. It is
/// still repeated inside each `Reach` via [`Reach::total`], because a
/// row lifted out of this struct into a tooltip must carry its own
/// denominator -- the top-level figure is context, not the source of
/// truth for any row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageReport {
    /// Every session Headstate has a row for.
    ///
    /// The same figure [`crate::claude::overview::Counts::sessions`]
    /// carries and for the same reason: present as the denominator the
    /// other figures are read against, never as a hero number.
    pub sessions: u64,
    /// One row per measurement, in a FIXED order.
    ///
    /// Fixed and not sorted, deliberately. Ordering by how much is
    /// uncovered would rank the rows by badness, which is a grade
    /// expressed as a layout -- and it would also make the panel's shape
    /// change under the reader as the corpus grows.
    pub measurements: Vec<Measurement>,
    /// Sessions whose stored token sum stopped at the read budget.
    ///
    /// Reported beside the rows rather than inside one, because it is a
    /// different KIND of shortfall: these sessions ARE measured and
    /// counted as such, and their figures are floors. Folding them into
    /// `unread` would say they contributed nothing, which is false.
    pub truncated_measurements: u64,
}

/// Count what the app has read, against what it holds.
///
/// Three cheap `COUNT`s over Headstate's own cache -- no transcript
/// body, no `stat`, nothing under `~/.claude`. That bound is the point:
/// a panel whose whole subject is the cost of reading the corpus must
/// not read the corpus to draw itself.
///
/// # Errors
///
/// Only when the database cannot be read, which is an `Err` and not a
/// report of zeros, for the reason
/// [`crate::claude::overview::aggregate`] gives: a struct of zeros here
/// would render "0 of 0 sessions" on every row, which looks exactly like
/// an empty corpus and is the absent-is-not-zero defect applied to the
/// panel that exists to prevent it.
pub fn report(conn: &Connection) -> Result<CoverageReport, rusqlite::Error> {
    let sessions: u64 = conn.query_row("SELECT COUNT(*) FROM claude_session", [], |r| {
        r.get::<_, i64>(0)
    })? as u64;

    // The usage importer's own aggregate, called rather than re-counted.
    // `usage_profile` already owns `sessions_measured` and
    // `sessions_truncated`, and a second SELECT over the same table here
    // would be a second source of truth for one number.
    let profile = crate::claude::store::usage_profile(conn)?;
    let cost_measured = profile.sessions_measured.min(sessions);

    // Sessions a hook ever observed, which is `overview.rs`'s
    // `never_observed` read from the other side. Counted from
    // `claude_run` because that is the table the hook writes; a session
    // with no run row is one nothing ever watched.
    let observed: u64 = conn.query_row(
        "SELECT COUNT(*) FROM claude_session s
          WHERE EXISTS (SELECT 1 FROM claude_run r WHERE r.session_id = s.session_id)",
        [],
        |r| r.get::<_, i64>(0),
    )? as u64;

    // A transcript path is what every body read needs. A row without one
    // cannot be read at all, and that is a boundary rather than a
    // failure: the session was recorded from a source that did not carry
    // the path.
    let with_path: u64 = conn.query_row(
        "SELECT COUNT(*) FROM claude_session
          WHERE transcript_path IS NOT NULL AND transcript_path <> ''",
        [],
        |r| r.get::<_, i64>(0),
    )? as u64;

    Ok(CoverageReport {
        sessions,
        truncated_measurements: profile.sessions_truncated,
        measurements: vec![
            Measurement {
                id: "cost".into(),
                label: "Token and cost totals".into(),
                unit: "session".into(),
                // The reason, phrased as a boundary. These sessions are
                // not broken and nothing retries them: the importer
                // measures what it has read, and the rest ran before it
                // did or carried no usage block to read.
                scope_note: "ran before the usage importer read them, or carried no usage block"
                    .into(),
                reach: Reach {
                    measured: cost_measured,
                    out_of_scope: sessions.saturating_sub(cost_measured),
                    // Nothing here is a failed read: a transcript the
                    // importer could not read leaves no row, and is
                    // indistinguishable in this table from one it never
                    // reached. Claiming an unread count we cannot
                    // establish would be the confident-wrong-number
                    // defect, so this is 0 and the out-of-scope note
                    // carries both cases.
                    unread: 0,
                },
            },
            Measurement {
                id: "hooks".into(),
                label: "Run and lifecycle records".into(),
                unit: "session".into(),
                scope_note: "ran before the Claude Code hooks were installed".into(),
                reach: Reach {
                    measured: observed,
                    out_of_scope: sessions.saturating_sub(observed),
                    unread: 0,
                },
            },
            Measurement {
                id: "transcript".into(),
                label: "Transcript bodies".into(),
                unit: "session".into(),
                scope_note: "were recorded without a transcript path to read".into(),
                reach: Reach {
                    measured: with_path,
                    out_of_scope: sessions.saturating_sub(with_path),
                    unread: 0,
                },
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn session(conn: &Connection, id: &str, path: Option<&str>) {
        conn.execute(
            "INSERT INTO claude_session (session_id, transcript_path, first_seen_at)
             VALUES (?1, ?2, '2026-09-01T00:00:00Z')",
            rusqlite::params![id, path],
        )
        .unwrap();
    }

    fn usage(conn: &Connection, id: &str, truncated: bool) {
        conn.execute(
            "INSERT INTO claude_session_usage
               (session_id, messages, input_tokens, output_tokens, cache_read,
                cache_creation, truncated, measured_at)
             VALUES (?1, 1, 1, 1, 0, 0, ?2, '2026-09-01T00:00:00Z')",
            rusqlite::params![id, truncated as i64],
        )
        .unwrap();
    }

    fn run(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO claude_run (session_id, pid, started_at)
             VALUES (?1, 1, '2026-09-01T00:00:00Z')",
            rusqlite::params![id],
        )
        .unwrap();
    }

    /// Every row's parts add up to the denominator it is read against.
    ///
    /// This is the invariant the whole panel rests on: a figure whose
    /// parts do not sum to `total()` cannot be rendered as "N of M"
    /// without one of the two numbers being wrong.
    ///
    /// Sabotage: making `out_of_scope` `sessions` rather than
    /// `sessions - measured` fails this on every row.
    #[test]
    fn every_reach_sums_to_the_denominator() {
        let conn = db();
        for i in 0..7 {
            session(&conn, &format!("s{i}"), Some("/tmp/t.jsonl"));
        }
        usage(&conn, "s0", false);
        usage(&conn, "s1", false);
        run(&conn, "s0");

        let out = report(&conn).unwrap();
        assert_eq!(out.sessions, 7);
        for m in &out.measurements {
            assert_eq!(m.reach.total(), out.sessions, "row {} lost rows", m.id);
        }
    }

    /// The three measurements read three different columns.
    ///
    /// They are not copies of one number wearing three labels: a corpus
    /// where cost, hooks and transcript paths cover different sessions
    /// must produce three different `measured` figures.
    ///
    /// Sabotage: pointing the hooks row at `cost_measured` makes the two
    /// equal and fails here.
    #[test]
    fn the_rows_measure_different_things() {
        let conn = db();
        for i in 0..5 {
            // Two sessions carry no transcript path at all.
            session(&conn, &format!("s{i}"), (i < 3).then_some("/tmp/t.jsonl"));
        }
        usage(&conn, "s0", false);
        run(&conn, "s0");
        run(&conn, "s1");
        run(&conn, "s2");
        run(&conn, "s3");

        let by = |id: &str| {
            report(&conn)
                .unwrap()
                .measurements
                .into_iter()
                .find(|m| m.id == id)
                .unwrap()
                .reach
        };
        assert_eq!(by("cost").measured, 1);
        assert_eq!(by("hooks").measured, 4);
        assert_eq!(by("transcript").measured, 3);
    }

    /// A truncated measurement is MEASURED, not unread.
    ///
    /// It contributed real figures that happen to be floors, so counting
    /// it as unread would say it contributed nothing. Reported beside
    /// the rows instead.
    ///
    /// Sabotage: adding `sessions_truncated` into the cost row's
    /// `unread` drops `measured` below the real count and fails both
    /// assertions.
    #[test]
    fn a_truncated_read_still_counts_as_measured() {
        let conn = db();
        session(&conn, "s0", Some("/tmp/t.jsonl"));
        session(&conn, "s1", Some("/tmp/t.jsonl"));
        usage(&conn, "s0", true);
        usage(&conn, "s1", false);

        let out = report(&conn).unwrap();
        let cost = &out.measurements[0].reach;
        assert_eq!(cost.measured, 2);
        assert_eq!(cost.unread, 0);
        assert_eq!(out.truncated_measurements, 1);
    }

    /// An empty corpus reports zero measured and zero out of scope.
    ///
    /// Not an error and not a row claiming coverage: `total()` is 0, so
    /// the view has a denominator of zero to render in words rather than
    /// a ratio to divide by it.
    #[test]
    fn an_empty_corpus_has_an_empty_denominator() {
        let out = report(&db()).unwrap();
        assert_eq!(out.sessions, 0);
        for m in &out.measurements {
            assert_eq!(m.reach.total(), 0);
            assert!(!m.reach.observed());
        }
    }

    /// Every row states why its uncovered sessions are uncovered.
    ///
    /// A bare count of uncovered rows is a defect list. The note is what
    /// makes it a statement of scope, so its absence is a test failure
    /// rather than a style note.
    ///
    /// Sabotage: emptying any `scope_note` fails here.
    #[test]
    fn every_row_carries_its_scope_note() {
        let out = report(&db()).unwrap();
        for m in &out.measurements {
            assert!(!m.scope_note.is_empty(), "row {} has no scope note", m.id);
            assert!(!m.unit.is_empty(), "row {} has no unit", m.id);
        }
    }

    /// The uncovered sessions are never described as damage.
    ///
    /// The words are the ticket. `overview.rs` insists the archived
    /// majority "must not be rendered as damage", and the same applies
    /// to every note here -- these sessions are outside what was being
    /// measured, not broken by it.
    ///
    /// Sabotage: rewording any note to "missing a usage block" fails
    /// here.
    #[test]
    fn no_scope_note_frames_absence_as_a_defect() {
        let out = report(&db()).unwrap();
        for m in &out.measurements {
            let note = m.scope_note.to_lowercase();
            for word in [
                "missing",
                "incomplete",
                "failed",
                "error",
                "broken",
                "invalid",
            ] {
                assert!(
                    !note.contains(word),
                    "row {} frames normal absence as damage: {note:?}",
                    m.id
                );
            }
        }
    }

    /// The report offers no rolled-up figure.
    ///
    /// Structural, not stylistic: `CoverageReport` has no total, no
    /// score and no percentage, and the rows are in a fixed order so the
    /// layout cannot rank them by badness either. A future field that
    /// combined unlike rows would have to defeat this test first.
    #[test]
    fn the_report_carries_no_grade() {
        let conn = db();
        session(&conn, "s0", Some("/tmp/t.jsonl"));
        usage(&conn, "s0", false);

        let out = report(&conn).unwrap();
        let json = serde_json::to_string(&out).unwrap();
        for banned in ["score", "grade", "percent", "ratio", "health", "severity"] {
            assert!(!json.contains(banned), "report carries a {banned}");
        }
        // The fixed order is part of the contract: not sorted by how
        // much is uncovered, which would be a grade as a layout.
        let ids: Vec<_> = out.measurements.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["cost", "hooks", "transcript"]);
    }
}
