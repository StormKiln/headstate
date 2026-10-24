//! Whether an Update All run is going, how far it has got, and how the
//! last one ended (#1016).
//!
//! # Why a registry rather than a boolean
//!
//! `packages/runs.rs` states three gaps a registry closed, and all three
//! apply here:
//!
//! > **Nothing could stop a run.** The spawned task's `JoinHandle` was
//! > dropped, so a run that was started by mistake -- wrong branch, wrong
//! > selection -- ran to completion regardless.
//! > **Nothing stopped a second run** starting on a repository already
//! > updating...
//! > **A phone could not learn how a run ended.** Progress and completion
//! > are events, and a suspended app holds no event stream at all.
//!
//! Mapped onto Update All:
//!
//! 1. **Started by mistake.** The wrong network is the dominant case and
//!    the one that produces the worst case. MEASURED on the reporting
//!    machine: 45 repositories, `git fetch --dry-run` at 25ms--1055ms, so
//!    ~40s when every remote is reachable -- and 45 x `GIT_TIMEOUT` (30s)
//!    **= 22 minutes** on a dropped VPN or a captive portal, where every
//!    individual call is perfectly within its own bound. The user
//!    realises within seconds and must be able to stop.
//! 2. **A second run.** Two concurrent runs would put two `git pull`
//!    invocations in the same repository, contending on `index.lock`.
//! 3. **A phone holds no event stream while suspended**
//!    (`src-mobile/src/background.rs` is explicit). A phone that starts a
//!    40-second run and locks the screen misses every frame including the
//!    terminal one. A registry that can be ASKED is the difference
//!    between a feature a phone can use and one it can only start.
//!
//! # The claim covers the RUN, not one repository
//!
//! This is the one place this registry's shape differs from
//! `UpdateRuns`, which is keyed by repository path because that is what a
//! package run acts on. Update All acts on the whole scan root, so a
//! per-repository claim would let two runs interleave across 45
//! repositories and make both reports wrong. There is one slot.

use crate::worktrees::{RepoUpdateOutcome, UpdateAllReport, UpdateResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A run in flight, or the outcome of the last one to finish.
///
/// Both live in the same slot, on `RunState`'s reasoning: a client asking
/// "what happened to my run" gets one answer whether it is still going or
/// ended while the client was away, and does not have to ask twice.
///
/// `run` numbers the runs this process has started, so a client can tell
/// the report of the run it just started from the one that was already on
/// record when it clicked (#1946).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum UpdateAllState {
    /// Still going. `done` counts finished repositories, and `landed`
    /// names them, so the overview can re-read each row as its repository
    /// finishes rather than every row at the end. Paths are fine here:
    /// this is a read the client asks for, as the report is, not a
    /// progress event.
    Running {
        run: u64,
        done: usize,
        total: usize,
        landed: Vec<String>,
    },
    /// Ended, with the same report the command returns. A client that
    /// missed the event reads it here instead.
    Done {
        run: u64,
        report: Box<UpdateAllReport>,
    },
    /// Ended WITHOUT the run reporting how it finished: the blocking task
    /// failed to join, or the command was dropped mid-run (#1947).
    ///
    /// Not collapsed into `Done`, because the report here was assembled by
    /// the registry from the outcomes the run recorded before it died, and
    /// not collapsed into an error string either, because those outcomes
    /// are real: repositories that were fast-forwarded stay
    /// fast-forwarded, and a user must be told which. The rest of the run's
    /// repositories are `NotAttempted`.
    Failed {
        run: u64,
        reason: String,
        report: Box<UpdateAllReport>,
    },
}

/// The one Update All run this process may have going.
///
/// `Option` rather than a map: see the module doc. One slot, and it holds
/// the last outcome after the run ends so a returning client can read it.
#[derive(Default)]
pub struct UpdateAllRuns {
    slot: Mutex<Option<Entry>>,
    /// The last run number handed out.
    runs: std::sync::atomic::AtomicU64,
}

struct Entry {
    /// Set by [`UpdateAllRuns::cancel`], read between repositories by the
    /// run itself.
    stop: Arc<AtomicBool>,
    /// The repositories the run was started over, in its order.
    paths: Vec<String>,
    /// What the run has done so far, one entry per finished repository, in
    /// `paths`' order. Kept HERE rather than only inside the run, so that
    /// a run which dies still leaves what it applied behind (#1947).
    applied: Vec<RepoUpdateOutcome>,
    state: UpdateAllState,
}

/// The claim on the slot for one run.
///
/// Its `Drop` is what guarantees the slot never stays `Running` after the
/// run is gone (#1947). Before it, the command's `?` on a failed join
/// returned before `finished` was ever called, and every later Update All
/// was refused as "already going" until the app restarted.
#[must_use = "dropping the claim ends the run as failed"]
pub struct RunClaim<'a> {
    runs: &'a UpdateAllRuns,
    stop: Arc<AtomicBool>,
    settled: bool,
}

impl RunClaim<'_> {
    /// The flag the run reads between repositories.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Record how the run ended.
    pub fn finished(mut self, report: UpdateAllReport) {
        self.settled = true;
        self.runs.finished(report);
    }

    /// Record that the run ended without a report, keeping what it had
    /// already applied. `scope` carries what the run did not produce
    /// itself -- the scan's shortfall and the scope's own outcomes -- and
    /// its outcomes are appended after the run's. Returns the report that
    /// was recorded.
    pub fn failed(mut self, reason: String, scope: UpdateAllReport) -> UpdateAllReport {
        self.settled = true;
        self.runs.fail(reason, scope)
    }
}

impl Drop for RunClaim<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.runs.fail(
                "the update run stopped without reporting how it ended".into(),
                UpdateAllReport {
                    diagnostics: Vec::new(),
                    outcomes: Vec::new(),
                    cancelled: false,
                    timed_out: false,
                    unreadable: Vec::new(),
                },
            );
        }
    }
}

impl UpdateAllRuns {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Entry>> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Claim the slot for a new run over `paths`.
    ///
    /// `Err` when one is already going: two runs pulling the same 45
    /// repositories is not something to discover afterwards. The message
    /// is user-facing, since this is what the command returns.
    pub fn start(&self, paths: &[String]) -> Result<RunClaim<'_>, String> {
        let mut slot = self.lock();
        if let Some(e) = slot.as_ref() {
            if matches!(e.state, UpdateAllState::Running { .. }) {
                return Err("an update run is already going".into());
            }
        }
        let run = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let stop = Arc::new(AtomicBool::new(false));
        *slot = Some(Entry {
            stop: stop.clone(),
            paths: paths.to_vec(),
            applied: Vec::with_capacity(paths.len()),
            state: UpdateAllState::Running {
                run,
                done: 0,
                total: paths.len(),
                landed: Vec::new(),
            },
        });
        Ok(RunClaim {
            runs: self,
            stop,
            settled: false,
        })
    }

    /// Record one finished repository. Ignored once the run has ended, so
    /// a late frame cannot resurrect a finished run as `Running`.
    pub fn outcome_landed(&self, outcome: RepoUpdateOutcome) {
        let mut slot = self.lock();
        if let Some(e) = slot.as_mut() {
            if let UpdateAllState::Running { run, total, .. } = e.state {
                e.applied.push(outcome);
                e.state = UpdateAllState::Running {
                    run,
                    done: e.applied.len(),
                    total,
                    landed: e.applied.iter().map(|o| o.path.clone()).collect(),
                };
            }
        }
    }

    /// Record how a run ended. The entry stays, so a client that was away
    /// can still read it.
    fn finished(&self, report: UpdateAllReport) {
        let mut slot = self.lock();
        if let Some(e) = slot.as_mut() {
            if let UpdateAllState::Running { run, .. } = e.state {
                e.state = UpdateAllState::Done {
                    run,
                    report: Box::new(report),
                };
            }
        }
    }

    /// End the run as failed, from what it recorded. Partial is not
    /// nothing: every outcome already applied is kept, and every
    /// repository the run never reached is `NotAttempted` rather than
    /// absent.
    fn fail(&self, reason: String, scope: UpdateAllReport) -> UpdateAllReport {
        let mut slot = self.lock();
        let Some(e) = slot.as_mut() else {
            return scope;
        };
        let UpdateAllState::Running { run, .. } = e.state else {
            return scope;
        };
        let mut outcomes = std::mem::take(&mut e.applied);
        outcomes.extend(
            e.paths
                .iter()
                .skip(outcomes.len())
                .map(|path| RepoUpdateOutcome {
                    path: path.clone(),
                    result: UpdateResult::NotAttempted,
                }),
        );
        outcomes.extend(scope.outcomes);
        let report = UpdateAllReport { outcomes, ..scope };
        e.state = UpdateAllState::Failed {
            run,
            reason,
            report: Box::new(report.clone()),
        };
        report
    }

    /// Ask the run to stop after the repository it is on.
    ///
    /// `Err` when there is nothing running, rather than succeeding
    /// silently: a Cancel that appears to work on a run that already
    /// finished would be its own small lie.
    pub fn cancel(&self) -> Result<(), String> {
        let slot = self.lock();
        match slot.as_ref() {
            Some(e) if matches!(e.state, UpdateAllState::Running { .. }) => {
                e.stop.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err("no update run is going".into()),
        }
    }

    /// What happened to the run, or `None` if this process has never had
    /// one.
    pub fn state(&self) -> Option<UpdateAllState> {
        let slot = self.lock();
        slot.as_ref().map(|e| e.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> UpdateAllReport {
        UpdateAllReport {
            outcomes: vec![RepoUpdateOutcome {
                path: "/repo".into(),
                result: UpdateResult::AlreadyLevel,
            }],
            cancelled: false,
            timed_out: false,
            unreadable: vec![],
            diagnostics: vec![],
        }
    }

    fn paths(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("repo-{i}")).collect()
    }

    fn outcome(path: &str, result: UpdateResult) -> RepoUpdateOutcome {
        RepoUpdateOutcome {
            path: path.into(),
            result,
        }
    }

    #[test]
    fn a_second_run_is_refused_while_one_is_going() {
        let runs = UpdateAllRuns::default();
        let _claim = runs
            .start(&paths(45))
            .expect("the first run claims the slot");
        assert!(
            runs.start(&paths(45)).is_err(),
            "two runs would put two git pulls in the same repository"
        );
    }

    #[test]
    fn a_new_run_may_start_once_the_last_one_finished() {
        let runs = UpdateAllRuns::default();
        runs.start(&paths(2)).expect("first").finished(report());
        let _claim = runs.start(&paths(2)).expect("the slot is free again");
    }

    #[test]
    fn cancel_sets_the_flag_the_run_reads() {
        let runs = UpdateAllRuns::default();
        let claim = runs.start(&paths(3)).expect("start");
        let stop = claim.stop_flag();
        assert!(!stop.load(Ordering::SeqCst));
        runs.cancel().expect("cancel");
        assert!(
            stop.load(Ordering::SeqCst),
            "cancel must set the flag the loop checks between repositories"
        );
        claim.finished(report());
    }

    #[test]
    fn cancelling_nothing_is_an_error_rather_than_a_quiet_success() {
        let runs = UpdateAllRuns::default();
        assert!(runs.cancel().is_err(), "there is no run to stop");
        runs.start(&paths(1)).expect("start").finished(report());
        assert!(
            runs.cancel().is_err(),
            "a Cancel that appears to work on a finished run is its own small lie"
        );
    }

    /// The resume read: the outcome survives the run, which is what makes
    /// this usable from a phone that was asleep for the whole thing.
    #[test]
    fn the_outcome_can_be_read_back_after_the_run_ends() {
        let runs = UpdateAllRuns::default();
        let claim = runs.start(&paths(1)).expect("start");
        runs.outcome_landed(outcome("repo-0", UpdateResult::AlreadyLevel));
        match runs.state() {
            Some(UpdateAllState::Running {
                done: 1,
                total: 1,
                landed,
                ..
            }) => assert_eq!(landed, vec!["repo-0".to_string()]),
            other => panic!("a running run names what has landed: {other:?}"),
        }
        claim.finished(report());
        match runs.state() {
            Some(UpdateAllState::Done { report, .. }) => {
                assert_eq!(report.outcomes.len(), 1);
            }
            other => {
                panic!("the report must be readable after the event stream is gone: {other:?}")
            }
        }
    }

    #[test]
    fn a_late_progress_frame_cannot_resurrect_a_finished_run() {
        let runs = UpdateAllRuns::default();
        runs.start(&paths(2)).expect("start").finished(report());
        runs.outcome_landed(outcome("repo-1", UpdateResult::AlreadyLevel));
        assert!(
            matches!(runs.state(), Some(UpdateAllState::Done { .. })),
            "a finished run stays finished"
        );
    }

    #[test]
    fn a_process_that_has_run_nothing_says_so() {
        assert!(UpdateAllRuns::default().state().is_none());
    }

    /// #1946: a client compares run numbers to tell the report of the run
    /// it started from the one already on record when it clicked.
    #[test]
    fn each_run_gets_its_own_number() {
        let runs = UpdateAllRuns::default();
        runs.start(&paths(1)).expect("first").finished(report());
        let first = match runs.state() {
            Some(UpdateAllState::Done { run, .. }) => run,
            other => panic!("{other:?}"),
        };
        let _claim = runs.start(&paths(1)).expect("second");
        match runs.state() {
            Some(UpdateAllState::Running { run, .. }) => assert!(run > first),
            other => panic!("{other:?}"),
        }
    }

    /// #1947: the run panics after one repository. The registry must not
    /// stay `Running` (which refused every later run until a restart), and
    /// the repository it already updated must still be reported.
    #[test]
    fn a_run_that_dies_frees_the_slot_and_keeps_what_it_applied() {
        let runs = UpdateAllRuns::default();
        let claim = runs.start(&paths(3)).expect("start");
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runs.outcome_landed(outcome(
                "repo-0",
                UpdateResult::Updated {
                    message: "Fast-forward".into(),
                },
            ));
            panic!("the blocking task panicked");
        }));
        assert!(died.is_err());
        let mut scope = report();
        scope.outcomes = vec![outcome(
            "gone",
            UpdateResult::Skipped {
                reason: "no longer found".into(),
            },
        )];
        scope.unreadable = vec!["/locked".into()];
        let recorded = claim.failed("the update run failed to finish".into(), scope);
        assert_eq!(recorded.unreadable, vec!["/locked".to_string()]);

        match runs.state() {
            Some(UpdateAllState::Failed { reason, report, .. }) => {
                assert!(reason.contains("failed to finish"), "{reason}");
                let results: Vec<_> = report
                    .outcomes
                    .iter()
                    .map(|o| (o.path.as_str(), &o.result))
                    .collect();
                assert!(
                    matches!(results[0], ("repo-0", UpdateResult::Updated { .. })),
                    "the applied update is not discarded: {results:?}"
                );
                assert_eq!(results[1], ("repo-1", &UpdateResult::NotAttempted));
                assert_eq!(results[2], ("repo-2", &UpdateResult::NotAttempted));
                assert!(matches!(results[3], ("gone", UpdateResult::Skipped { .. })));
                assert_eq!(results.len(), 4);
            }
            other => panic!("a dead run must not read as running: {other:?}"),
        }
        let _next = runs
            .start(&paths(1))
            .expect("a dead run must not refuse every later one");
    }

    /// #1947, the guard itself: a claim dropped without being settled --
    /// the command's future dropped, or an early return nobody wrote a
    /// `failed` for -- still ends the run.
    #[test]
    fn a_claim_dropped_unsettled_ends_the_run() {
        let runs = UpdateAllRuns::default();
        {
            let _claim = runs.start(&paths(2)).expect("start");
            runs.outcome_landed(outcome("repo-0", UpdateResult::AlreadyLevel));
        }
        match runs.state() {
            Some(UpdateAllState::Failed { report, .. }) => {
                assert_eq!(report.outcomes.len(), 2);
                assert_eq!(report.outcomes[0].result, UpdateResult::AlreadyLevel);
                assert_eq!(report.outcomes[1].result, UpdateResult::NotAttempted);
            }
            other => panic!("an abandoned run must not stay running: {other:?}"),
        }
        let _next = runs.start(&paths(1)).expect("the slot is free");
    }
}
