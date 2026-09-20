//! Stopping a Claude Code session Headstate is already tracking (#1219).
//!
//! # Why Headstate intervenes at all
//!
//! [`super`]'s header is a rule about **writing to `~/.claude`**, and
//! signalling a process is not that. But the module's spirit is
//! "Headstate does not intervene in Claude Code", and this intervenes --
//! so reading that header as permission would be wrong, and the case is
//! made here on its own terms.
//!
//! Headstate already tells the user a session auto-compacted repeatedly
//! ([`super::signals::Compactions`]), that its tools are failing in a
//! concentrated pattern, that it has waited for hours
//! ([`super::signals::Waiting`]), and that the machine is oversubscribed
//! (`health::runaway`). It surfaces every input to "should I stop this"
//! and then makes the user re-find, in Activity Monitor or a terminal, **a
//! pid Headstate already holds**. That gap is not neutrality; it is the
//! single-pane-of-glass promise breaking at exactly the moment the pane
//! has done its job.
//!
//! The intervention is the narrowest one available: signalling a process
//! the user owns, on the machine in front of them, with the evidence shown
//! and the action PROPOSED rather than taken. Editing files Claude Code
//! owns remains out of bounds and nothing here does it.
//!
//! # `Class::Local`, and the phone cannot reach it
//!
//! `remote/surface.rs` classes `claude_stop_session` as `Class::Local` and
//! gives it **no dispatch arm**, so the stop does not cross the remote
//! wire. The class's own test is whether the phone could act on the
//! answer, and a phone cannot see or use a terminated process on a Mac it
//! is not sitting at. `Destructive` was considered and rejected: a stopped
//! session keeps its transcript and can be resumed, so a step-up signature
//! would be friction disproportionate to a recoverable action -- and would
//! make the genuinely irreversible actions feel routine by association.
//!
//! # SIGTERM first, and never a bare SIGKILL
//!
//! [`super::registry`]'s header records the measurement: a SIGKILLed
//! session **leaves its registry file behind** with `status` frozen at
//! `"busy"`, and `SessionEnd` never fires. Confirmed by experiment -- the
//! file was still there twenty seconds later, not a late reap. The user
//! gets no record of what the session was mid-way through.
//!
//! So [`Escalation`] is SIGTERM, then a bounded wait, then SIGKILL only if
//! the process is still there -- and the caller is told, in
//! [`StopOutcome::signal`], which one actually ended it. There is no path
//! through this module that sends SIGKILL first;
//! `sigterm_is_always_sent_before_any_escalation` pins that.
//!
//! # The pid is re-derived AT THE MOMENT of the stop
//!
//! The session list is ten seconds stale (`CLAUDE_POLL_MS`), and reading a
//! pid from it and signalling that is how you kill a process that
//! INHERITED the pid. `caches/mod.rs`'s `remove_venv` states the rule for
//! deletions -- "every check runs against the filesystem rather than the
//! row the user clicked" -- and this is the same rule for a signal.
//!
//! [`confirm`] therefore re-reads `~/.claude/sessions/` and re-probes the
//! process table on this call, and pairs the two with
//! [`super::liveness::START_TOLERANCE_SECS`] exactly as
//! [`super::liveness::derive`] does. That pairing is not reinvented here;
//! reinventing it would give the machine two answers to "is this the same
//! process" that drift apart the first time either changes.
//!
//! **If the start time cannot be confirmed, that is `Unknown` and the stop
//! is REFUSED.** Not deferred, not attempted on a guess. The failure this
//! prevents is destroying someone's unrelated work, and the only safe
//! direction for an unconfirmable pid is to do nothing and say why.
//!
//! # Proposed with evidence, never acted on unattended
//!
//! #1141's `cleanup::propose` is a better shape than a confirmation
//! dialog, and this follows it: [`propose`] returns [`StopProposal`]s
//! carrying their evidence, under a per-run blast-radius cap
//! ([`MAX_PER_RUN`]), with `"proposed"`, `"refused"` and `"skipped"`
//! first-class in the ledger. A refusal is a ROW, not a dropped entry --
//! a stop that keeps being passed over is something the user must be able
//! to see.
//!
//! Per `health::runaway`'s Notice-vs-Alert framing, a stuck session is an
//! INDICATOR and not a recommendation. Nothing in this module decides that
//! a session should be stopped; it states what is true about one and lets
//! the user decide.

use serde::{Deserialize, Serialize};

use super::liveness::{ProcessProbe, Registry, START_TOLERANCE_SECS};

/// How many stops one proposal pass may put in front of the user.
///
/// The blast-radius cap `cleanup::max_per_run` establishes, at the size
/// the action warrants. A pass that proposed forty stops is not a
/// proposal, it is a prompt to click through -- and clicking through is
/// how the pid-reuse refusal below gets skimmed past.
pub const MAX_PER_RUN: usize = 5;

/// How long SIGTERM is given before SIGKILL is considered.
///
/// A Claude Code session handling SIGTERM has to flush its transcript and
/// let `SessionEnd` fire, which is the entire reason SIGTERM goes first.
/// Five seconds is long enough for that on the reporting machine and short
/// enough that a user watching a button does not conclude it did nothing.
///
/// It is a CEILING, not a sleep: [`stop`] polls and returns as soon as the
/// process is gone, so a session that exits in 80 ms costs 80 ms.
pub const TERM_GRACE_SECS: u64 = 5;

/// How often the grace period re-checks whether the process is gone.
pub const POLL_INTERVAL_MS: u64 = 100;

/// Why a stop was not attempted.
///
/// An enum and not a `String` because the arms have DIFFERENT remedies,
/// which is the rule `launch::LaunchError` states: "this session is not
/// running" needs no remedy at all, "the pid has been reused" means the
/// list is stale and a refresh fixes it, and "we could not read the
/// registry" is a permissions problem on `~/.claude/sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Refusal {
    /// The registry directory could not be listed at all, so the entry
    /// proving this session is alive may be one we could not see.
    RegistryUnreadable { why: String },
    /// The registry listed fine and does not mention this session.
    NotRunning { why: String },
    /// The pid is there, but the start times disagree -- a different
    /// process is wearing the number. **This is the refusal that matters
    /// most**: signalling here destroys unrelated work.
    PidReused { pid: u32, drift_secs: i64 },
    /// The start time could not be established either way. NOT a shade of
    /// `NotRunning`, for `liveness`'s stated reason: a check that could
    /// not be COMPLETED is not a check that came back negative.
    Unconfirmable { why: String },
    /// The per-run cap was reached before this one was considered.
    CapReached { cap: usize },
}

impl Refusal {
    /// The sentence shown to the user and stored in the ledger's `error`.
    pub fn why(&self) -> String {
        match self {
            Refusal::RegistryUnreadable { why } => format!(
                "the live session registry could not be read ({why}), so this session's \
                 process could not be confirmed and nothing was signalled"
            ),
            Refusal::NotRunning { why } => why.clone(),
            Refusal::PidReused { pid, drift_secs } => format!(
                "pid {pid} is running but started {drift_secs}s from the recorded time, so the \
                 number has been reused by a different process -- nothing was signalled"
            ),
            Refusal::Unconfirmable { why } => format!(
                "{why}, so this could not be told from a recycled pid and nothing was signalled"
            ),
            Refusal::CapReached { cap } => format!(
                "only {cap} stops are proposed at a time, so this one was not considered in \
                 this pass"
            ),
        }
    }
}

/// A pid that has been confirmed, on THIS call, to be the session's own.
///
/// The type is the guarantee. It has no public constructor other than
/// [`confirm`], so there is no way to reach [`stop`] with a pid read off a
/// ten-second-old list -- which is the whole failure mode this module is
/// written against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedPid {
    pid: u32,
    session_id: String,
    /// The start time both sources agreed on, kept so a caller can state
    /// what was compared rather than asserting that something was.
    started_at: i64,
}

impl ConfirmedPid {
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn started_at(&self) -> i64 {
        self.started_at
    }
}

/// Re-derive, NOW, whether `session_id` is the process `pid` claims.
///
/// Every check is against the registry read and the probe passed in on
/// THIS call. Nothing is taken from a rendered row.
///
/// The arms mirror [`super::liveness::derive`] deliberately, including the
/// order: a registry that could not be listed poisons every answer and is
/// checked before the entry lookup, because a miss in a partially-read map
/// is not evidence of anything.
pub fn confirm<P: ProcessProbe>(
    probe: &P,
    registry: &Registry,
    session_id: &str,
) -> Result<ConfirmedPid, Refusal> {
    if let Some(why) = &registry.failure {
        return Err(Refusal::RegistryUnreadable { why: why.clone() });
    }

    let Some(entry) = registry.entries.get(session_id) else {
        // An entry we could not PARSE is not an absent one -- the same
        // distinction `derive` draws one level out. Either way nothing is
        // signalled, but the reasons differ and so do the remedies.
        if !registry.unreadable.is_empty() {
            return Err(Refusal::Unconfirmable {
                why: format!(
                    "{} live-session record(s) could not be read, so this session not \
                     appearing among the rest is not evidence that it is gone",
                    registry.unreadable.len()
                ),
            });
        }
        return Err(Refusal::NotRunning {
            why: "the live session registry was read and does not list this session, so there \
                  is no process to stop"
                .into(),
        });
    };

    let Some(text) = entry.proc_start.as_deref() else {
        return Err(Refusal::Unconfirmable {
            why: format!(
                "the registry lists pid {} for this session but no start time",
                entry.pid
            ),
        });
    };
    let Some(recorded) = super::liveness::parse_proc_start(text) else {
        return Err(Refusal::Unconfirmable {
            why: format!(
                "the recorded start time {text:?} for pid {} could not be read",
                entry.pid
            ),
        });
    };

    match probe.start_time(entry.pid) {
        Err(why) => Err(Refusal::Unconfirmable {
            why: format!(
                "whether pid {} is running could not be checked: {why}",
                entry.pid
            ),
        }),
        Ok(None) => Err(Refusal::NotRunning {
            why: format!(
                "pid {} is in the live session registry but is no longer running, so there is \
                 nothing to stop",
                entry.pid
            ),
        }),
        // The SAME pairing `liveness::derive` uses, with the same
        // tolerance, reached through the same constant. Reinventing it
        // here would give the app two answers to one question.
        Ok(Some(actual)) if (actual - recorded).abs() <= START_TOLERANCE_SECS => Ok(ConfirmedPid {
            pid: entry.pid,
            session_id: session_id.to_string(),
            started_at: actual,
        }),
        Ok(Some(actual)) => Err(Refusal::PidReused {
            pid: entry.pid,
            drift_secs: (actual - recorded).abs(),
        }),
    }
}

/// Which signal actually ended the process.
///
/// Reported rather than assumed, because the two outcomes leave the user
/// with different things: a session that took SIGTERM wrote its transcript
/// tail and fired `SessionEnd`, and a session that needed SIGKILL did
/// neither and left its registry file behind with `status` frozen at
/// `"busy"`. The UI says which happened; the user should not have to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Escalation {
    /// SIGTERM was sent and the process was gone within the grace period.
    Terminated,
    /// SIGTERM was sent, the grace period elapsed, and SIGKILL followed.
    ///
    /// Reaching this arm requires having sent SIGTERM first: [`stop`] has
    /// no other path to it.
    Killed,
}

/// What one stop did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopOutcome {
    pub session_id: String,
    pub pid: u32,
    /// Which signal ended it. See [`Escalation`].
    pub signal: Escalation,
    /// How long the process took to go after SIGTERM, in milliseconds.
    pub waited_ms: u64,
}

/// Sending a signal and asking whether a process is still there.
///
/// A trait so [`stop`] is testable without a real process --
/// **no test in this module may signal a live pid**, and this is what
/// makes that possible rather than aspirational.
pub trait Signaller {
    /// Send SIGTERM. `Ok(())` means the signal was delivered.
    fn term(&self, pid: u32) -> Result<(), String>;
    /// Send SIGKILL. Only ever called after [`Signaller::term`].
    fn kill(&self, pid: u32) -> Result<(), String>;
    /// Whether the pid is still in the process table.
    ///
    /// `Err` is "could not tell", which [`stop`] treats as "still there"
    /// rather than as gone: concluding success from a failed check is the
    /// fail-open `liveness` is written against.
    fn alive(&self, pid: u32) -> Result<bool, String>;
    /// Wait before re-checking. Injected so a test does not sleep.
    fn wait(&self, ms: u64);
}

/// SIGTERM, a bounded wait, then SIGKILL only if it is still there.
///
/// The escalation is not optional and not configurable, because the point
/// of it is the ORDER: `registry.rs` measured that a bare SIGKILL costs
/// the user the record of what the session was doing, and a setting that
/// could skip the grace period would reintroduce exactly that.
///
/// `pid` is a [`ConfirmedPid`], so this cannot be reached with a pid that
/// was not re-derived on this call.
pub fn stop<S: Signaller>(sig: &S, confirmed: &ConfirmedPid) -> Result<StopOutcome, String> {
    let pid = confirmed.pid;
    sig.term(pid)
        .map_err(|e| format!("could not signal pid {pid}: {e}"))?;

    let deadline_ms = TERM_GRACE_SECS * 1_000;
    let mut waited = 0u64;
    while waited < deadline_ms {
        // Checked BEFORE the first wait would be wrong the other way --
        // a process given no time at all to handle SIGTERM would look
        // like one that ignored it. One interval first, then the check.
        sig.wait(POLL_INTERVAL_MS);
        waited += POLL_INTERVAL_MS;
        // An unreadable process table is NOT "it is gone". Treated as
        // still-there, which costs a wait and at worst a SIGKILL that
        // finds nothing -- the opposite error would report a live
        // session as stopped.
        if let Ok(false) = sig.alive(pid) {
            return Ok(StopOutcome {
                session_id: confirmed.session_id.clone(),
                pid,
                signal: Escalation::Terminated,
                waited_ms: waited,
            });
        }
    }

    sig.kill(pid)
        .map_err(|e| format!("pid {pid} did not exit and could not be killed: {e}"))?;
    Ok(StopOutcome {
        session_id: confirmed.session_id.clone(),
        pid,
        signal: Escalation::Killed,
        waited_ms: waited,
    })
}

/// The evidence shown beside a proposed stop.
///
/// Everything here is a FACT about the session, and none of it is a
/// recommendation. `health::runaway`'s Notice-vs-Alert split is the rule:
/// the watch tier describes conditions a large build produces
/// legitimately, so it is an indicator on a page and never an
/// interruption. A long-running session is the same shape of claim, and
/// this struct must not grow a `should_stop` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopEvidence {
    /// The session's own handle, when the registry carried one.
    pub name: Option<String>,
    /// Where it is running.
    pub cwd: Option<String>,
    /// `busy` / `idle` as the session last PUBLISHED it. Advisory only,
    /// for the reason `registry.rs` §"`status` is deliberately
    /// distrusted" gives: it is stored, not derived.
    pub status: Option<String>,
    /// How long the process has been up, in seconds, from the start time
    /// [`confirm`] compared. `None` when the clock could not be read.
    pub uptime_secs: Option<i64>,
    /// How many times it auto-compacted, when a record exists.
    ///
    /// `Option` because absent is not zero (#1065): no `PreCompact` row
    /// means either no compaction or a session predating the hook, and
    /// rendering the second as "0" is a confident wrong answer.
    pub auto_compactions: Option<u32>,
    /// The last turn, as `preview::tail` renders it -- what the session
    /// was SAYING when the user went looking.
    ///
    /// The issue's requirement, and the reason the proposal is not a bare
    /// confirmation dialog: a user asked to end something must be shown
    /// what they are ending. `None` when the transcript could not be read,
    /// which the UI states rather than leaving a blank.
    pub last_turn: Option<String>,
}

/// One session the pass considered, with what it found.
///
/// The `cleanup::LedgerEntry` shape in this domain's terms: an `action` of
/// `"proposed"`, `"refused"` or `"skipped"`, a target, and the evidence.
/// A refusal is a ROW here, exactly as it is there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopProposal {
    pub session_id: String,
    /// `"proposed"` or `"refused"`.
    pub action: String,
    /// Set only on `"proposed"`: the pid confirmed at proposal time.
    ///
    /// Shown so the user can see WHICH process is meant, and deliberately
    /// NOT what the stop signals -- [`confirm`] runs again at the moment
    /// of the stop, because this number is stale the instant it is
    /// rendered.
    pub pid: Option<u32>,
    /// Set only on `"refused"`.
    pub refusal: Option<Refusal>,
    /// Why, in one sentence, for both actions.
    pub why: Option<String>,
    pub evidence: StopEvidence,
}

/// Build the proposal list for one pass, under the blast-radius cap.
///
/// Every session asked about gets a ROW, including the refusals and
/// including the ones the cap pushed out -- `cleanup::propose`'s rule,
/// and for its stated reason: an entry that keeps being passed over is
/// something the user should be able to see, not a silent gap.
///
/// Pure apart from the probe: it proposes, and the caller decides what to
/// do with the list. Nothing here signals anything.
pub fn propose<P: ProcessProbe>(
    probe: &P,
    registry: &Registry,
    session_ids: &[String],
    evidence_for: impl Fn(&str, Option<i64>) -> StopEvidence,
) -> Vec<StopProposal> {
    let mut out = Vec::new();
    let mut considered = 0usize;
    for id in session_ids {
        if considered >= MAX_PER_RUN {
            let r = Refusal::CapReached { cap: MAX_PER_RUN };
            out.push(StopProposal {
                session_id: id.clone(),
                action: "refused".into(),
                pid: None,
                why: Some(r.why()),
                refusal: Some(r),
                evidence: evidence_for(id, None),
            });
            continue;
        }
        considered += 1;
        match confirm(probe, registry, id) {
            Ok(c) => out.push(StopProposal {
                session_id: id.clone(),
                action: "proposed".into(),
                pid: Some(c.pid()),
                refusal: None,
                why: Some(format!(
                    "pid {} is running and its start time matches what the registry recorded",
                    c.pid()
                )),
                evidence: evidence_for(id, Some(c.started_at())),
            }),
            Err(r) => out.push(StopProposal {
                session_id: id.clone(),
                action: "refused".into(),
                pid: None,
                why: Some(r.why()),
                refusal: Some(r),
                evidence: evidence_for(id, None),
            }),
        }
    }
    out
}

/// The real signaller, over `libc::kill`.
///
/// Unix only. Windows has no SIGTERM, and the grace period this module is
/// built around has no analogue there -- so rather than pretending, the
/// non-Unix build has no implementation and the command refuses.
#[cfg(unix)]
pub struct UnixSignaller;

#[cfg(unix)]
impl UnixSignaller {
    /// `kill(2)`, with `errno` read on failure.
    ///
    /// `ESRCH` is folded into success for the two SIGNALS, because a
    /// process that exited between the confirmation and the signal is the
    /// outcome that was wanted, not an error to report.
    fn send(pid: u32, signal: i32) -> Result<(), String> {
        // SAFETY: `kill` takes two integers, touches no memory we own,
        // and the pid came from `ConfirmedPid` -- so it was in the process
        // table when it was confirmed, and is never 0 or negative (which
        // is what would make this signal a process GROUP).
        let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(err.to_string())
    }
}

#[cfg(unix)]
impl Signaller for UnixSignaller {
    fn term(&self, pid: u32) -> Result<(), String> {
        Self::send(pid, libc::SIGTERM)
    }

    fn kill(&self, pid: u32) -> Result<(), String> {
        Self::send(pid, libc::SIGKILL)
    }

    fn alive(&self, pid: u32) -> Result<bool, String> {
        // Signal 0 is the liveness probe `kill(2)` documents: it performs
        // the permission checks and finds the process, and sends nothing.
        //
        // SAFETY: as `send` above, and with signal 0 nothing is delivered.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            // EPERM means it EXISTS and is not ours. Alive, and the
            // signal will fail -- which is the honest answer rather than
            // reporting it stopped.
            Some(libc::EPERM) => Ok(true),
            _ => Err(err.to_string()),
        }
    }

    fn wait(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[cfg(test)]
mod tests;
