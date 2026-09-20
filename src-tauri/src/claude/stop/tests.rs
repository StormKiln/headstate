//! Tests for #1219's stop.
//!
//! **No test here signals a live pid.** Every process fact comes from a
//! fixture: [`FakeProbe`] scripts the process table and [`FakeSignal`]
//! records what was sent without sending anything. That is not a
//! convenience -- a test that signalled an arbitrary pid on the machine
//! running `cargo test` would be the exact accident this module is
//! written to prevent, committed as a test.
//!
//! Each of the four mandated cases names the sabotage that was run
//! against it and what was seen when the code it covers was broken.

use super::*;
use crate::claude::liveness::RegistryEntry;
use std::cell::RefCell;

/// A probe whose answers -- including its FAILURE -- the test chooses.
///
/// The failure arm is why this exists rather than a `ps` call: the real
/// process table cannot be made to fail on demand, so `Unconfirmable`
/// would otherwise be untested, and it is the arm whose absence would
/// turn "we could not tell" into a signal sent on a guess.
struct FakeProbe(Result<Option<i64>, String>);
impl ProcessProbe for FakeProbe {
    fn start_time(&self, _pid: u32) -> Result<Option<i64>, String> {
        self.0.clone()
    }
}

/// `Fri Sep 11 09:43:48 2026` UTC, from a real registry file.
const PROC_START: &str = "Fri Sep 11 09:43:48 2026";
const PROC_START_EPOCH: i64 = 1_789_119_828;
const PID: u32 = 14779;

fn registry() -> Registry {
    let mut r = Registry::default();
    r.entries.insert(
        "s1".into(),
        RegistryEntry {
            pid: PID,
            session_id: "s1".into(),
            proc_start: Some(PROC_START.into()),
            cwd: Some("/Users/acme/code/widget".into()),
            name: Some("widget-c3".into()),
            status: Some("busy".into()),
            version: Some("2.0.1".into()),
        },
    );
    r
}

fn no_evidence(_id: &str, _started: Option<i64>) -> StopEvidence {
    StopEvidence {
        name: None,
        cwd: None,
        status: None,
        uptime_secs: None,
        auto_compactions: None,
        last_turn: None,
    }
}

/// Everything sent to a pid, in order, with nothing actually sent.
#[derive(Default)]
struct Log {
    sent: Vec<&'static str>,
    waits: Vec<u64>,
}

/// A signaller that records and never signals.
///
/// `alive_after` is how many liveness checks report "still there" before
/// the process is reported gone; `usize::MAX` is a process that ignores
/// SIGTERM entirely, which is the escalation case.
struct FakeSignal {
    log: RefCell<Log>,
    alive_after: RefCell<usize>,
    /// When set, `alive` reports the process table could not be read.
    probe_fails: bool,
}

impl FakeSignal {
    fn new(alive_after: usize) -> Self {
        Self {
            log: RefCell::new(Log::default()),
            alive_after: RefCell::new(alive_after),
            probe_fails: false,
        }
    }
    fn unreadable() -> Self {
        Self {
            log: RefCell::new(Log::default()),
            alive_after: RefCell::new(usize::MAX),
            probe_fails: true,
        }
    }
    fn sent(&self) -> Vec<&'static str> {
        self.log.borrow().sent.clone()
    }
}

impl Signaller for FakeSignal {
    fn term(&self, _pid: u32) -> Result<(), String> {
        self.log.borrow_mut().sent.push("TERM");
        Ok(())
    }
    fn kill(&self, _pid: u32) -> Result<(), String> {
        self.log.borrow_mut().sent.push("KILL");
        Ok(())
    }
    fn alive(&self, _pid: u32) -> Result<bool, String> {
        if self.probe_fails {
            return Err("Operation not permitted".into());
        }
        let mut n = self.alive_after.borrow_mut();
        if *n == 0 {
            return Ok(false);
        }
        *n = n.saturating_sub(1);
        Ok(true)
    }
    fn wait(&self, ms: u64) {
        // Recorded, never slept. A five-second grace period in a unit
        // test is five seconds of CI.
        self.log.borrow_mut().waits.push(ms);
    }
}

/// A [`ConfirmedPid`] built the only way one can be: through [`confirm`].
///
/// Deliberately NOT a literal struct construction. The type's guarantee
/// is that it cannot be made without a confirmation, and a test that
/// fabricated one would be testing a different type from the one that
/// ships.
fn confirmed() -> ConfirmedPid {
    confirm(&FakeProbe(Ok(Some(PROC_START_EPOCH))), &registry(), "s1")
        .expect("the matching start time must confirm")
}

// ---------------------------------------------------------------------
// MANDATED 1: a mismatched start time is REFUSED, not signalled.
// ---------------------------------------------------------------------

/// The pid-reuse case, and the one that would destroy unrelated work.
///
/// SABOTAGE: widened `confirm`'s tolerance check to `.abs() <=
/// START_TOLERANCE_SECS * 10_000`, so the drifted start time fell inside
/// the window. The test FAILED with `Ok(ConfirmedPid { pid: 14779, .. })`
/// where a `Refusal::PidReused` was expected. Restored, and it passed.
#[test]
fn a_pid_whose_start_time_does_not_match_is_refused() {
    // Eight hours later: the number was recycled by something unrelated.
    let drifted = PROC_START_EPOCH + 8 * 3600;
    let got = confirm(&FakeProbe(Ok(Some(drifted))), &registry(), "s1");
    assert_eq!(
        got,
        Err(Refusal::PidReused {
            pid: PID,
            drift_secs: 8 * 3600,
        })
    );
    // And it SAYS so, rather than refusing mutely.
    let why = got.unwrap_err().why();
    assert!(why.contains("reused"), "{why}");
    assert!(why.contains("nothing was signalled"), "{why}");
}

/// The tolerance is the SHARED one, not a local copy.
///
/// A drift inside `START_TOLERANCE_SECS` is the same process with two
/// clocks; outside it, it is not. Pinned at both sides of the boundary so
/// a divergent constant here would fail rather than silently widen what
/// gets signalled.
#[test]
fn the_pid_pairing_uses_livenesss_own_tolerance() {
    let inside = PROC_START_EPOCH + START_TOLERANCE_SECS;
    assert!(confirm(&FakeProbe(Ok(Some(inside))), &registry(), "s1").is_ok());

    let outside = PROC_START_EPOCH + START_TOLERANCE_SECS + 1;
    assert!(matches!(
        confirm(&FakeProbe(Ok(Some(outside))), &registry(), "s1"),
        Err(Refusal::PidReused { .. })
    ));
}

/// A start time that could not be established is Unknown, and Unknown
/// REFUSES. Not a shade of "not running", per `liveness`'s rule.
#[test]
fn an_unconfirmable_start_time_refuses_rather_than_guessing() {
    // The process table could not be read.
    assert!(matches!(
        confirm(
            &FakeProbe(Err("Operation not permitted".into())),
            &registry(),
            "s1"
        ),
        Err(Refusal::Unconfirmable { .. })
    ));

    // The registry recorded no start time at all.
    let mut r = registry();
    r.entries.get_mut("s1").unwrap().proc_start = None;
    assert!(matches!(
        confirm(&FakeProbe(Ok(Some(PROC_START_EPOCH))), &r, "s1"),
        Err(Refusal::Unconfirmable { .. })
    ));

    // The recorded start time did not parse.
    let mut r = registry();
    r.entries.get_mut("s1").unwrap().proc_start = Some("11 Sep 09:43:48".into());
    assert!(matches!(
        confirm(&FakeProbe(Ok(Some(PROC_START_EPOCH))), &r, "s1"),
        Err(Refusal::Unconfirmable { .. })
    ));
}

/// A registry that could not be LISTED poisons every answer, and is
/// checked before the entry lookup -- a miss in a partially-read map is
/// not evidence of anything.
#[test]
fn an_unreadable_registry_refuses_before_any_lookup() {
    let mut r = registry();
    r.failure = Some("Permission denied".into());
    assert!(matches!(
        confirm(&FakeProbe(Ok(Some(PROC_START_EPOCH))), &r, "s1"),
        Err(Refusal::RegistryUnreadable { .. })
    ));
}

/// A pid that is simply gone is `NotRunning`, which is a settled answer
/// and not a failure -- distinct from the three arms above.
#[test]
fn a_session_whose_process_has_exited_is_not_running_rather_than_unconfirmable() {
    assert!(matches!(
        confirm(&FakeProbe(Ok(None)), &registry(), "s1"),
        Err(Refusal::NotRunning { .. })
    ));
}

// ---------------------------------------------------------------------
// MANDATED 2: SIGTERM is sent before any escalation.
// ---------------------------------------------------------------------

/// SIGTERM first, always, and SIGKILL only after the grace period.
///
/// `registry.rs` measured what a bare SIGKILL costs: the registry file
/// stays behind with `status` frozen at `"busy"` and `SessionEnd` never
/// fires, so the user has no record of what the session was mid-way
/// through.
///
/// SABOTAGE: reordered `stop` to call `sig.kill(pid)` before `sig.term`.
/// Both assertions FAILED -- the sent log read `["KILL", "TERM"]`, so
/// `sent[0] == "TERM"` failed and the "no KILL before TERM" check failed
/// with it. Restored, and both passed.
#[test]
fn sigterm_is_always_sent_before_any_escalation() {
    // A session that ignores SIGTERM: the escalation case, which is the
    // only path that reaches SIGKILL at all.
    let sig = FakeSignal::new(usize::MAX);
    let out = stop(&sig, &confirmed()).expect("a stop that escalates still succeeds");
    assert_eq!(out.signal, Escalation::Killed);

    let sent = sig.sent();
    assert_eq!(
        sent.first(),
        Some(&"TERM"),
        "SIGTERM must be first: {sent:?}"
    );
    let term_at = sent.iter().position(|s| *s == "TERM").expect("a TERM");
    let kill_at = sent.iter().position(|s| *s == "KILL").expect("a KILL");
    assert!(term_at < kill_at, "SIGKILL preceded SIGTERM: {sent:?}");
    assert_eq!(sent, vec!["TERM", "KILL"], "exactly one of each: {sent:?}");
}

/// The common case never escalates at all.
///
/// A session that takes SIGTERM is gone before the grace period runs out,
/// and nothing further is sent -- so the transcript tail and `SessionEnd`
/// survive, which is the entire reason for the order.
#[test]
fn a_session_that_takes_sigterm_is_never_killed() {
    let sig = FakeSignal::new(1);
    let out = stop(&sig, &confirmed()).expect("a clean stop");
    assert_eq!(out.signal, Escalation::Terminated);
    assert_eq!(sig.sent(), vec!["TERM"]);
    assert_eq!(out.pid, PID);
    assert_eq!(out.session_id, "s1");
}

/// There is NO bare-SIGKILL path in the module.
///
/// Asserted over the source rather than over behaviour, because the risk
/// is a future edit adding a second entry point that skips the grace
/// period -- which no behavioural test of `stop` would ever see.
/// `Signaller::kill` may be reached from exactly one place, and that
/// place is after the wait loop in `stop`.
///
/// SABOTAGE: added a `pub fn force_kill<S: Signaller>(sig: &S, c:
/// &ConfirmedPid) { let _ = sig.kill(c.pid); }` to the module. The test
/// FAILED: it counted 2 `.kill(` call sites where 1 is allowed, and named
/// the second. Removed, and it passed.
#[test]
fn no_bare_sigkill_path_exists() {
    let src = include_str!("../stop.rs");
    // Comments dropped: this module's prose names SIGKILL repeatedly, and
    // without this the guard would fire on the sentences explaining it.
    // The same rule `invariants.rs` states for its own scanners.
    let code: String = src
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let call_sites = code.matches("sig.kill(").count();
    assert_eq!(
        call_sites, 1,
        "`Signaller::kill` must be reachable from exactly one place -- the escalation \
         after the grace period in `stop`. Found {call_sites} call site(s)."
    );

    // And that one call site is BELOW the wait loop, not above it.
    let loop_at = code
        .find("while waited < deadline_ms")
        .expect("the grace loop");
    let kill_at = code.find("sig.kill(").expect("the escalation");
    assert!(
        loop_at < kill_at,
        "the SIGKILL escalation must come after the grace period, not before it"
    );

    // The real signaller sends SIGTERM for `term` and SIGKILL for `kill`,
    // and not the other way round.
    assert!(code.contains("Self::send(pid, libc::SIGTERM)"));
    assert!(code.contains("Self::send(pid, libc::SIGKILL)"));
}

/// A process table that cannot be read is NOT "it is gone".
///
/// The fail-open `liveness` is written against, in this module's terms:
/// concluding a clean stop from a failed check would report a live
/// session as terminated.
#[test]
fn an_unreadable_process_table_does_not_read_as_a_clean_exit() {
    let sig = FakeSignal::unreadable();
    let out = stop(&sig, &confirmed()).expect("a stop");
    assert_eq!(
        out.signal,
        Escalation::Killed,
        "a liveness check that failed must not be taken as the process having exited"
    );
}

/// A SIGTERM that could not be delivered is an error, and nothing
/// escalates on top of it.
#[test]
fn a_failed_sigterm_stops_there() {
    struct Refuses;
    impl Signaller for Refuses {
        fn term(&self, _pid: u32) -> Result<(), String> {
            Err("Operation not permitted".into())
        }
        fn kill(&self, _pid: u32) -> Result<(), String> {
            panic!("SIGKILL must not follow a SIGTERM that was never delivered");
        }
        fn alive(&self, _pid: u32) -> Result<bool, String> {
            Ok(true)
        }
        fn wait(&self, _ms: u64) {}
    }
    let err = stop(&Refuses, &confirmed()).expect_err("a refused signal is an error");
    assert!(err.contains("could not signal"), "{err}");
}

// ---------------------------------------------------------------------
// MANDATED 3: the command is `Class::Local` -- no dispatch arm, and
// present in `DESKTOP_ONLY_WRAPPERS`.
// ---------------------------------------------------------------------

/// The stop must not cross the remote wire: a phone must not be able to
/// kill a desktop session.
///
/// Read out of the source rather than restated, for the reason
/// `surfaceGuard.test.ts` gives about copies of a security boundary: a
/// drifted copy goes on passing while describing a surface the desktop no
/// longer has.
///
/// The desktop's own `every_local_command_has_no_arm` covers the dispatch
/// half generically; this asserts it for THIS command by name, so
/// removing the row and the arm together could not quietly pass.
///
/// SABOTAGE: added `"claude_stop_session" => res(...)` to `call()` in
/// `remote/surface.rs`. This test FAILED on the no-arm assertion, and
/// `remote::surface`'s own `every_registered_command_has_exactly_one_class`
/// sibling `local commands must not have an arm` failed alongside it.
/// Removed, and both passed.
#[test]
fn the_stop_command_is_local_and_has_no_dispatch_arm() {
    let surface = include_str!("../../remote/surface.rs");
    assert!(
        surface.contains("(\"claude_stop_session\", Class::Local)"),
        "the stop must be classed Local in the desktop surface"
    );
    let body = {
        let start = surface.find("async fn call(").expect("call must exist");
        let end = surface[start..]
            .find("#[cfg(test)]")
            .map(|i| start + i)
            .unwrap_or(surface.len());
        &surface[start..end]
    };
    assert!(
        !body.contains("\"claude_stop_session\" =>"),
        "a Local command must have NO dispatch arm -- a phone must not be able to kill a \
         desktop session"
    );

    // The mobile crate's mirror row, so the two tables agree.
    let mobile = include_str!("../../../../src-mobile/src/surface.rs");
    assert!(
        mobile.contains("(\"claude_stop_session\", Class::Local)"),
        "the mobile mirror must class the stop Local too"
    );

    // And the frontend half: the wrapper is declared desktop-only.
    // `surfaceGuard.test.ts` asserts that list with `toEqual`, so an
    // omission fails CI there; this checks the entry exists at all from
    // the side that can see both files.
    let guard = include_str!("../../../../src/api/surfaceGuard.test.ts");
    assert!(
        guard.contains("\"claudeStopSession\""),
        "the wrapper must be named in DESKTOP_ONLY_WRAPPERS"
    );
}

// ---------------------------------------------------------------------
// MANDATED 4: a refusal is RECORDED as a refusal, not silently dropped.
// ---------------------------------------------------------------------

/// Every session considered gets a row, and a refusal says which refusal.
///
/// `cleanup::propose`'s rule: an entry that keeps being passed over is
/// something the user should be able to see, not a silent gap in the
/// list. A stop that refuses because the pid was reused is the single
/// most important thing this feature can tell a user, and dropping it
/// would leave them pressing a button that appears to do nothing.
///
/// SABOTAGE: changed `propose`'s `Err(r)` arm to `Err(_) => continue`, so
/// refusals vanished from the list. The test FAILED: the list held 1 row
/// where 3 were expected, and the `refused` lookups panicked on `None`.
/// Restored, and it passed.
#[test]
fn a_refusal_is_recorded_as_a_refusal() {
    // Three sessions: one live, one whose pid was reused, one absent.
    let mut r = registry();
    r.entries.insert(
        "s2".into(),
        RegistryEntry {
            pid: 2222,
            session_id: "s2".into(),
            proc_start: Some(PROC_START.into()),
            ..Default::default()
        },
    );

    // `s1` and `s2` share a probe answer here, so to separate them the
    // probe reports the DRIFTED time and `s1` is checked with its own.
    let reused = propose(
        &FakeProbe(Ok(Some(PROC_START_EPOCH + 8 * 3600))),
        &r,
        &["s1".into(), "s2".into(), "s3".into()],
        no_evidence,
    );
    assert_eq!(reused.len(), 3, "every session considered gets a row");

    let by_id = |id: &str| -> StopProposal {
        reused
            .iter()
            .find(|p| p.session_id == id)
            .expect("a row per session")
            .clone()
    };

    for id in ["s1", "s2"] {
        let row = by_id(id);
        assert_eq!(row.action, "refused", "{id}");
        assert!(
            matches!(row.refusal, Some(Refusal::PidReused { .. })),
            "{id}: {:?}",
            row.refusal
        );
        assert!(row.pid.is_none(), "{id} must not carry a pid it refused");
        assert!(
            row.why.as_deref().unwrap_or_default().contains("reused"),
            "{id}"
        );
    }

    // The session the registry never mentioned: also a row, and a
    // DIFFERENT refusal -- the two have different remedies.
    let s3 = by_id("s3");
    assert_eq!(s3.action, "refused");
    assert!(
        matches!(s3.refusal, Some(Refusal::NotRunning { .. })),
        "{:?}",
        s3.refusal
    );

    // And the live one is proposed, with its confirmed pid shown.
    let live = propose(
        &FakeProbe(Ok(Some(PROC_START_EPOCH))),
        &registry(),
        &["s1".into()],
        no_evidence,
    );
    assert_eq!(live[0].action, "proposed");
    assert_eq!(live[0].pid, Some(PID));
    assert!(live[0].refusal.is_none());
}

/// The blast-radius cap is a refusal too, not a truncation.
///
/// `cleanup::propose` caps a run and this does the same, but the ones
/// pushed out are RECORDED -- a list that silently stopped at five would
/// tell a user with six stuck sessions that they have five.
#[test]
fn the_per_run_cap_refuses_rather_than_truncating() {
    let ids: Vec<String> = (0..MAX_PER_RUN + 3).map(|i| format!("s{i}")).collect();
    let rows = propose(
        &FakeProbe(Ok(None)),
        &Registry::default(),
        &ids,
        no_evidence,
    );
    assert_eq!(rows.len(), ids.len(), "nothing is dropped");

    let capped: Vec<&StopProposal> = rows
        .iter()
        .filter(|p| matches!(p.refusal, Some(Refusal::CapReached { .. })))
        .collect();
    assert_eq!(capped.len(), 3, "the three past the cap say so");
    assert!(capped[0]
        .why
        .as_deref()
        .unwrap_or_default()
        .contains("at a time"));
    assert!(rows.iter().all(|p| p.action == "refused"));
}

/// Nothing in a proposal is a recommendation.
///
/// `health::runaway`'s Notice-vs-Alert split, applied here: a stuck
/// session is an INDICATOR. The evidence struct must never grow a field
/// that reads as the app advising a kill, because the UI renders it
/// verbatim and would inherit the advice.
#[test]
fn a_proposal_carries_evidence_and_never_a_recommendation() {
    let json = serde_json::to_string(&StopProposal {
        session_id: "s1".into(),
        action: "proposed".into(),
        pid: Some(PID),
        refusal: None,
        why: Some("pid 14779 is running".into()),
        evidence: StopEvidence {
            name: Some("widget-c3".into()),
            cwd: Some("/Users/acme/code/widget".into()),
            status: Some("busy".into()),
            uptime_secs: Some(7_200),
            auto_compactions: Some(4),
            last_turn: Some("Running the test suite".into()),
        },
    })
    .unwrap();

    for banned in ["recommend", "should_stop", "advice", "suggest"] {
        assert!(
            !json.contains(banned),
            "a proposal must not advise: {banned}"
        );
    }
    // The evidence the issue requires is actually carried.
    assert!(json.contains("last_turn"));
    assert!(json.contains("auto_compactions"));
    assert!(json.contains("uptime_secs"));
}

/// `Escalation` crosses to TypeScript as a discriminated string, so the
/// UI can say which signal ended it rather than guessing.
#[test]
fn the_outcome_states_which_signal_ended_it() {
    let t = serde_json::to_string(&Escalation::Terminated).unwrap();
    let k = serde_json::to_string(&Escalation::Killed).unwrap();
    assert_eq!(t, "\"terminated\"");
    assert_eq!(k, "\"killed\"");
}

/// A `ConfirmedPid` cannot be built from a rendered row.
///
/// The type's fields are private and `confirm` is the only constructor,
/// which is what makes "re-derived NOW" a property of the code rather
/// than a convention. Asserted over the source because a compile-time
/// guarantee has no runtime shape to test.
#[test]
fn a_confirmed_pid_has_no_constructor_but_confirm() {
    let src = include_str!("../stop.rs");
    let code: String = src
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'))
        })
        .collect::<Vec<_>>()
        .join("\n");
    // `Ok(ConfirmedPid {` is a CONSTRUCTION. The struct definition and
    // the `impl` block both spell `ConfirmedPid {` too, so the bare
    // count would not distinguish them.
    assert_eq!(
        code.matches("Ok(ConfirmedPid {").count(),
        1,
        "`ConfirmedPid` must be constructed in exactly one place -- `confirm`"
    );
    // None of ITS fields are public, so no caller can assemble one.
    // Scoped to the struct body: `StopOutcome` legitimately has a public
    // `pid`, and a whole-file search would fire on that instead.
    let at = code.find("pub struct ConfirmedPid {").expect("the struct");
    let body = &code[at..][..code[at..].find('}').expect("its closing brace")];
    for field in ["pub pid", "pub session_id", "pub started_at"] {
        assert!(
            !body.contains(field),
            "`ConfirmedPid` must keep its fields private: {field}"
        );
    }
}
