//! Desktop GitLab queues run independently of GitHub and of the webview.
//! Each queue owns the page loader's deadline; no outer timeout discards its
//! partial result. Only published receipts can advance notification history.
use super::queues::{self, FetchedList, MergeRequest};
use crate::{
    github::model::CiState,
    identity::{PrIdentity, Source},
    poll::NotifyPrefs,
    source_poll,
    store::{open_db, source_cache::Coverage, CachedList},
};
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

#[derive(Clone, PartialEq, Eq)]
struct Selection {
    source: Option<Source>,
    revision: u64,
}

/// Manage `Arc<Control>` even without GitHub authentication. Restore the
/// desktop's source selection before spawning; a phone's selection must not
/// change this control. Tray refresh, focus and cadence changes call `wake`.
pub struct Control {
    selection: Mutex<Selection>,
    wake: Notify,
}

impl Control {
    pub fn new(source: Option<Source>) -> Self {
        Self {
            selection: Mutex::new(Selection {
                source,
                revision: 0,
            }),
            wake: Notify::new(),
        }
    }

    pub fn select(&self, source: Option<Source>) {
        let mut selected = self.selection.lock().unwrap_or_else(|e| e.into_inner());
        if selected.source != source {
            selected.source = source;
            selected.revision = selected.revision.wrapping_add(1);
        }
        drop(selected);
        self.wake();
    }

    pub fn wake(&self) {
        // A wake during a fetch remains a permit for the next tick.
        self.wake.notify_one();
    }

    fn selected(&self) -> Selection {
        self.selection
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn is_current(&self, selection: &Selection) -> bool {
        self.selected() == *selection
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    AuthoredArrival,
    ReviewArrival,
    ReviewDraftReady,
    Conflict,
    CiFailed,
}

impl NoticeKind {
    fn enabled(self, prefs: &NotifyPrefs) -> bool {
        prefs.enabled
            && match self {
                Self::AuthoredArrival => prefs.new_pr,
                Self::ReviewArrival | Self::ReviewDraftReady => prefs.ready_to_review,
                Self::Conflict => prefs.conflicted,
                Self::CiFailed => prefs.ci_failed,
            }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::AuthoredArrival => "appeared in your authored merge requests",
            Self::ReviewArrival => "appeared in your GitLab review queue",
            Self::ReviewDraftReady => "is no longer a draft in your GitLab review queue",
            Self::Conflict => "GitLab reports merge conflicts",
            Self::CiFailed => "CI is failing",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Notice {
    identity: PrIdentity,
    title: String,
    kind: NoticeKind,
}

#[derive(Default)]
struct Baseline {
    source: Option<Source>,
    list: Option<CachedList>,
    rows: Option<HashMap<PrIdentity, MergeRequest>>,
    viewer: Option<String>,
    complete: bool,
}

impl Baseline {
    fn observe(&mut self, source: &Source, list: CachedList, result: &FetchedList) -> Vec<Notice> {
        // The scheduler supplies one source/list per baseline; keep the key
        // here too so a host change can never compare colliding project/IIDs.
        if self.source.as_ref() != Some(source) || self.list != Some(list) {
            *self = Self::default();
            self.source = Some(source.clone());
            self.list = Some(list);
        }
        if self.viewer != result.viewer {
            self.rows = None;
            self.complete = false;
            self.viewer = result.viewer.clone();
        }
        let mut notices = Vec::new();
        if let Some(before) = &self.rows {
            for mr in &result.mrs {
                if &mr.source != source {
                    continue;
                }
                let identity = mr.identity();
                let was = before.get(&identity);
                let kind = match list {
                    CachedList::Reviewing if !mr.is_draft => match was {
                        Some(was) if was.is_draft => Some(NoticeKind::ReviewDraftReady),
                        None if self.complete => Some(NoticeKind::ReviewArrival),
                        _ => None,
                    },
                    CachedList::Authored => match was {
                        None if self.complete && !mr.is_draft => Some(NoticeKind::AuthoredArrival),
                        Some(was)
                            if mr.head_oid.is_some()
                                && mr.head_oid == was.head_oid
                                && mr.ci == Some(CiState::Failure)
                                && was.ci.is_some_and(|ci| ci != CiState::Failure) =>
                        {
                            Some(NoticeKind::CiFailed)
                        }
                        // A different blocker (or a pending/unknown status)
                        // does not prove the old head was free of conflicts.
                        // GitLab's detailed status, not GitHub merge readiness:
                        // https://docs.gitlab.com/api/merge_requests/#merge-status
                        Some(was)
                            if mr.detailed_merge_status.as_deref() == Some("conflict")
                                && was.detailed_merge_status.as_deref() == Some("mergeable") =>
                        {
                            Some(NoticeKind::Conflict)
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(kind) = kind {
                    notices.push(Notice {
                        identity,
                        title: mr.title.clone(),
                        kind,
                    });
                }
            }
        }
        self.rows = Some(
            result
                .mrs
                .iter()
                .filter(|mr| &mr.source == source)
                .map(|mr| (mr.identity(), mr.clone()))
                .collect(),
        );
        self.complete = matches!(result.coverage, Coverage::Complete)
            && result.mrs.iter().all(|mr| &mr.source == source);
        notices
    }
}

fn notify(app: &AppHandle, notice: Notice, prefs: &NotifyPrefs) {
    use tauri_plugin_notification::NotificationExt;
    if !notice.kind.enabled(prefs) || !crate::poll::notification_allowed(app) {
        return;
    }
    let identity = notice.identity;
    let body = format!(
        "GitLab {}: {}!{} — {}",
        identity.source.host,
        identity.repo,
        identity.number,
        notice.kind.reason()
    );
    if let Err(error) = app
        .notification()
        .builder()
        .title(notice.title)
        .body(body)
        .show()
    {
        log::warn!("could not show GitLab notification: {error}");
    }
}

async fn preferences(app: &AppHandle) -> NotifyPrefs {
    let Ok(dir) = app.path().app_data_dir() else {
        return NotifyPrefs::default();
    };
    tauri::async_runtime::spawn_blocking(move || {
        open_db(&dir.join("headstate.db"))
            .ok()
            .and_then(|conn| {
                crate::store::settings::get(&conn, crate::store::settings::keys::NOTIFY_PREFS)
                    .ok()
                    .flatten()
            })
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

async fn persist(app: &AppHandle, source: &Source, list: CachedList, result: &FetchedList) {
    let saved = match app.path().app_data_dir() {
        Ok(dir) => {
            let source = source.clone();
            let receipt = result.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let conn = open_db(&dir.join("headstate.db"))?;
                crate::store::source_cache::save_gitlab_snapshot(
                    &conn,
                    &source,
                    list,
                    &receipt.mrs,
                    &receipt.coverage,
                )
            })
            .await
            .is_ok_and(|result| result.is_ok())
        }
        Err(_) => false,
    };
    if !saved {
        let _ = app.emit(
            "store-error",
            "The GitLab list could not be saved for offline use.",
        );
    }
}

async fn fetch_and_persist(
    app: &AppHandle,
    source: &Source,
    list: CachedList,
) -> Option<(source_poll::Publication, FetchedList)> {
    let attempt = source_poll::begin(app, source.clone(), list).await;
    // The accumulating loader enforces FETCH_TIMEOUT itself. Wrapping it in
    // timeout/select would throw away pages that already arrived.
    match queues::fetch(source, list, |receipt| async {
        if let Some(publication) = source_poll::success_publication(app, &attempt).await {
            source_poll::complete_gitlab(app, publication, Ok(receipt));
        }
    })
    .await
    {
        Ok(receipt) => {
            let publication = source_poll::success_publication(app, &attempt).await?;
            persist(app, source, list, &receipt).await;
            Some((publication, receipt))
        }
        Err(error) => {
            if let Some(publication) = source_poll::publication(app, &attempt).await {
                let failure = source_poll::Failure {
                    message: error.to_string(),
                    transient: error.transient(),
                    not_asked: matches!(
                        error,
                        queues::QueueError::MissingCli | queues::QueueError::UnsupportedHost
                    ),
                };
                source_poll::complete_gitlab(app, publication, Err(failure));
            }
            None
        }
    }
}

/// Load preferences after the network and persistence work settles. A user
/// disabling alerts while a fetch is pending must not receive its MR titles.
async fn notification_batch<T, P: Future<Output = NotifyPrefs>>(
    prepared: impl Future<Output = Option<T>>,
    read_preferences: impl FnOnce() -> P,
) -> Option<(T, NotifyPrefs)> {
    let prepared = prepared.await?;
    let prefs = read_preferences().await;
    Some((prepared, prefs))
}

async fn poll_queue(
    app: &AppHandle,
    control: &Control,
    selection: &Selection,
    source: &Source,
    list: CachedList,
    baseline: &mut Baseline,
) {
    if !control.is_current(selection) {
        return;
    }
    let Some(((publication, receipt), prefs)) =
        notification_batch(fetch_and_persist(app, source, list), || preferences(app)).await
    else {
        return;
    };
    if control.is_current(selection) {
        for notice in baseline.observe(source, list, &receipt) {
            notify(app, notice, &prefs);
        }
    }
    // The permit holds until both the persisted receipt and terminal
    // event are published. Newer foreground successes win atomically.
    source_poll::complete_gitlab(app, publication, Ok(receipt));
}

/// Start once at desktop setup, independently of whether a GitHub client exists.
/// Both queue loaders run concurrently, each with its own bounded page budget.
/// Cadence uses the configured focused interval and its background multiplier.
pub fn spawn(
    app: AppHandle,
    control: Arc<Control>,
    focused: Arc<AtomicBool>,
    interval_secs: Arc<AtomicU64>,
) {
    tauri::async_runtime::spawn(async move {
        let mut authored = Baseline::default();
        let mut reviewing = Baseline::default();
        let mut baseline_revision = None;
        loop {
            let selection = control.selected();
            let Some(source) = selection.source.as_ref() else {
                control.wake.notified().await;
                continue;
            };
            if baseline_revision != Some(selection.revision) {
                authored = Baseline::default();
                reviewing = Baseline::default();
                baseline_revision = Some(selection.revision);
            }
            tokio::join!(
                poll_queue(
                    &app,
                    &control,
                    &selection,
                    source,
                    CachedList::Authored,
                    &mut authored,
                ),
                poll_queue(
                    &app,
                    &control,
                    &selection,
                    source,
                    CachedList::Reviewing,
                    &mut reviewing,
                ),
            );
            tokio::select! {
                _ = control.wake.notified() => {}
                _ = tokio::time::sleep(crate::poll::interval_for_secs(
                    focused.load(Ordering::Relaxed),
                    interval_secs.load(Ordering::Relaxed),
                )) => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Provider;

    fn source(host: &str) -> Source {
        Source {
            provider: Provider::Gitlab,
            host: host.into(),
        }
    }

    fn mr(source: &Source, number: u64) -> MergeRequest {
        serde_json::from_value(serde_json::json!({
            "source": source, "id": number, "number": number, "title": "fixture",
            "url": format!("https://{}/group/sub/project/-/merge_requests/{number}", source.host),
            "repo": "group/sub/project", "author": "fixture", "is_draft": false,
            "head_ref": "topic", "head_oid": "head", "base_ref": "main",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "labels": [], "reviewers": [], "assignees": [], "comment_count": 0,
            "detailed_merge_status": null, "ci": null, "review": null,
            "unresolved_threads": null
        }))
        .unwrap()
    }

    fn receipt(mrs: Vec<MergeRequest>, coverage: Coverage) -> FetchedList {
        FetchedList {
            viewer: Some("fixture-account".into()),
            total: matches!(coverage, Coverage::Complete).then_some(mrs.len() as u64),
            mrs,
            coverage,
        }
    }

    #[test]
    fn first_receipt_is_silent_but_arrival_after_measured_empty_is_news() {
        let source = source("gitlab.com");
        for list in [CachedList::Authored, CachedList::Reviewing] {
            let mut baseline = Baseline::default();
            let full = receipt(vec![mr(&source, 7)], Coverage::Complete);
            assert!(baseline.observe(&source, list, &full).is_empty());
            assert!(baseline
                .observe(&source, list, &receipt(vec![], Coverage::Complete))
                .is_empty());
            let notices = baseline.observe(&source, list, &full);
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0].identity, full.mrs[0].identity());
            assert_eq!(
                notices[0].kind,
                match list {
                    CachedList::Authored => NoticeKind::AuthoredArrival,
                    CachedList::Reviewing => NoticeKind::ReviewArrival,
                }
            );
        }
    }

    #[test]
    fn changing_account_after_an_empty_list_establishes_a_silent_baseline() {
        let source = source("gitlab.com");
        let mut baseline = Baseline::default();
        baseline.observe(
            &source,
            CachedList::Authored,
            &receipt(vec![], Coverage::Complete),
        );
        let mut next = receipt(vec![mr(&source, 7)], Coverage::Complete);
        next.viewer = Some("different-account".into());
        assert!(baseline
            .observe(&source, CachedList::Authored, &next)
            .is_empty());
    }

    #[test]
    fn partial_or_unknown_baselines_do_not_turn_missing_rows_into_arrivals() {
        let source = source("gitlab.com");
        let full = receipt(vec![mr(&source, 7), mr(&source, 8)], Coverage::Complete);
        for coverage in [Coverage::Partial { total: Some(2) }, Coverage::Unknown] {
            for list in [CachedList::Authored, CachedList::Reviewing] {
                let mut baseline = Baseline::default();
                baseline.observe(&source, list, &full);
                baseline.observe(
                    &source,
                    list,
                    &receipt(vec![mr(&source, 7)], coverage.clone()),
                );
                assert!(baseline.observe(&source, list, &full).is_empty());
                let added = receipt(
                    vec![mr(&source, 7), mr(&source, 8), mr(&source, 9)],
                    Coverage::Complete,
                );
                assert_eq!(baseline.observe(&source, list, &added).len(), 1);
            }
        }
    }

    #[test]
    fn a_partial_receipt_keeps_measured_changes_without_claiming_completeness() {
        let source = source("gitlab.com");
        let mut baseline = Baseline::default();
        baseline.observe(
            &source,
            CachedList::Authored,
            &receipt(vec![], Coverage::Complete),
        );
        let notices = baseline.observe(
            &source,
            CachedList::Authored,
            &receipt(vec![mr(&source, 7)], Coverage::Partial { total: Some(20) }),
        );
        assert_eq!(notices.len(), 1);
        assert!(!baseline.complete);
    }

    #[test]
    fn review_draft_transition_does_not_invent_ci_or_approval_readiness() {
        let source = source("gitlab.com");
        let mut baseline = Baseline::default();
        let mut draft = mr(&source, 7);
        draft.is_draft = true;
        baseline.observe(
            &source,
            CachedList::Reviewing,
            &receipt(vec![draft], Coverage::Complete),
        );
        let notices = baseline.observe(
            &source,
            CachedList::Reviewing,
            &receipt(vec![mr(&source, 7)], Coverage::Complete),
        );
        assert_eq!(notices[0].kind, NoticeKind::ReviewDraftReady);
        assert_eq!(
            notices[0].kind.reason(),
            "is no longer a draft in your GitLab review queue"
        );
    }

    #[test]
    fn unknown_ci_and_pending_merge_status_cannot_establish_breakage_transitions() {
        let source = source("gitlab.com");
        let mut broken = mr(&source, 7);
        broken.ci = Some(CiState::Failure);
        broken.detailed_merge_status = Some("conflict".into());
        for merge in [
            None,
            Some("checking"),
            Some("unchecked"),
            Some("ci_must_pass"),
        ] {
            let mut baseline = Baseline::default();
            let mut before = mr(&source, 7);
            before.detailed_merge_status = merge.map(str::to_owned);
            baseline.observe(
                &source,
                CachedList::Authored,
                &receipt(vec![before], Coverage::Complete),
            );
            assert!(baseline
                .observe(
                    &source,
                    CachedList::Authored,
                    &receipt(vec![broken.clone()], Coverage::Complete)
                )
                .is_empty());
        }
        let mut baseline = Baseline::default();
        let mut before = mr(&source, 7);
        before.detailed_merge_status = Some("mergeable".into());
        baseline.observe(
            &source,
            CachedList::Authored,
            &receipt(vec![before], Coverage::Complete),
        );
        let notices = baseline.observe(
            &source,
            CachedList::Authored,
            &receipt(vec![broken], Coverage::Complete),
        );
        assert_eq!(notices[0].kind, NoticeKind::Conflict);
    }

    #[test]
    fn hosts_and_lists_keep_separate_notification_baselines() {
        let first = source("gitlab.com");
        let other = source("gitlab.example");
        let mut baseline = Baseline::default();
        baseline.observe(
            &first,
            CachedList::Authored,
            &receipt(vec![], Coverage::Complete),
        );
        assert!(baseline
            .observe(
                &other,
                CachedList::Authored,
                &receipt(vec![mr(&other, 7)], Coverage::Complete)
            )
            .is_empty());
        assert!(baseline
            .observe(
                &other,
                CachedList::Reviewing,
                &receipt(vec![mr(&other, 8)], Coverage::Complete)
            )
            .is_empty());
        // A wrong-source row cannot produce an arrival or a complete baseline.
        assert!(baseline
            .observe(
                &other,
                CachedList::Reviewing,
                &receipt(vec![mr(&first, 9)], Coverage::Complete)
            )
            .is_empty());
        assert!(!baseline.complete);
    }

    #[test]
    fn preference_gates_are_independent_and_master_disable_wins() {
        let mut prefs = NotifyPrefs {
            new_pr: false,
            ..NotifyPrefs::default()
        };
        assert!(!NoticeKind::AuthoredArrival.enabled(&prefs));
        assert!(NoticeKind::ReviewArrival.enabled(&prefs));
        prefs.ready_to_review = false;
        assert!(!NoticeKind::ReviewDraftReady.enabled(&prefs));
        assert!(NoticeKind::Conflict.enabled(&prefs));
        prefs.enabled = false;
        assert!(!NoticeKind::Conflict.enabled(&prefs));
        assert!(!NoticeKind::CiFailed.enabled(&prefs));
    }

    #[tokio::test]
    async fn disabling_notifications_during_fetch_suppresses_both_queue_arrivals() {
        let source = source("gitlab.com");
        for list in [CachedList::Authored, CachedList::Reviewing] {
            let saved_prefs = Mutex::new(NotifyPrefs::default());
            let reads = AtomicU64::new(0);
            let mut baseline = Baseline::default();
            baseline.observe(&source, list, &receipt(vec![], Coverage::Complete));
            let (fetched, pending_fetch) = tokio::sync::oneshot::channel();
            let pending =
                notification_batch(async { Some(pending_fetch.await.unwrap()) }, || async {
                    reads.fetch_add(1, Ordering::Relaxed);
                    *saved_prefs.lock().unwrap()
                });
            tokio::pin!(pending);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(5), &mut pending,)
                    .await
                    .is_err()
            );
            assert_eq!(reads.load(Ordering::Relaxed), 0);

            // The user changes settings while the queue is still in flight.
            saved_prefs.lock().unwrap().enabled = false;
            fetched
                .send(receipt(vec![mr(&source, 7)], Coverage::Complete))
                .unwrap();
            let (receipt, prefs) = pending.await.unwrap();
            let notices = baseline.observe(&source, list, &receipt);
            assert_eq!(notices.len(), 1, "this arrival would expose an MR title");
            assert!(notices.iter().all(|notice| !notice.kind.enabled(&prefs)));
            assert_eq!(reads.load(Ordering::Relaxed), 1);
            assert!(
                baseline.observe(&source, list, &receipt).is_empty(),
                "muted receipts still advance history, so enabling alerts cannot replay them"
            );
        }
    }

    #[tokio::test]
    async fn selection_wakes_only_its_own_scheduler_and_invalidates_resumed_history() {
        let control = Control::new(None);
        let other_host = Control::new(None);
        assert!(control.selected().source.is_none());
        let github_wake = Notify::new();
        control.select(Some(source("gitlab.com")));
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            control.wake.notified()
        )
        .await
        .is_ok());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), github_wake.notified())
                .await
                .is_err()
        );
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(5),
            other_host.wake.notified()
        )
        .await
        .is_err());
        let selected = control.selected();
        control.select(selected.source.clone());
        assert!(control.is_current(&selected));
        control.select(None);
        control.select(selected.source.clone());
        assert!(
            !control.is_current(&selected),
            "a resumed source starts a silent baseline"
        );
    }
}

/// Reconcile both memberships after any attempted write, including a lost
/// response. The caller schedules reads without delaying the action receipt.
/// Never retries the write; the original success/error is preserved.
pub async fn after_mutation<T>(
    mutation: impl Future<Output = T>,
    mut schedule: impl FnMut(CachedList),
) -> T {
    let result = mutation.await;
    queues::invalidate();
    schedule(CachedList::Authored);
    schedule(CachedList::Reviewing);
    result
}

#[cfg(test)]
mod mutation_tests {
    use super::*;
    #[tokio::test]
    async fn action_receipt_waits_for_write_but_schedules_both_lists_even_on_error() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (send, recv) = tokio::sync::oneshot::channel::<Result<(), &str>>();
        let recorded = calls.clone();
        let task = tokio::spawn(async move {
            after_mutation(async { recv.await.unwrap() }, move |list| {
                recorded.lock().unwrap().push(list)
            })
            .await
        });
        tokio::task::yield_now().await;
        assert!(calls.lock().unwrap().is_empty());
        send.send(Err("write outcome unknown")).unwrap();
        assert_eq!(task.await.unwrap(), Err("write outcome unknown"));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![CachedList::Authored, CachedList::Reviewing]
        );
    }
}
