//! What the non-boundary hook events say about a session (#1065, #1066,
//! #1067, epic #1060).
//!
//! `SessionStart` and `SessionEnd` bound a run and live in `claude_run`.
//! The three events #1060's sub-issues 4-6 install do not bound
//! anything: a compaction, a subagent spawn and a notification are
//! points in time. They land in `claude_hook_event` (migration 14), and
//! this module is what reads them back.
//!
//! Three questions, one table, one pass:
//!
//! - **How hard is this session pushing context?** [`Compactions`],
//!   counting `PreCompact` and splitting `manual` from `auto`.
//! - **What did it spawn?** [`AgentTypes`], the `agent_type` a
//!   `SubagentStart` STATED, against the one `subagent.rs` infers from
//!   the directory layout.
//! - **Is it waiting for me right now?** [`Waiting`], which is the one
//!   with a hard problem in it.
//!
//! # Absent is not zero, and here it is the whole design
//!
//! Every figure this module produces is `Option`-shaped or carries its
//! own "we have never seen a record for this" flag, because the corpus
//! this ships onto has **no** rows in this table and will not have any
//! for sessions that already ran. A session with no `PreCompact` rows
//! either never compacted or predates the hook, and those are not the
//! same claim. [`Observed`] is the type that keeps them apart, and
//! nothing here returns a bare `usize`.
//!
//! # Why the staleness problem is this module's and not the view's
//!
//! #1067's requirement -- "a stale waiting indicator is worse than
//! none" -- is a rule about evidence, and evidence is what this layer
//! holds. A view handed a bare `waiting: bool` cannot re-derive whether
//! the process is still there; it would have to trust the flag, and the
//! flag is a point in time that nothing corrects when a session is
//! SIGKILLed. So [`waiting`] takes the liveness verdict as a parameter
//! and returns a type whose present-tense variant **cannot be
//! constructed** without a `Running` process. See [`Waiting`].

use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::liveness::Liveness;

/// A count that knows whether it was ever in a position to count.
///
/// The root `CLAUDE.md` rule, as a type. `Some(0)` is "we have hook
/// records for this session and none of them were compactions"; `None`
/// is "no hook record of this kind has ever arrived for this session, so
/// this is not a zero, it is a silence".
///
/// On the corpus this ships onto every session is `None`, which is
/// exactly why it cannot be a `usize`: 1,524 rows rendering "0
/// compactions" would be 1,524 confident wrong answers, and the chart
/// shape would invite the eye to read them as a measured floor.
pub type Observed<T> = Option<T>;

/// How hard a session pushed against its context window (#1065).
///
/// Counted from `PreCompact` and not `PostCompact`; the argument for
/// which of the pair is installed lives on
/// [`super::hook::compaction`], next to the measurement that settles it.
///
/// # `manual` and `auto` are counted apart, and `other` is neither
///
/// `manual` is a user choice and `auto` is the session hitting a wall,
/// so a single total would answer neither question. The third field is
/// the one that matters for the rule about enum values: a trigger that
/// is neither documented value is NOT folded into either count and NOT
/// relabelled "other". It is kept verbatim in [`Self::unknown`], so the
/// UI renders the value Claude Code actually sent. A future trigger
/// named `emergency` should appear as `emergency`, not as a silent
/// increment to `auto`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compactions {
    /// Compactions the user asked for.
    pub manual: usize,
    /// Compactions the session ran into.
    pub auto: usize,
    /// Triggers that are neither, verbatim and counted, newest-first
    /// order not guaranteed.
    ///
    /// A `Vec` of pairs rather than a map so the serialised shape is
    /// stable for the UI and so an unknown value cannot collide with a
    /// field name.
    pub unknown: Vec<(String, usize)>,
    /// Compactions whose record carried no `trigger` at all.
    ///
    /// Distinct from an unknown VALUE: this is a record we read whose
    /// field was absent, which means the payload shape moved rather than
    /// the vocabulary growing.
    pub untriggered: usize,
}

impl Compactions {
    /// Every compaction counted, whatever its trigger.
    ///
    /// Sums the unknown values too: they are compactions, and a total
    /// that quietly omitted them would be short by an amount the reader
    /// cannot see.
    pub fn total(&self) -> usize {
        self.manual
            + self.auto
            + self.untriggered
            + self.unknown.iter().map(|(_, n)| n).sum::<usize>()
    }

    /// Whether repeated AUTO compactions are worth flagging on the row.
    ///
    /// #1065 asks for "a signal when auto-compactions are repeated,
    /// since that is the case a user might act on". Manual ones are
    /// deliberately excluded: a user who compacts by hand four times has
    /// not hit a wall, they have made four choices.
    ///
    /// The threshold is stated here rather than in the view so the
    /// backend and the two frontends cannot disagree about what "several
    /// times" means.
    pub fn under_pressure(&self) -> bool {
        self.auto >= AUTO_COMPACT_PRESSURE
    }
}

/// How many auto-compactions read as sustained context pressure.
///
/// Two, not one. A single auto-compaction is an ordinary long session
/// and flagging it would put a badge on a large share of real work; the
/// signal #1065 wants is the REPEATED case. Deliberately small because
/// the cost of the flag is one muted word on a row the user is already
/// looking at, not an interruption.
///
/// Pinned by `src/lib/mirroredConstants.test.ts` against its TypeScript
/// twin, so the row and the tile cannot disagree.
pub const AUTO_COMPACT_PRESSURE: usize = 2;

/// What a session STATED it spawned, against what we INFER it spawned
/// (#1066).
///
/// # The hook is an additional source, never a replacement
///
/// `subagent.rs` classifies a session as a subagent from its cwd, and
/// that inference was validated across 391 real cwds. It is load-bearing
/// for the session-list rollup and it is the ONLY source for every
/// session that already exists, because all of them predate the hook.
/// Nothing here removes it or overrides it.
///
/// What the hook adds is the agent's **type**, which the inference
/// cannot produce at all: a cwd of `.claude/worktrees/agent-<hex>`
/// yields an opaque id and no indication of whether that was an
/// `Explore` or a `code-reviewer`. So the two sources are not rivals for
/// the same field in the normal case -- the hook answers a question the
/// inference never could.
///
/// # Where they CAN disagree, and why it is surfaced rather than resolved
///
/// They overlap on one claim: whether a session spawned subagents at
/// all. The inference says so by attributing children to a parent
/// (`claude_subagent.parent_session_id`); the hook says so by recording
/// a `SubagentStart` against the parent's own `session_id` -- see
/// [`super::hook::subagents`] for why the parent is the id on that
/// payload.
///
/// When the hook recorded spawns for a session the inference attributed
/// no children to, the layout assumption has moved: subagents are
/// running somewhere the cwd rule does not recognise. #1066 is explicit
/// that this is "a finding worth surfacing rather than silently
/// resolving", so [`Self::disagreement`] returns the sentence and the UI
/// prints it. Picking a winner here would hide precisely the signal that
/// the inference needs revisiting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTypes {
    /// Each stated `agent_type` and how many were spawned, commonest
    /// first, then alphabetically so the order is total and the UI does
    /// not reshuffle between polls.
    ///
    /// This is what turns "3 subagents" into "2 general-purpose, 1
    /// code-reviewer", which is #1066's stated goal.
    pub stated: Vec<(String, usize)>,
    /// Spawns whose record carried no usable `agent_type`.
    ///
    /// The hook fired and named no type -- counted, never guessed at,
    /// and never folded into a named bucket. An empty string arrives
    /// here too: `handoff` stores `SubagentStop`'s `agent_type: b ?? ""`
    /// as NULL precisely so a blank cannot render as a type.
    pub untyped: usize,
    /// Subagent sessions the INFERENCE attributed to this one.
    ///
    /// Carried alongside rather than merged, because the disagreement
    /// check needs both numbers and because the two count different
    /// things: this counts child SESSIONS that exist, `stated` counts
    /// spawn EVENTS that were observed.
    pub inferred_children: usize,
}

impl AgentTypes {
    /// Spawns the hook observed, across all types.
    pub fn spawns(&self) -> usize {
        self.untyped + self.stated.iter().map(|(_, n)| n).sum::<usize>()
    }

    /// The sentence to show when the two sources do not agree, if they
    /// do not (#1066).
    ///
    /// `None` is the normal case and covers three genuinely-agreeing
    /// situations: both sources found subagents, neither did, or the
    /// hook has nothing to say because it was not installed when this
    /// session ran.
    ///
    /// # Why only ONE direction is a disagreement
    ///
    /// Hook spawns with no inferred children IS one: the session told us
    /// it spawned subagents and the cwd rule found none of them, which
    /// means they ran somewhere the rule does not look.
    ///
    /// Inferred children with no hook spawns is NOT one, and this is the
    /// distinction that keeps the check from crying wolf on every row.
    /// It is the expected state for every session that predates the
    /// install -- which is all of them today -- and for any session that
    /// ran while the hook was uninstalled. Reporting it would flag the
    /// entire corpus as contradictory on the day this ships, and a guard
    /// that fires everywhere is one that gets turned off.
    pub fn disagreement(&self) -> Option<String> {
        let spawns = self.spawns();
        if spawns > 0 && self.inferred_children == 0 {
            return Some(format!(
                "The hook recorded {spawns} subagent {} for this session, but \
                 none of its subagents were found by their working directory. \
                 Subagents may be running somewhere the directory rule does \
                 not recognise.",
                if spawns == 1 { "start" } else { "starts" },
            ));
        }
        None
    }
}

/// Whether a session is waiting on the user, and how sure we are (#1067).
///
/// # The problem this type exists to make unrepresentable
///
/// A `Notification` record is a point in time. Between it being written
/// and anyone reading it, the session may have been answered, exited, or
/// SIGKILLed -- and SIGKILL is the case `install.rs` records as leaving
/// no record at all, so there is nothing that ever arrives to correct
/// the file.
///
/// A stale "waiting" indicator is worse than no indicator, because it
/// sends the user to a session that does not need them. It is the same
/// failure as migration 11's refused `status` column and as #841's
/// fail-open: a value we could not refresh, presented as a fact.
///
/// So this enum has no boolean in it and no field a caller can set.
/// [`waiting`] is the only constructor, and the present-tense variant is
/// reachable only through a `Liveness::Running`. A caller that wants to
/// render "waiting" has to match [`Waiting::Now`], and there is no way
/// to get one for a dead process.
///
/// # The three answers, and why the third is not the first
///
/// - [`Waiting::Now`] -- the newest record for this session is an idle
///   prompt AND the process is running. Present tense, present-tense
///   evidence.
/// - [`Waiting::LastSeen`] -- it was waiting at a stated time, and we
///   cannot confirm it still is. #1067's own wording: "last seen
///   waiting at HH:MM" rather than a claim about now. This is what a
///   dead or unknown process gets, and it is a real answer rather than
///   an absence -- the session DID stop and ask for input, and that is
///   worth seeing on a row you are deciding whether to resume.
/// - [`Waiting::No`] -- nothing is waiting, for one of the reasons
///   [`NotWaiting`] distinguishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Waiting {
    /// Waiting on the user right now, with a live process to back it.
    Now {
        /// The notification type as Claude Code sent it, verbatim.
        ///
        /// `idle_prompt` and `permission_prompt` are the two #1067
        /// names, and they are DIFFERENT stories -- one session idling
        /// and one repeatedly asking permission -- so this is the value
        /// and not a boolean. Any other value renders as itself; there
        /// is no "other" bucket, per the epic's rule.
        kind: String,
        /// When the notification was recorded.
        at: String,
    },
    /// It asked for input at this time, and we cannot say whether it
    /// still needs it.
    LastSeen {
        kind: String,
        at: String,
        /// Why the present tense could not be claimed -- the liveness
        /// reason, carried through so the row's tooltip says which of
        /// "the process is gone" and "we could not tell" applies.
        why: String,
    },
    /// Not waiting. See [`NotWaiting`] for which kind of not.
    No {
        #[serde(flatten)]
        why: NotWaiting,
    },
}

/// The ways a session can fail to be waiting, which are not one way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum NotWaiting {
    /// No `Notification` record has ever arrived for this session.
    ///
    /// Absent is not zero: on every session that predates the install
    /// this is the answer, and it means "we were not watching", NOT
    /// "it never waited". The UI must not render it as a settled no.
    NeverObserved,
    /// A notification was recorded, and something newer happened since.
    ///
    /// This is the expiry rule #1067 asks for, and it is what makes the
    /// indicator clear itself: any later record for the session -- a
    /// `SessionEnd`, a later compaction, a later run -- means the
    /// session moved on, so the earlier "waiting" is spent.
    Superseded,
    /// The newest notification was not a request for input.
    ///
    /// `auth_success` and friends are notifications that say something
    /// happened, not that something is wanted.
    NotAPrompt,
}

/// The notification types that mean "a human is wanted" (#1067).
///
/// Only these two. The documented list also carries `auth_success`,
/// `elicitation_complete`, `quota_auto_resume_fired` and others that
/// report an event rather than request an answer, and treating those as
/// "waiting" would put the indicator on sessions that are working fine
/// -- which is the false-positive direction this feature cannot afford.
///
/// A type not in this list is not ignored: it is recorded, it is
/// rendered verbatim wherever the counts are shown, and it simply does
/// not raise the waiting indicator. Unknown values render as themselves.
pub const PROMPTS: &[&str] = &["idle_prompt", "permission_prompt"];

/// One session's rows out of `claude_hook_event`, already grouped.
///
/// Read once for the whole list rather than per row: the list has ~1,500
/// rows and a query each would be 1,500 round trips on a 10-second poll.
/// One `SELECT` over a table whose rows are bounded by how many hooks
/// have fired is cheaper than the `claude_run` read the list already
/// does.
#[derive(Debug, Clone, Default)]
pub struct Events {
    /// `PreCompact` triggers, in file order.
    compact_triggers: Vec<Option<String>>,
    /// `SubagentStart` types, in file order.
    agent_types: Vec<Option<String>>,
    /// The newest `Notification`'s type and time, if any.
    newest_notification: Option<(String, String)>,
    /// The newest timestamp of ANY row for this session, whatever the
    /// event -- the expiry clock for [`waiting`].
    newest_at: Option<String>,
}

impl Events {
    /// The compaction figures (#1065).
    ///
    /// `None` when no `PreCompact` row has ever arrived for this
    /// session, which is the absent-is-not-zero gate: a `Some(0)` from
    /// here would be a lie, because a session with no rows was never
    /// measured rather than measured at zero.
    pub fn compactions(&self) -> Observed<Compactions> {
        if self.compact_triggers.is_empty() {
            return None;
        }
        let mut out = Compactions::default();
        let mut unknown: HashMap<&str, usize> = HashMap::new();
        for t in &self.compact_triggers {
            match t.as_deref() {
                Some("manual") => out.manual += 1,
                Some("auto") => out.auto += 1,
                // Verbatim, never "other". A vocabulary that grew is
                // information; a bucket named "other" destroys it.
                Some(other) => *unknown.entry(other).or_default() += 1,
                None => out.untriggered += 1,
            }
        }
        out.unknown = sorted_counts(unknown);
        Some(out)
    }

    /// The stated agent types (#1066).
    ///
    /// `inferred_children` is supplied by the caller because it comes
    /// from `claude_subagent`, which this module does not own -- and
    /// because passing it in is what lets [`AgentTypes::disagreement`]
    /// compare two sources rather than one source and an assumption.
    ///
    /// Returns `Some` whenever EITHER source has something to say. A
    /// session with inferred children and no hook rows still gets an
    /// `AgentTypes`, because that is the normal pre-hook state and the
    /// rollup must keep working for it -- which is #1066's "keep the
    /// existing inference as the fallback" as code rather than as a
    /// promise.
    pub fn agent_types(&self, inferred_children: usize) -> Observed<AgentTypes> {
        if self.agent_types.is_empty() && inferred_children == 0 {
            return None;
        }
        let mut out = AgentTypes {
            inferred_children,
            ..Default::default()
        };
        let mut named: HashMap<&str, usize> = HashMap::new();
        for t in &self.agent_types {
            // `filter` on emptiness as well as absence: `handoff`
            // already stores an empty type as NULL, and doing it again
            // here means a row written by an older Headstate cannot
            // produce a blank bucket either.
            match t.as_deref().filter(|s| !s.is_empty()) {
                Some(name) => *named.entry(name).or_default() += 1,
                None => out.untyped += 1,
            }
        }
        out.stated = sorted_counts(named);
        Some(out)
    }

    /// Whether this session is waiting on the user (#1067).
    ///
    /// `liveness` is a PARAMETER and not read here, for the reason the
    /// module docs give: the cross-check is the whole feature, and a
    /// function that derived its own liveness would be a second opinion
    /// that can disagree with the badge rendered beside it. The caller
    /// has already derived it for the row.
    ///
    /// # The order of the checks is the argument
    ///
    /// 1. **No record at all** -> `NeverObserved`. Not a no.
    /// 2. **Something newer happened** -> `Superseded`. This is the
    ///    expiry, and it comes BEFORE the liveness check on purpose: a
    ///    session that was answered and carried on is not waiting even
    ///    though its process is very much alive, and checking liveness
    ///    first would report it as waiting forever.
    /// 3. **Not a prompt type** -> `NotAPrompt`.
    /// 4. **Running** -> `Now`. The only path to the present tense.
    /// 5. **Anything else** -> `LastSeen`, carrying the liveness reason.
    ///    `Dead` and `Unknown` both land here, and deliberately: "the
    ///    process is gone" and "we could not tell" are different
    ///    sentences but the same entitlement, which is none. Neither may
    ///    claim the present tense.
    pub fn waiting(&self, liveness: &Liveness) -> Waiting {
        let Some((kind, at)) = self.newest_notification.as_ref() else {
            return Waiting::No {
                why: NotWaiting::NeverObserved,
            };
        };
        // Expiry: any row newer than the notification means the session
        // moved on. Compared as RFC 3339 strings, which sort
        // lexicographically in the same order they sort chronologically
        // for a fixed offset -- and the hook writes UTC with an explicit
        // offset for exactly this reason (see `hook::Record::ts`).
        if self.newest_at.as_deref().is_some_and(|n| n > at.as_str()) {
            return Waiting::No {
                why: NotWaiting::Superseded,
            };
        }
        if !PROMPTS.contains(&kind.as_str()) {
            return Waiting::No {
                why: NotWaiting::NotAPrompt,
            };
        }
        match liveness {
            Liveness::Running { .. } => Waiting::Now {
                kind: kind.clone(),
                at: at.clone(),
            },
            Liveness::Dead { why } | Liveness::Unknown { why } => Waiting::LastSeen {
                kind: kind.clone(),
                at: at.clone(),
                why: why.clone(),
            },
        }
    }
}

/// Counts as a stable, total order: commonest first, then by name.
///
/// The second key is not decoration. Two agent types with the same count
/// would otherwise come out in `HashMap` iteration order, which differs
/// between runs, and the UI would reshuffle a list on a poll where
/// nothing changed.
fn sorted_counts(counts: HashMap<&str, usize>) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> =
        counts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Read every session's hook events, grouped by session.
///
/// One query for the whole corpus. Sessions with no rows are ABSENT from
/// the map rather than present with an empty [`Events`], so the caller's
/// `get` returning `None` is the never-observed case and the
/// absent-is-not-zero rule survives the lookup.
///
/// # Ordering
///
/// `ORDER BY at` ascending so the last `Notification` seen while
/// scanning is the newest one, which is what [`Events::waiting`] reads.
/// Doing it in SQL rather than comparing timestamps per row keeps this
/// one pass with no per-session sort.
pub fn events_by_session(conn: &Connection) -> Result<HashMap<String, Events>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT session_id, event, at, trigger_kind, agent_type, notification_type
         FROM claude_hook_event
         ORDER BY at ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut out: HashMap<String, Events> = HashMap::new();
    for row in rows {
        let (sid, event, at, trigger, agent_type, notification) = row?;
        let e = out.entry(sid).or_default();
        match event.as_str() {
            "PreCompact" => e.compact_triggers.push(trigger),
            "SubagentStart" => e.agent_types.push(agent_type),
            "Notification" => {
                if let Some(kind) = notification {
                    // Ascending order means a later row is newer, so the
                    // last one to arrive wins.
                    e.newest_notification = Some((kind, at.clone()));
                }
            }
            // An event this app does not recognise still advances the
            // expiry clock below. It was written by a newer hook, it is
            // activity on this session, and a "waiting" indicator that
            // ignored it would outlive evidence that the session moved
            // on. Degrading toward clearing the flag is the safe
            // direction.
            _ => {}
        }
        // EVERY row, recognised or not. `at` is ascending, so the last
        // assignment is the newest.
        e.newest_at = Some(at);
    }
    Ok(out)
}

/// The newest activity timestamp per session from OUTSIDE this table.
///
/// The expiry rule in [`Events::waiting`] says "any newer record for
/// that session", and the records that matter most are the ones this
/// table does not hold: a `SessionEnd` is the strongest possible
/// evidence that nobody is waiting, and it lives in `claude_run`.
///
/// #1067's test "a waiting indicator does not survive a `SessionEnd`"
/// is exactly this join, which is why it is read here rather than left
/// to the liveness check. Liveness alone would not clear it: a session
/// that ended cleanly and whose pid has been REUSED by another live
/// process reads as... well, `derive` guards that with
/// `pid_start_time`, but the honest reason is simpler -- an ended
/// session is not waiting even if we cannot tell what its pid is doing,
/// and that fact is in the run table.
pub fn newest_run_activity(conn: &Connection) -> Result<HashMap<String, String>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT session_id, MAX(COALESCE(ended_at, started_at))
         FROM claude_run GROUP BY session_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (sid, at) = row?;
        if let Some(at) = at {
            out.insert(sid, at);
        }
    }
    Ok(out)
}

impl Events {
    /// Fold a run-table timestamp into the expiry clock.
    ///
    /// Called once per session after both reads, so [`Events::waiting`]
    /// compares the notification against the newest evidence from EITHER
    /// table. Without this a `SessionEnd` would not clear a waiting
    /// indicator, which is the one expiry #1067 names by hand.
    pub fn observe_activity(&mut self, at: &str) {
        if self.newest_at.as_deref().is_none_or(|n| at > n) {
            self.newest_at = Some(at.to_owned());
        }
    }
}

#[cfg(test)]
mod tests;
