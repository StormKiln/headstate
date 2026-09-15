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

use crate::worktrees::UpdateAllReport;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A run in flight, or the outcome of the last one to finish.
///
/// Both live in the same slot, on `RunState`'s reasoning: a client asking
/// "what happened to my run" gets one answer whether it is still going or
/// ended while the client was away, and does not have to ask twice.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum UpdateAllState {
    /// Still going. `done` counts finished repositories.
    Running { done: usize, total: usize },
    /// Ended, with the same report the command returns. A client that
    /// missed the event reads it here instead.
    Done { report: Box<UpdateAllReport> },
}

/// The one Update All run this process may have going.
///
/// `Option` rather than a map: see the module doc. One slot, and it holds
/// the last outcome after the run ends so a returning client can read it.
#[derive(Default)]
pub struct UpdateAllRuns(Mutex<Option<Entry>>);

pub struct Entry {
    /// Set by [`UpdateAllRuns::cancel`], read between repositories by the
    /// run itself.
    stop: Arc<AtomicBool>,
    state: UpdateAllState,
}

impl UpdateAllRuns {
    /// Claim the slot for a new run.
    ///
    /// `Err` when one is already going: two runs pulling the same 45
    /// repositories is not something to discover afterwards. The message
    /// is user-facing, since this is what the command returns.
    pub fn start(&self, total: usize) -> Result<Arc<AtomicBool>, String> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = slot.as_ref() {
            if matches!(e.state, UpdateAllState::Running { .. }) {
                return Err("an update run is already going".into());
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        *slot = Some(Entry {
            stop: stop.clone(),
            state: UpdateAllState::Running { done: 0, total },
        });
        Ok(stop)
    }

    /// Record progress. Ignored once the run has ended, so a late frame
    /// cannot resurrect a finished run as `Running`.
    pub fn progress(&self, done: usize, total: usize) {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = slot.as_mut() {
            if matches!(e.state, UpdateAllState::Running { .. }) {
                e.state = UpdateAllState::Running { done, total };
            }
        }
    }

    /// Record how a run ended. The entry stays, so a client that was away
    /// can still read it.
    pub fn finished(&self, report: UpdateAllReport) {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = slot.as_mut() {
            e.state = UpdateAllState::Done {
                report: Box::new(report),
            };
        }
    }

    /// Ask the run to stop after the repository it is on.
    ///
    /// `Err` when there is nothing running, rather than succeeding
    /// silently: a Cancel that appears to work on a run that already
    /// finished would be its own small lie.
    pub fn cancel(&self) -> Result<(), String> {
        let slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
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
        let slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref().map(|e| e.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktrees::{RepoUpdateOutcome, UpdateResult};

    fn report() -> UpdateAllReport {
        UpdateAllReport {
            outcomes: vec![RepoUpdateOutcome {
                path: "/repo".into(),
                result: UpdateResult::AlreadyLevel,
            }],
            cancelled: false,
            timed_out: false,
            unreadable: vec![],
        }
    }

    #[test]
    fn a_second_run_is_refused_while_one_is_going() {
        let runs = UpdateAllRuns::default();
        runs.start(45).expect("the first run claims the slot");
        assert!(
            runs.start(45).is_err(),
            "two runs would put two git pulls in the same repository"
        );
    }

    #[test]
    fn a_new_run_may_start_once_the_last_one_finished() {
        let runs = UpdateAllRuns::default();
        runs.start(2).expect("first");
        runs.finished(report());
        runs.start(2).expect("the slot is free again");
    }

    #[test]
    fn cancel_sets_the_flag_the_run_reads() {
        let runs = UpdateAllRuns::default();
        let stop = runs.start(3).expect("start");
        assert!(!stop.load(Ordering::SeqCst));
        runs.cancel().expect("cancel");
        assert!(
            stop.load(Ordering::SeqCst),
            "cancel must set the flag the loop checks between repositories"
        );
    }

    #[test]
    fn cancelling_nothing_is_an_error_rather_than_a_quiet_success() {
        let runs = UpdateAllRuns::default();
        assert!(runs.cancel().is_err(), "there is no run to stop");
        runs.start(1).expect("start");
        runs.finished(report());
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
        runs.start(1).expect("start");
        runs.progress(1, 1);
        runs.finished(report());
        match runs.state() {
            Some(UpdateAllState::Done { report }) => {
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
        runs.start(2).expect("start");
        runs.finished(report());
        runs.progress(1, 2);
        assert!(
            matches!(runs.state(), Some(UpdateAllState::Done { .. })),
            "a finished run stays finished"
        );
    }

    #[test]
    fn a_process_that_has_run_nothing_says_so() {
        assert!(UpdateAllRuns::default().state().is_none());
    }
}
