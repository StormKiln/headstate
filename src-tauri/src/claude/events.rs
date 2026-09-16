//! What the hook recorded about failures and denials (#1062, #1063,
//! #1064, epic #1060).
//!
//! Three events, one table, and one question each:
//!
//! | event | the question | fields |
//! |---|---|---|
//! | `StopFailure` | why did the turn die? | `error_type`, `error_message` |
//! | `PostToolUseFailure` | which tools fail, and how? | `tool_name`, `error_message` |
//! | `PermissionDenied` | what is auto mode blocking? | `tool_name`, `denial_reason` |
//!
//! None of them can be answered from the transcript at a price worth
//! paying. The reader is windowed at 40 head records plus a 16 KB tail
//! (`transcript.rs:151,163`), so a failure in the body of a 73 MB
//! transcript is outside it, and the full-body alternative is a 1.8 GB
//! scan that grows without bound. A denial is worse than expensive: it is
//! a thing that DID NOT HAPPEN, so there is no tool result to parse at any
//! price.
//!
//! # Absent is not zero, and here it is the whole design
//!
//! This is the rule the root `CLAUDE.md` records as having shipped as a
//! real defect (#846), and these three events are unusually exposed to it.
//! A session with no `StopFailure` rows may have had no failures, OR may
//! have run before the hook was installed -- and on any machine that
//! adopted Headstate after using Claude Code, which is the normal case,
//! it is overwhelmingly the second. `overview.rs` measured the same thing
//! one table over: every one of 1,461 sessions had zero runs, because
//! none of them was ever observed.
//!
//! "0 failures" on such a session is the most legible possible lie. It
//! invites the reader to conclude the session was clean when the truth is
//! that nobody was watching.
//!
//! So this module never returns a bare count. [`Observation`] is a
//! three-state answer -- [`Observation::Unobserved`],
//! [`Observation::Partial`], [`Observation::Observed`] -- modelled on the
//! half-install state `install::Status` already carries, and the count
//! only exists inside the arm that earned it.
//!
//! # How "was this session observed" is decided
//!
//! Not from these events, and that is the point: asking "did it record a
//! failure" to decide "were we watching" is circular, and answers
//! `Unobserved` for every session that simply did not fail.
//!
//! It is decided from `claude_run`, which the session-bounding hooks
//! write. A session with a run was observed by a hook; one without never
//! was. That is an independent witness, and it is why this module reads
//! two tables rather than one.
//!
//! The remaining gap is honest and named: a session observed while only
//! SOME of the five events were installed -- a half install, or an upgrade
//! from a Headstate that predates #1062 -- is `Partial`. Its counts are a
//! floor rather than a total, and the UI says so rather than presenting
//! them as complete.
//!
//! # Liveness must never read this (#1062)
//!
//! `install.rs` has recorded since #910 that `StopFailure` "does not fire
//! on SIGKILL, so it adds error context rather than liveness", and #1062
//! carries that constraint forward explicitly. A session that hit a rate
//! limit is a session that is very much still running.
//!
//! Nothing here is reachable from `liveness::derive`, and that is
//! structural rather than remembered: `derive` takes `(probe, registry,
//! session_id, runs)` and `runs` comes from `claude_run`, which has no
//! column for any of this. `invariants.rs`'s
//! `liveness_never_reads_the_failure_events` guards the boundary against a
//! later well-meaning join.
//!
//! # A denial is not an error (#1064)
//!
//! `PermissionDenied` fires when auto mode refuses a tool call. That is a
//! guardrail doing its job, not damage, and the vocabulary here keeps the
//! two apart: [`Profile::denials`] is counted and rendered separately from
//! [`Profile::failures`], and nothing in this module calls a denial a
//! failure. The distinction matters because the remedy differs -- a
//! recurring denial is either a guardrail working exactly as intended or a
//! workflow being silently blocked, and only the user can tell which.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// The events this module reads, and the only ones it will store a count
/// for.
///
/// Spelled out rather than "whatever is in the table" so that a record
/// from an event this version does not understand cannot silently join a
/// total. It matches `handoff::EVENT_KINDS`, which is asserted by
/// [`tests::the_read_and_write_sides_agree_about_the_events`].
pub const KINDS: &[&str] = &["StopFailure", "PostToolUseFailure", "PermissionDenied"];

/// How much of a session's failure history we are in a position to state.
///
/// Three states, because two would lie -- the same shape and the same
/// reason as `install::Status`, which models a half install rather than
/// assuming it away.
///
/// The count lives INSIDE [`Observation::Observed`] rather than beside
/// this enum, so that it is unrepresentable to hold a number for a session
/// nobody watched. A caller cannot accidentally render an `Unobserved`
/// session as zero, because there is no zero there to render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Observation {
    /// No hook ever observed this session, so nothing can be said about
    /// what went wrong in it.
    ///
    /// The normal state for history that predates the install, which on a
    /// machine that adopted Headstate after using Claude Code is almost
    /// all of it. NOT a failure and NOT zero: the remedy is to install the
    /// hooks, and the rendering is "not recorded".
    Unobserved,
    /// The session was observed, but not by every event in [`KINDS`].
    ///
    /// A half install (`install::Status::Stale`), or a session that ran
    /// under a Headstate older than #1062. The counts that follow are a
    /// FLOOR, not a total, and `missing` names what was not being
    /// recorded so the reader can tell which figure is short.
    Partial {
        /// The events of [`KINDS`] that were not installed, sorted.
        missing: Vec<String>,
        /// What was counted anyway. Partial is not nothing: the root
        /// `CLAUDE.md` rule that a bounded operation's partial result is
        /// still an answer applies here exactly as it does to a
        /// budget-exhausted stats board.
        profile: Profile,
    },
    /// The session was observed by every event in [`KINDS`], so the
    /// profile is a total.
    ///
    /// A zero here is a real, measured zero and may be rendered as one.
    Observed {
        /// What happened. May legitimately be empty.
        profile: Profile,
    },
}

impl Observation {
    /// The profile, when there is one to show.
    ///
    /// `None` for [`Observation::Unobserved`], which is the case with no
    /// number attached. A caller that wants to render a figure has to pass
    /// through here and therefore has to handle the absent case.
    pub fn profile(&self) -> Option<&Profile> {
        match self {
            Observation::Unobserved => None,
            Observation::Partial { profile, .. } | Observation::Observed { profile } => {
                Some(profile)
            }
        }
    }

    /// Whether any figure here is a floor rather than a total.
    pub fn is_floor(&self) -> bool {
        matches!(self, Observation::Partial { .. })
    }
}

/// One named thing that happened, with how often.
///
/// The name is carried VERBATIM from the payload and is never mapped to a
/// known set. #1062 and #1063 both require it: an `error_type` Headstate
/// has never seen renders as itself, and a tool name it has never seen
/// renders as itself. Folding an unrecognised value into "other" would
/// destroy the one piece of information the record exists to carry, and a
/// new Claude Code release adding an error type would silently start
/// hiding it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// The `error_type` or `tool_name`, exactly as recorded.
    ///
    /// `None` when the payload carried none. Distinct from the empty
    /// string, and rendered as "not recorded" rather than as a blank row
    /// -- absent is not zero applies to a label as much as to a count.
    pub name: Option<String>,
    /// How many records carried it.
    pub count: usize,
    /// The most recent free-text detail seen for this name, if any.
    ///
    /// ONE example rather than every message: the question is "what kind
    /// of thing is going wrong", and forty copies of the same truncated
    /// string answers it no better than one while costing forty times the
    /// wire. Capped at [`super::hook::TEXT_FIELD_CAP`] by the writer.
    ///
    /// Untrusted vendor text. It is display-only, it is never logged, and
    /// it must never be interpolated anywhere a committed file could pick
    /// it up -- the repo privacy guard scans for real logins and repo
    /// names, and an error message can contain both.
    pub detail: Option<String>,
}

/// What went wrong in a session, or across all of them.
///
/// Failures and denials are counted APART and never summed (#1064). A
/// denial is a guardrail working; a failure is something that broke. One
/// number covering both would answer neither question and would present
/// the guardrail as damage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// `StopFailure` by `error_type` -- why turns died. Commonest first.
    ///
    /// `rate_limit` recurring is a different problem from `overloaded`
    /// recurring, and the user can act on the first, so the breakdown
    /// matters more than the total (#1062).
    pub turn_failures: Vec<Tally>,
    /// `PostToolUseFailure` by `tool_name`. Commonest first.
    ///
    /// A session where `Bash` failed 14 times is a session that was
    /// fighting something (#1063).
    pub tool_failures: Vec<Tally>,
    /// `PermissionDenied` by `tool_name`. Commonest first.
    ///
    /// NOT a failure list. See the module docs: one denial is noise, the
    /// same denial forty times is a finding, and neither is an error.
    pub denials: Vec<Tally>,
}

impl Profile {
    /// Turn failures plus tool failures. Denials are NOT included.
    ///
    /// The exclusion is the point and is required by #1064: a denial is a
    /// guardrail working, and adding it to a failure total would report a
    /// well-defended session as a broken one.
    pub fn failures(&self) -> usize {
        total(&self.turn_failures) + total(&self.tool_failures)
    }

    /// How many tool calls auto mode refused.
    pub fn denials(&self) -> usize {
        total(&self.denials)
    }

    /// Whether anything at all was recorded.
    pub fn is_empty(&self) -> bool {
        self.turn_failures.is_empty() && self.tool_failures.is_empty() && self.denials.is_empty()
    }

    /// The tool that failed most, when one clearly dominates.
    ///
    /// #1063 asks for "a quiet signal when failures are concentrated in
    /// one tool, which is the shape worth looking at". Concentration is
    /// the signal, not volume: three failures spread over three tools is
    /// noise, and three in one tool is a thing to go and look at.
    ///
    /// `None` unless one tool holds a strict majority of the failures AND
    /// there is more than one failure. A strict majority rather than a
    /// plurality because a 2/2/1 split has no story, and requiring more
    /// than one failure because a single failure is trivially 100%
    /// concentrated and pointing at it would fire the signal constantly.
    pub fn concentrated_tool(&self) -> Option<&Tally> {
        let total = total(&self.tool_failures);
        if total < 2 {
            return None;
        }
        self.tool_failures
            .iter()
            .find(|t| t.count * 2 > total && t.name.is_some())
    }
}

fn total(t: &[Tally]) -> usize {
    t.iter().map(|x| x.count).sum()
}

/// Read one session's failure and denial profile (#1062, #1063, #1064).
///
/// `installed` is the set of events currently installed, from
/// `install::status`. It is a PARAMETER rather than read here because this
/// function must be testable without a settings file, and because the
/// caller has already read it for the page's install banner -- reading it
/// again per session would mean one settings parse per row.
///
/// # Cost
///
/// One indexed `SELECT` over `claude_hook_event` for the session plus one
/// `EXISTS` over `claude_run`. Both hit an index created by their
/// migrations, and neither reads a transcript -- the hook recorded this in
/// O(1) as it happened, which is the entire reason the events exist.
pub fn for_session(
    conn: &Connection,
    session_id: &str,
    installed: &[String],
) -> Result<Observation, rusqlite::Error> {
    // The independent witness. See the module docs on why observation is
    // decided from `claude_run` and not from the events themselves: asking
    // the events would answer "unobserved" for every session that simply
    // did not fail.
    let observed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM claude_run WHERE session_id = ?1)",
        [session_id],
        |r| r.get(0),
    )?;

    // `COALESCE(error_type, tool_name)` is the SUBJECT: `StopFailure`
    // fills the first and the two tool events fill the second, and no
    // event fills both -- `Record::failure_fields` owns that partition.
    // Grouping on the coalesced value is what lets one query serve all
    // three without the caller knowing which column each event uses.
    let mut stmt = conn.prepare(
        "SELECT event, COALESCE(error_type, tool_name) AS subject,
                MAX(failure_detail) AS detail, COUNT(*) AS n
           FROM claude_hook_event
          WHERE session_id = ?1 AND event IN (?2, ?3, ?4)
          GROUP BY event, subject
          ORDER BY n DESC, subject",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![session_id, KINDS[0], KINDS[1], KINDS[2]],
            row_to_group,
        )?
        .collect::<Result<Vec<_>, _>>()?;

    // A session with records but no run is still observed -- the events
    // ARE the observation in that case. This can happen when the hook was
    // installed mid-session, so the start record was never written.
    // Treating it as unobserved would discard records we demonstrably have.
    if !observed && rows.is_empty() {
        return Ok(Observation::Unobserved);
    }

    Ok(classify(assemble(rows), installed))
}

/// The profile across every stored session (#1062-#1064).
///
/// The more informative view for denials especially: #1064 puts it
/// plainly, one denial is noise and the same denial forty times is a
/// finding, and only the cross-session view can tell them apart.
///
/// `sessions_observed` is reported alongside so the reader can see the
/// DENOMINATOR. A profile over 3 observed sessions out of 1,461 stored is
/// a very different statement from the same profile over all of them, and
/// without the denominator both render identically.
pub fn across_sessions(conn: &Connection, installed: &[String]) -> Result<Corpus, rusqlite::Error> {
    // `i64` then widened: `COUNT(*)` is SQLite's own integer type and
    // `usize` does not implement `FromSql`. Clamped at zero rather than
    // cast blind, so a negative could never wrap into an enormous count.
    let count = |sql: &str| -> Result<usize, rusqlite::Error> {
        let n: i64 = conn.query_row(sql, [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    };

    let sessions = count("SELECT COUNT(*) FROM claude_session")?;
    // Sessions a hook observed, which is the population the profile can
    // possibly describe. Everything else predates the install.
    let observed = count("SELECT COUNT(DISTINCT session_id) FROM claude_run")?;
    // Sessions that actually recorded one of these events. A small number
    // here against a large `observed` is the good news case -- most
    // sessions had nothing go wrong -- and it must not read as a failure
    // to measure.
    // Scoped to OUR three events. `claude_hook_event` also holds
    // #1065-#1067's compaction, subagent and notification rows, and a
    // session that merely compacted has recorded nothing about failures
    // -- counting it here would inflate the "recorded something"
    // numerator with sessions this card has nothing to say about.
    let with_events = {
        let mut stmt = conn.prepare(
            "SELECT COUNT(DISTINCT session_id) FROM claude_hook_event
              WHERE event IN (?1, ?2, ?3)",
        )?;
        let n: i64 = stmt.query_row(rusqlite::params![KINDS[0], KINDS[1], KINDS[2]], |r| {
            r.get(0)
        })?;
        n.max(0) as usize
    };

    let mut stmt = conn.prepare(
        "SELECT event, COALESCE(error_type, tool_name) AS subject,
                MAX(failure_detail) AS detail, COUNT(*) AS n
           FROM claude_hook_event
          WHERE event IN (?1, ?2, ?3)
          GROUP BY event, subject
          ORDER BY n DESC, subject",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![KINDS[0], KINDS[1], KINDS[2]],
            row_to_group,
        )?
        .collect::<Result<Vec<_>, _>>()?;

    let observation = if observed == 0 && rows.is_empty() {
        Observation::Unobserved
    } else {
        classify(assemble(rows), installed)
    };

    Ok(Corpus {
        observation,
        sessions,
        sessions_observed: observed,
        sessions_with_events: with_events,
    })
}

/// The cross-session profile, with the denominators that make it readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Corpus {
    /// What was recorded, and how much of it can be trusted as a total.
    pub observation: Observation,
    /// Every stored session, as the denominator.
    pub sessions: usize,
    /// Of those, how many a hook ever observed. The rest predate the
    /// install and can contribute nothing -- reporting the profile without
    /// this figure would invite reading it as covering everything.
    pub sessions_observed: usize,
    /// Of the observed, how many recorded at least one of these events.
    ///
    /// A SMALL number here is the good news, and the UI must say so: it
    /// means most observed sessions had nothing go wrong. Presented
    /// without that framing it reads as a coverage problem.
    pub sessions_with_events: usize,
}

/// One `GROUP BY` row, before it is sorted into a profile.
struct Group {
    event: String,
    subject: Option<String>,
    detail: Option<String>,
    count: usize,
}

fn row_to_group(r: &rusqlite::Row<'_>) -> Result<Group, rusqlite::Error> {
    Ok(Group {
        event: r.get(0)?,
        subject: r.get(1)?,
        detail: r.get(2)?,
        count: r.get::<_, i64>(3)?.max(0) as usize,
    })
}

/// Sort the grouped rows into the three lists.
///
/// An event not in [`KINDS`] is DROPPED rather than counted anywhere. A
/// row can only be there because a future version wrote it, and silently
/// adding it to a total would make a figure this version cannot explain.
fn assemble(rows: Vec<Group>) -> Profile {
    let mut p = Profile::default();
    for g in rows {
        let tally = Tally {
            name: g.subject,
            count: g.count,
            detail: g.detail,
        };
        match g.event.as_str() {
            "StopFailure" => p.turn_failures.push(tally),
            "PostToolUseFailure" => p.tool_failures.push(tally),
            "PermissionDenied" => p.denials.push(tally),
            _ => {}
        }
    }
    p
}

/// Decide whether a profile is a total or a floor.
///
/// The answer turns on which of [`KINDS`] are installed RIGHT NOW, which
/// is the honest question: a count is complete only if every event that
/// could contribute to it was being recorded.
fn classify(profile: Profile, installed: &[String]) -> Observation {
    let mut missing: Vec<String> = KINDS
        .iter()
        .filter(|k| !installed.iter().any(|i| i == *k))
        .map(|k| (*k).to_string())
        .collect();
    if missing.is_empty() {
        Observation::Observed { profile }
    } else {
        missing.sort();
        Observation::Partial { missing, profile }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    /// Every event installed, which is the ordinary state after #1062.
    fn all() -> Vec<String> {
        KINDS.iter().map(|k| (*k).to_string()).collect()
    }

    fn session(conn: &Connection, sid: &str) {
        conn.execute(
            "INSERT INTO claude_session (session_id, first_seen_at)
             VALUES (?1, '2026-09-15T10:00:00Z')",
            [sid],
        )
        .unwrap();
    }

    /// A run, which is what makes a session OBSERVED.
    fn run(conn: &Connection, sid: &str) {
        conn.execute(
            "INSERT INTO claude_run (session_id, pid, started_at)
             VALUES (?1, 4242, '2026-09-15T10:00:00Z')",
            [sid],
        )
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn event(
        conn: &Connection,
        sid: &str,
        ev: &str,
        subject: Option<&str>,
        detail: Option<&str>,
        tool_use_id: &str,
        ts: &str,
    ) {
        // Routed to the same column the writer uses: `StopFailure`'s
        // subject is an `error_type` and the two tool events' is a
        // `tool_name`. A fixture that put everything in one column would
        // pass while the production `COALESCE` read the other.
        let (error_type, tool_name) = if ev == "StopFailure" {
            (subject, None)
        } else {
            (None, subject)
        };
        conn.execute(
            "INSERT OR IGNORE INTO claude_hook_event
                (session_id, event, at, error_type, tool_name, failure_detail,
                 tool_use_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                sid,
                ev,
                ts,
                error_type,
                tool_name,
                detail,
                // NULL, not "", so the partial unique index does not
                // collapse two distinct id-less records.
                if tool_use_id.is_empty() {
                    None
                } else {
                    Some(tool_use_id)
                },
            ],
        )
        .unwrap();
    }

    // -----------------------------------------------------------------
    // Absent is not zero. The rule all three issues inherit, and the one
    // that has already shipped as a defect in this repo (#846).
    // -----------------------------------------------------------------

    /// A session from before the hook says "not recorded", NEVER
    /// "0 failures" (#1062).
    ///
    /// This is the single most important test in this file. On a machine
    /// that adopted Headstate after using Claude Code, EVERY historical
    /// session is in this state -- `overview.rs` measured 1,461 of 1,461
    /// with no runs at all -- so a bare zero here would be a confidently
    /// wrong answer on the whole corpus.
    ///
    /// PROVEN BY SABOTAGE: making `for_session` return
    /// `Observation::Observed { profile }` unconditionally (dropping the
    /// `observed` check) fails here, because the session then reports a
    /// real measured zero instead of `Unobserved`.
    #[test]
    fn a_session_from_before_the_hook_is_unobserved_rather_than_zero() {
        let conn = db();
        session(&conn, "old");
        // No run: no hook ever saw this session.

        let got = for_session(&conn, "old", &all()).unwrap();

        assert_eq!(got, Observation::Unobserved);
        // And there is NO number to render. The count lives inside the
        // `Observed` arm precisely so this is unrepresentable.
        assert!(
            got.profile().is_none(),
            "an unobserved session must not carry a profile -- a zero here \
             would say the session was clean when the truth is nobody was \
             watching"
        );
    }

    /// An observed session that simply had no failures reports a real,
    /// measured zero -- which is a different answer from the one above.
    ///
    /// Both halves matter. If everything rendered as "not recorded" the
    /// feature would never be able to give good news, and a user whose
    /// sessions are genuinely clean would be told forever that nothing was
    /// measured.
    #[test]
    fn an_observed_session_with_no_failures_is_a_measured_zero() {
        let conn = db();
        session(&conn, "clean");
        run(&conn, "clean");

        let got = for_session(&conn, "clean", &all()).unwrap();

        let Observation::Observed { profile } = &got else {
            panic!("an observed session must report a measured answer: {got:?}");
        };
        assert!(profile.is_empty());
        assert_eq!(profile.failures(), 0);
        assert_eq!(profile.denials(), 0);
    }

    /// The two states are not equal, which is the whole claim.
    ///
    /// Stated as its own assertion because it is what a UI branches on,
    /// and because the two tests above would both pass if the type
    /// collapsed them.
    #[test]
    fn unobserved_and_measured_zero_are_different_answers() {
        let conn = db();
        session(&conn, "old");
        session(&conn, "clean");
        run(&conn, "clean");

        let old = for_session(&conn, "old", &all()).unwrap();
        let clean = for_session(&conn, "clean", &all()).unwrap();

        assert_ne!(
            old, clean,
            "a session nobody watched and a session that was watched and \
             was clean must not render the same"
        );
    }

    /// A half install makes the counts a FLOOR, and says which event is
    /// missing (#1064's "absent is not zero" test).
    ///
    /// Modelled on `install::Status::Stale`, which already names the
    /// missing event rather than reporting a bare "needs reinstalling".
    #[test]
    fn a_half_install_reports_a_floor_and_names_what_is_missing() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("Bash"),
            Some("exited 1"),
            "t1",
            "2026-09-15T10:00:00Z",
        );

        // `PermissionDenied` was never installed, so a denial count of
        // zero would be a claim we are not entitled to make.
        let installed = vec!["StopFailure".to_string(), "PostToolUseFailure".to_string()];
        let got = for_session(&conn, "s", &installed).unwrap();

        let Observation::Partial { missing, profile } = &got else {
            panic!("a half install must report a floor: {got:?}");
        };
        assert_eq!(missing, &["PermissionDenied".to_string()]);
        assert!(got.is_floor());
        // Partial is not nothing: what WAS recorded is still reported.
        assert_eq!(profile.tool_failures.len(), 1);
    }

    // -----------------------------------------------------------------
    // Unknown values render as themselves (#1062, #1063).
    // -----------------------------------------------------------------

    /// An `error_type` Headstate has never seen is carried verbatim, not
    /// folded into "other" (#1062's first test).
    ///
    /// A new Claude Code release can add an error type at any time. Mapping
    /// the unrecognised ones to a bucket would mean the first user to hit a
    /// new failure mode sees the least about it, which inverts what the
    /// feature is for.
    ///
    /// PROVEN BY SABOTAGE: adding a `match` in `assemble` that rewrites any
    /// name outside a known set to `Some("other")` fails here on the exact
    /// string.
    #[test]
    fn an_unknown_error_type_renders_as_itself() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        event(
            &conn,
            "s",
            "StopFailure",
            Some("quantum_decoherence"),
            Some("the turn collapsed"),
            "",
            "2026-09-15T10:00:00Z",
        );

        let got = for_session(&conn, "s", &all()).unwrap();

        let p = got.profile().unwrap();
        assert_eq!(p.turn_failures.len(), 1);
        assert_eq!(
            p.turn_failures[0].name.as_deref(),
            Some("quantum_decoherence"),
            "an error type this version has never seen must survive \
             verbatim -- 'other' would destroy the only thing the record \
             carries"
        );
    }

    /// A tool name Headstate has never seen renders as itself (#1063's
    /// first test).
    ///
    /// MCP servers define arbitrary tool names, so the unknown case is the
    /// COMMON one rather than an edge: a user with three MCP servers has a
    /// dozen tool names this binary has never heard of.
    #[test]
    fn an_unknown_tool_name_renders_as_itself() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("mcp__acme_widgets__reticulate"),
            Some("connection refused"),
            "t1",
            "2026-09-15T10:00:00Z",
        );

        let got = for_session(&conn, "s", &all()).unwrap();

        assert_eq!(
            got.profile().unwrap().tool_failures[0].name.as_deref(),
            Some("mcp__acme_widgets__reticulate")
        );
    }

    // -----------------------------------------------------------------
    // #1063: counts do not double-count retries.
    // -----------------------------------------------------------------

    /// Two records for the SAME `tool_use_id` count once; a different one
    /// counts again.
    ///
    /// Both halves, because a dedup that swallowed genuinely distinct
    /// failures would pass the first assertion and destroy the feature.
    ///
    /// PROVEN BY SABOTAGE: dropping `tool_use_id` from migration 14's
    /// primary key makes the redelivered record a second row and this
    /// fails at 3 instead of 2.
    #[test]
    fn a_retried_tool_call_is_not_counted_twice() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        // The same tool call, failing twice at DIFFERENT instants -- a
        // retry, which is what #1063 names. Distinct timestamps on
        // purpose: migration 14's (session_id, event, at) key cannot
        // collapse these, so what collapses them is migration 15's
        // partial unique index on `tool_use_id`, which is the thing
        // under test. A fixture reusing one instant would pass on the
        // wrong mechanism.
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("Bash"),
            Some("exited 1"),
            "toolu_01",
            "2026-09-15T10:00:00Z",
        );
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("Bash"),
            Some("exited 1"),
            "toolu_01",
            "2026-09-15T10:00:05Z",
        );
        // A genuinely different failure of the same tool.
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("Bash"),
            Some("exited 2"),
            "toolu_02",
            "2026-09-15T10:00:01Z",
        );

        let got = for_session(&conn, "s", &all()).unwrap();

        let p = got.profile().unwrap();
        assert_eq!(p.tool_failures.len(), 1, "one tool, so one row");
        assert_eq!(
            p.tool_failures[0].count, 2,
            "a redelivered record must not inflate the count, and two \
             distinct failures must still count twice"
        );
    }

    // -----------------------------------------------------------------
    // #1064: a denial is a guardrail, not an error.
    // -----------------------------------------------------------------

    /// A denial with an empty `denial_reason` SAYS so rather than
    /// inventing one (#1064's first test).
    ///
    /// The absence has to survive storage as an absence, which is why the
    /// column is nullable. Inventing a plausible reason would be worse
    /// than showing none: the user would act on a sentence Headstate made
    /// up.
    #[test]
    fn a_denial_with_no_reason_says_so_rather_than_inventing_one() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        event(
            &conn,
            "s",
            "PermissionDenied",
            Some("Write"),
            None,
            "toolu_01",
            "2026-09-15T10:00:00Z",
        );

        let got = for_session(&conn, "s", &all()).unwrap();

        let p = got.profile().unwrap();
        assert_eq!(p.denials.len(), 1);
        assert_eq!(p.denials[0].name.as_deref(), Some("Write"));
        assert_eq!(
            p.denials[0].detail, None,
            "no reason was recorded, and None is how that is said -- a \
             default sentence here would be Headstate inventing a finding"
        );
    }

    /// A denial is NEVER counted as a failure (#1064).
    ///
    /// The vocabulary rule as an assertion. A denial is auto mode doing
    /// its job, and rolling it into a failure total would report a
    /// well-defended session as a broken one.
    ///
    /// PROVEN BY SABOTAGE: adding `+ total(&self.denials)` to
    /// `Profile::failures` fails here at 1 != 0.
    #[test]
    fn a_denial_is_not_counted_as_a_failure() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        event(
            &conn,
            "s",
            "PermissionDenied",
            Some("Bash"),
            Some("auto mode refuses writes outside the worktree"),
            "toolu_01",
            "2026-09-15T10:00:00Z",
        );

        let p = for_session(&conn, "s", &all()).unwrap();
        let p = p.profile().unwrap();

        assert_eq!(p.denials(), 1);
        assert_eq!(
            p.failures(),
            0,
            "a denial is a guardrail working -- counting it as a failure \
             says something went wrong when nothing did"
        );
    }

    // -----------------------------------------------------------------
    // #1063: the concentration signal.
    // -----------------------------------------------------------------

    /// Failures concentrated in ONE tool are the shape worth surfacing.
    #[test]
    fn failures_concentrated_in_one_tool_are_flagged() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        // One instant per record, per migration 14's key.
        for i in 0..5 {
            event(
                &conn,
                "s",
                "PostToolUseFailure",
                Some("Bash"),
                Some("exited 1"),
                &format!("t{i}"),
                &format!("2026-09-15T10:00:0{i}Z"),
            );
        }
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("Read"),
            Some("no such file"),
            "tr",
            "2026-09-15T10:00:09Z",
        );

        let got = for_session(&conn, "s", &all()).unwrap();
        let concentrated = got.profile().unwrap().concentrated_tool().unwrap();

        assert_eq!(concentrated.name.as_deref(), Some("Bash"));
        assert_eq!(concentrated.count, 5);
    }

    /// Failures spread evenly are NOT flagged.
    ///
    /// The negative direction, which is what stops the signal from firing
    /// constantly and being ignored. A signal that is always on is not a
    /// signal.
    #[test]
    fn failures_spread_across_tools_are_not_flagged() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        for (i, tool) in ["Bash", "Read", "Edit"].iter().enumerate() {
            event(
                &conn,
                "s",
                "PostToolUseFailure",
                Some(tool),
                Some("failed"),
                &format!("t{i}"),
                &format!("2026-09-15T10:00:0{i}Z"),
            );
        }

        let got = for_session(&conn, "s", &all()).unwrap();

        assert!(
            got.profile().unwrap().concentrated_tool().is_none(),
            "three failures over three tools is noise, not a finding"
        );
    }

    /// A SINGLE failure is not "concentrated".
    ///
    /// One failure is trivially 100% in one tool, so a naive majority test
    /// fires on every session that ever had one thing go wrong.
    #[test]
    fn a_single_failure_is_not_a_concentration() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("Bash"),
            Some("exited 1"),
            "t1",
            "2026-09-15T10:00:00Z",
        );

        let got = for_session(&conn, "s", &all()).unwrap();

        assert!(
            got.profile().unwrap().concentrated_tool().is_none(),
            "one failure is trivially 100% concentrated; firing here would \
             make the signal constant and therefore useless"
        );
    }

    // -----------------------------------------------------------------
    // The cross-session view.
    // -----------------------------------------------------------------

    /// The corpus view carries the DENOMINATORS, not just the profile.
    ///
    /// A profile over 2 observed sessions out of 1,461 stored is a very
    /// different statement from the same profile over all of them, and
    /// without the denominator both render identically. This is
    /// `overview.rs`'s `never_observed` rule one table over.
    #[test]
    fn the_corpus_view_reports_how_many_sessions_it_could_speak_for() {
        let conn = db();
        for sid in ["a", "b", "c"] {
            session(&conn, sid);
        }
        // Only two were ever observed.
        run(&conn, "a");
        run(&conn, "b");
        event(
            &conn,
            "a",
            "PermissionDenied",
            Some("Bash"),
            Some("outside the worktree"),
            "t1",
            "2026-09-15T10:00:00Z",
        );

        let got = across_sessions(&conn, &all()).unwrap();

        assert_eq!(got.sessions, 3);
        assert_eq!(
            got.sessions_observed, 2,
            "the third session predates the hook and can contribute nothing"
        );
        assert_eq!(
            got.sessions_with_events, 1,
            "a small number here is the GOOD news -- most observed sessions \
             had nothing go wrong"
        );
        assert_eq!(got.observation.profile().unwrap().denials(), 1);
    }

    /// A corpus nothing has observed is `Unobserved`, not a page of zeros.
    ///
    /// This is the state a machine is in the moment before the hooks are
    /// installed, and `overview.rs` measured it as the state of all 1,461
    /// sessions on the development machine. A grid of zeros here would be
    /// #846 exactly.
    #[test]
    fn a_corpus_with_no_observed_sessions_is_unobserved() {
        let conn = db();
        for sid in ["a", "b"] {
            session(&conn, sid);
        }

        let got = across_sessions(&conn, &all()).unwrap();

        assert_eq!(got.observation, Observation::Unobserved);
        assert_eq!(got.sessions, 2);
        assert_eq!(got.sessions_observed, 0);
        assert!(got.observation.profile().is_none());
    }

    /// Commonest first, which is the order a reader needs.
    #[test]
    fn tallies_are_ordered_commonest_first() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        // DISTINCT timestamps, one per record. Migration 14 keys on
        // (session_id, event, at), so a fixture that fired four records
        // at one instant would store one -- and real records cannot,
        // since each is its own hook process at its own instant.
        event(
            &conn,
            "s",
            "PostToolUseFailure",
            Some("Read"),
            Some("no such file"),
            "r1",
            "2026-09-15T10:00:00Z",
        );
        for i in 0..3 {
            event(
                &conn,
                "s",
                "PostToolUseFailure",
                Some("Bash"),
                Some("exited 1"),
                &format!("b{i}"),
                &format!("2026-09-15T10:00:0{}Z", i + 1),
            );
        }

        let got = for_session(&conn, "s", &all()).unwrap();
        let p = got.profile().unwrap();

        assert_eq!(p.tool_failures[0].name.as_deref(), Some("Bash"));
        assert_eq!(p.tool_failures[0].count, 3);
        assert_eq!(p.tool_failures[1].name.as_deref(), Some("Read"));
    }

    /// An event this version does not know is dropped, not counted.
    ///
    /// A row can only be there because a NEWER Headstate wrote it. Adding
    /// it to a total would produce a figure this version cannot explain
    /// and cannot break down -- the same reasoning `parse_line` uses to
    /// count an unknown `v` apart rather than folding it into "malformed".
    #[test]
    fn an_event_this_version_does_not_know_is_not_counted() {
        let conn = db();
        session(&conn, "s");
        run(&conn, "s");
        event(
            &conn,
            "s",
            "SomeFutureEvent",
            Some("whatever"),
            None,
            "t1",
            "2026-09-15T10:00:00Z",
        );

        let got = for_session(&conn, "s", &all()).unwrap();
        let p = got.profile().unwrap();

        assert!(p.is_empty(), "an unknown event must not join a total");
        assert_eq!(p.failures(), 0);
        assert_eq!(p.denials(), 0);
    }

    /// The read side and the write side agree about which events exist.
    ///
    /// Two lists of the same three strings in two modules is a drift risk,
    /// and the drift is silent in the worst direction: the writer would
    /// store an event the reader never counts, so the figure would be
    /// quietly short rather than wrong-looking.
    #[test]
    fn the_read_and_write_sides_agree_about_the_events() {
        let write = include_str!("handoff.rs");
        for kind in KINDS {
            assert!(
                write.contains(&format!("\"{kind}\"")),
                "{kind} is read here but `handoff.rs` does not divert it \
                 into `claude_hook_event` -- `Record::point_event` is the \
                 named list that decides, and an event missing from it \
                 falls through to the run path"
            );
        }
    }

    /// Every event this module reads is one the app actually installs.
    ///
    /// The other direction of the same drift: a reader for an event no
    /// hook is installed for would report a permanent, unexplained zero.
    #[test]
    fn every_kind_read_here_is_installed() {
        for kind in KINDS {
            assert!(
                super::super::install::EVENTS.contains(kind),
                "{kind} is read here but is not in install::EVENTS, so it \
                 would report a permanent zero nobody could explain"
            );
        }
    }
}
