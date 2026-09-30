//! Each source/list owns its attempts, last successful receipt and error.
//! GitLab fetching is not enabled here. Slice 4 plugs its bounded page loader
//! into this channel; slice 6 consumes it instead of the legacy GitHub events.
use crate::{
    github::client::FetchedList,
    identity::Source,
    store::{source_cache::Coverage, CachedList},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    NotRequested,
    Fetching,
    Ready,
    Partial,
    Unknown,
    Retrying,
    Failed,
    NotAsked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub source: Source,
    pub list: CachedList,
    pub phase: Phase,
    pub revision: u64,
    pub receipt_revision: Option<u64>,
    pub request_id: Option<String>,
    pub consecutive_failures: u32,
    /// Network receipt time, never the time somebody read SQLite.
    pub last_received_at: Option<String>,
    pub coverage: Option<Coverage>,
    pub error: Option<String>,
}

impl Status {
    fn new(source: Source, list: CachedList) -> Self {
        Self {
            source,
            list,
            phase: Phase::NotRequested,
            revision: 0,
            receipt_revision: None,
            request_id: None,
            consecutive_failures: 0,
            last_received_at: None,
            coverage: None,
            error: None,
        }
    }
}
type PollKey = (Source, CachedList);
type StatusEntries = HashMap<PollKey, (u64, Status)>;
type PublicationGates = HashMap<PollKey, Arc<tokio::sync::Mutex<()>>>;

type Receipts = HashMap<PollKey, (u64, FetchedList)>;
type GitLabReceipts = HashMap<PollKey, (u64, crate::gitlab::queues::FetchedList)>;

/// One atomic event/reply. Rows and attempt status have independent ordering:
/// an older successful attempt can publish rows beside a newer failure.
#[derive(Clone, Serialize)]
pub struct Update {
    #[serde(flatten)]
    pub status: Status,
    pub session: String,
    pub completed_request: Option<String>,
    pub prs: Option<Vec<crate::github::model::PullRequest>>,
    pub mrs: Option<Vec<crate::gitlab::queues::MergeRequest>>,
}

pub struct SourcePolls(
    Mutex<StatusEntries>,
    Mutex<PublicationGates>,
    Mutex<Receipts>,
    String,
    Mutex<GitLabReceipts>,
    Mutex<HashMap<PollKey, u64>>,
);
impl Default for SourcePolls {
    fn default() -> Self {
        Self(
            Mutex::default(),
            Mutex::default(),
            Mutex::default(),
            format!("{}:{}", std::process::id(), chrono::Utc::now().to_rfc3339()),
            Mutex::default(),
            Mutex::default(),
        )
    }
}

pub struct Publication {
    attempt: Attempt,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl SourcePolls {
    fn before_mutation(&self, attempt: &Attempt) -> bool {
        self.5
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(attempt.source.clone(), attempt.list))
            .is_some_and(|minimum| attempt.generation < *minimum)
    }
    async fn invalidate_gitlab(&self, source: &Source) {
        for list in [CachedList::Authored, CachedList::Reviewing] {
            let _guard = self.gate(source, list).lock_owned().await;
            let minimum = self
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(source.clone(), list))
                .map(|(generation, _)| generation + 1)
                .unwrap_or(1);
            self.5
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((source.clone(), list), minimum);
        }
    }
    fn gate(&self, source: &Source, list: CachedList) -> Arc<tokio::sync::Mutex<()>> {
        self.1
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry((source.clone(), list))
            .or_default()
            .clone()
    }
    async fn begin_and_emit(
        &self,
        source: Source,
        list: CachedList,
        request_id: Option<String>,
        emit: impl FnOnce(Status),
    ) -> Attempt {
        // Starting a newer attempt cannot race a committed snapshot's write
        // and terminal event. Network fetches never hold this per-key gate.
        let _publication = self.gate(&source, list).lock_owned().await;
        let (generation, status) = self.begin_request(&source, list, request_id.clone());
        emit(status);
        Attempt {
            source,
            list,
            generation,
            request_id,
        }
    }
    #[cfg(test)]
    async fn begin_attempt(&self, source: Source, list: CachedList) -> (Attempt, Status) {
        let attempt = self
            .begin_and_emit(source.clone(), list, None, |_| {})
            .await;
        (attempt, self.get(&source, list))
    }
    async fn publication(&self, attempt: &Attempt) -> Option<Publication> {
        let guard = self.gate(&attempt.source, attempt.list).lock_owned().await;
        let current = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(attempt.source.clone(), attempt.list))
            .is_some_and(|(generation, _)| *generation == attempt.generation);
        current.then_some(Publication {
            attempt: attempt.clone(),
            _guard: guard,
        })
    }
    async fn success_publication(&self, attempt: &Attempt) -> Option<Publication> {
        let guard = self.gate(&attempt.source, attempt.list).lock_owned().await;
        if self.before_mutation(attempt) {
            return None;
        }
        let newer_success = if attempt.source.provider == crate::identity::Provider::Gitlab {
            self.4
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(attempt.source.clone(), attempt.list))
                .is_some_and(|(generation, _)| *generation > attempt.generation)
        } else {
            self.2
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(attempt.source.clone(), attempt.list))
                .is_some_and(|(generation, _)| *generation > attempt.generation)
        };
        (!newer_success).then_some(Publication {
            attempt: attempt.clone(),
            _guard: guard,
        })
    }
    fn complete(
        &self,
        publication: Publication,
        result: Result<FetchedList, Failure>,
        emit: impl FnOnce(Status),
    ) {
        let attempt = &publication.attempt;
        let status_result = match result {
            Ok(result) => {
                let coverage = result.coverage.clone();
                self.2.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    (attempt.source.clone(), attempt.list),
                    (attempt.generation, result),
                );
                Ok(coverage)
            }
            Err(error) => Err(error),
        };
        if let Some(status) = self.finish(attempt, status_result) {
            emit(status);
        }
        // The publication permit remains held through both the status mutation
        // and callback. A newer attempt cannot interleave a fetching event.
    }
    fn complete_gitlab(
        &self,
        publication: Publication,
        result: Result<crate::gitlab::queues::FetchedList, Failure>,
        emit: impl FnOnce(Status),
    ) {
        let attempt = &publication.attempt;
        let status_result = match result {
            Ok(result) => {
                let coverage = result.coverage.clone();
                self.4.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    (attempt.source.clone(), attempt.list),
                    (attempt.generation, result),
                );
                Ok(coverage)
            }
            Err(error) => Err(error),
        };
        if let Some(status) = self.finish(attempt, status_result) {
            emit(status);
        }
    }
    fn winner(
        &self,
        attempt: &Attempt,
        fallback: Result<FetchedList, String>,
    ) -> Result<FetchedList, String> {
        self.2
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(attempt.source.clone(), attempt.list))
            .filter(|(generation, _)| *generation > attempt.generation)
            .map(|(_, result)| Ok(result.clone()))
            .unwrap_or(fallback)
    }
    fn winner_gitlab(
        &self,
        attempt: &Attempt,
        fallback: Result<crate::gitlab::queues::FetchedList, String>,
    ) -> Result<crate::gitlab::queues::FetchedList, String> {
        if self.before_mutation(attempt) {
            return Err(
                "GitLab changed while this list was loading. A new refresh is required.".into(),
            );
        }
        self.4
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(attempt.source.clone(), attempt.list))
            .filter(|(generation, _)| *generation > attempt.generation)
            .map(|(_, result)| Ok(result.clone()))
            .unwrap_or(fallback)
    }

    fn update(&self, status: Status, completed_request: Option<String>) -> Update {
        let prs = self
            .2
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(status.source.clone(), status.list))
            .map(|(_, receipt)| receipt.prs.clone());
        let mrs = self
            .4
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(status.source.clone(), status.list))
            .map(|(_, receipt)| receipt.mrs.clone());
        Update {
            status,
            session: self.3.clone(),
            completed_request,
            prs,
            mrs,
        }
    }
    pub async fn snapshot(&self, source: &Source, list: CachedList) -> Update {
        let _guard = self.gate(source, list).lock_owned().await;
        self.update(self.get(source, list), None)
    }

    pub fn get(&self, source: &Source, list: CachedList) -> Status {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(source.clone(), list))
            .map(|(_, status)| status.clone())
            .unwrap_or_else(|| Status::new(source.clone(), list))
    }
    #[cfg(test)]
    fn begin(&self, source: &Source, list: CachedList) -> (u64, Status) {
        self.begin_request(source, list, None)
    }
    fn begin_request(
        &self,
        source: &Source,
        list: CachedList,
        request_id: Option<String>,
    ) -> (u64, Status) {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (generation, status) = entries
            .entry((source.clone(), list))
            .or_insert_with(|| (0, Status::new(source.clone(), list)));
        *generation += 1;
        status.phase = Phase::Fetching;
        status.revision += 1;
        status.request_id = request_id;
        (*generation, status.clone())
    }
    fn finish(&self, attempt: &Attempt, result: Result<Coverage, Failure>) -> Option<Status> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (generation, status) = entries.get_mut(&(attempt.source.clone(), attempt.list))?;
        if *generation != attempt.generation {
            // An older successful request still advances usable data when no
            // newer success exists. Preserve the newer request's pending/error
            // phase: success data and last-attempt outcome are separate facts.
            if let Ok(coverage) = result {
                status.revision += 1;
                status.receipt_revision = Some(status.revision);
                status.last_received_at = Some(chrono::Utc::now().to_rfc3339());
                status.coverage = Some(coverage);
                return Some(status.clone());
            }
            return None;
        }
        status.revision += 1;
        match result {
            Ok(coverage) => {
                status.receipt_revision = Some(status.revision);
                status.phase = match coverage {
                    Coverage::Complete => Phase::Ready,
                    Coverage::Partial { .. } => Phase::Partial,
                    Coverage::Unknown => Phase::Unknown,
                };
                status.last_received_at = Some(chrono::Utc::now().to_rfc3339());
                status.coverage = Some(coverage);
                status.consecutive_failures = 0;
                status.error = None;
            }
            Err(failure) => {
                status.phase = if failure.not_asked {
                    Phase::NotAsked
                } else {
                    status.consecutive_failures = status.consecutive_failures.saturating_add(1);
                    if failure.transient && status.consecutive_failures < 2 {
                        Phase::Retrying
                    } else {
                        Phase::Failed
                    }
                };
                status.error = Some(failure.message);
                // Keep the last receipt and its coverage on every failure.
            }
        }
        Some(status.clone())
    }
}

pub struct Failure {
    pub message: String,
    pub transient: bool,
    pub not_asked: bool,
}
impl From<&crate::github::client::ClientError> for Failure {
    fn from(error: &crate::github::client::ClientError) -> Self {
        Self {
            message: error.to_string(),
            transient: error.is_transient(),
            not_asked: false,
        }
    }
}
#[derive(Clone)]
pub struct Attempt {
    source: Source,
    list: CachedList,
    generation: u64,
    request_id: Option<String>,
}

fn emit_status(app: &AppHandle, status: Status, completed_request: Option<String>) {
    // The legacy authored bar must also preserve a newer pending/failure state
    // when an older successful response supplies usable rows.
    if status.source == Source::default() && status.list == CachedList::Authored {
        let phase = match status.phase {
            Phase::Fetching => "fetching",
            Phase::Retrying => "retrying",
            _ => "idle",
        };
        if status.phase == Phase::Failed {
            if let Some(error) = &status.error {
                let _ = app.emit("poll-error", error);
            }
        }
        let _ = app.emit("poll-state", phase);
    }
    let update = app.state::<SourcePolls>().update(status, completed_request);
    let _ = app.emit("source-poll-status", update);
}

pub async fn begin(app: &AppHandle, source: Source, list: CachedList) -> Attempt {
    begin_request(app, source, list, None).await
}

pub async fn begin_request(
    app: &AppHandle,
    source: Source,
    list: CachedList,
    request_id: Option<String>,
) -> Attempt {
    app.state::<SourcePolls>()
        .begin_and_emit(source, list, request_id, |status| {
            emit_status(app, status, None);
        })
        .await
}

/// Hold through the database write, data events and terminal status.
pub async fn publication(app: &AppHandle, attempt: &Attempt) -> Option<Publication> {
    app.state::<SourcePolls>().publication(attempt).await
}

/// Completing consumes the permit, keeping status mutation and emission ordered
/// with every other attempt, including failures and declined requests.
pub fn complete(app: &AppHandle, publication: Publication, result: Result<FetchedList, Failure>) {
    let completed_request = publication.attempt.request_id.clone();
    app.state::<SourcePolls>()
        .complete(publication, result, |status| {
            emit_status(app, status, completed_request);
        });
}

/// An accepted or uncertain write retires all reads that began before it.
/// Ordinary competing refreshes still retain their older usable receipts.
pub async fn invalidate_gitlab(app: &AppHandle, source: &Source) {
    app.state::<SourcePolls>().invalidate_gitlab(source).await;
}

pub fn complete_gitlab(
    app: &AppHandle,
    publication: Publication,
    result: Result<crate::gitlab::queues::FetchedList, Failure>,
) {
    let completed_request = publication.attempt.request_id.clone();
    app.state::<SourcePolls>()
        .complete_gitlab(publication, result, |status| {
            emit_status(app, status, completed_request);
        });
}

pub async fn fail(app: &AppHandle, attempt: Attempt, failure: Failure) {
    if let Some(publication) = publication(app, &attempt).await {
        complete(app, publication, Err(failure));
    }
}

pub async fn success_publication(app: &AppHandle, attempt: &Attempt) -> Option<Publication> {
    app.state::<SourcePolls>()
        .success_publication(attempt)
        .await
}

/// Read the newest already-published success; never wait for a network request.
pub fn winner(
    app: &AppHandle,
    attempt: &Attempt,
    fallback: Result<FetchedList, String>,
) -> Result<FetchedList, String> {
    app.state::<SourcePolls>().winner(attempt, fallback)
}

pub fn winner_gitlab(
    app: &AppHandle,
    attempt: &Attempt,
    fallback: Result<crate::gitlab::queues::FetchedList, String>,
) -> Result<crate::gitlab::queues::FetchedList, String> {
    app.state::<SourcePolls>().winner_gitlab(attempt, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Provider;
    fn attempt(polls: &SourcePolls, source: &Source, list: CachedList) -> Attempt {
        Attempt {
            source: source.clone(),
            list,
            generation: polls.begin(source, list).0,
            request_id: None,
        }
    }
    fn failure() -> Failure {
        Failure {
            message: "request failed".into(),
            transient: true,
            not_asked: false,
        }
    }
    fn receipt(number: u64) -> FetchedList {
        let pr = serde_json::from_value(serde_json::json!({
            "number": number, "title": "fixture", "url": "https://github.com/o/r/pull/1",
            "repo": "o/r", "author": "fixture", "is_draft": true,
            "head_ref": "topic", "base_ref": "main", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "ci": "pending", "merge": "checking",
            "review": "review_required", "in_merge_queue": false, "labels": [], "comment_count": 0
        }))
        .unwrap();
        FetchedList {
            prs: vec![pr],
            total: Some(1),
            coverage: Coverage::Complete,
        }
    }

    fn gitlab_mr(source: &Source) -> crate::gitlab::queues::MergeRequest {
        serde_json::from_value(serde_json::json!({
            "source": source, "id": 17, "number": 7, "title": "fixture",
            "url": format!("https://{}/group/subgroup/project/-/merge_requests/7", source.host),
            "repo": "group/subgroup/project", "author": "fixture", "is_draft": true,
            "head_ref": "topic", "base_ref": "main",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "labels": [], "reviewers": [], "assignees": [], "comment_count": 0,
            "detailed_merge_status": "draft_status", "ci": null,
            "review": null, "unresolved_threads": null
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn background_gitlab_pages_survive_a_newer_pending_then_failed_foreground_request() {
        let polls = SourcePolls::default();
        let source = Source {
            provider: Provider::Gitlab,
            host: "gitlab.com".into(),
        };
        let (background, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let foreground = polls
            .begin_and_emit(
                source.clone(),
                CachedList::Authored,
                Some("foreground".into()),
                |_| {},
            )
            .await;
        let published = polls.success_publication(&background).await.unwrap();
        polls.complete_gitlab(
            published,
            Ok(crate::gitlab::queues::FetchedList {
                viewer: None,
                mrs: vec![gitlab_mr(&source)],
                total: None,
                coverage: Coverage::Partial { total: None },
            }),
            |status| {
                assert_eq!(status.phase, Phase::Fetching);
                assert_eq!(status.request_id.as_deref(), Some("foreground"));
                assert_eq!(status.coverage, Some(Coverage::Partial { total: None }));
                assert!(status.receipt_revision.is_some());
            },
        );
        let receipt_at = polls.get(&source, CachedList::Authored).last_received_at;
        let published = polls.publication(&foreground).await.unwrap();
        polls.complete_gitlab(published, Err(failure()), |_| {});
        let snapshot = polls.snapshot(&source, CachedList::Authored).await;
        assert_eq!(snapshot.status.phase, Phase::Retrying);
        assert_eq!(snapshot.status.last_received_at, receipt_at);
        assert_eq!(
            snapshot.status.coverage,
            Some(Coverage::Partial { total: None })
        );
        assert_eq!(snapshot.mrs, Some(vec![gitlab_mr(&source)]));
        assert!(snapshot.prs.is_none());
    }

    #[tokio::test]
    async fn mutation_retires_both_old_lists_even_if_the_reconciliation_fails() {
        let polls = SourcePolls::default();
        let source = Source {
            provider: Provider::Gitlab,
            host: "gitlab.com".into(),
        };
        let (authored, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let (reviewing, _) = polls
            .begin_attempt(source.clone(), CachedList::Reviewing)
            .await;
        polls.invalidate_gitlab(&source).await;
        let (fresh, _) = polls.begin_attempt(source, CachedList::Authored).await;
        let permit = polls.publication(&fresh).await.unwrap();
        polls.complete_gitlab(permit, Err(failure()), |_| {});
        assert!(polls.success_publication(&authored).await.is_none());
        assert!(polls.success_publication(&reviewing).await.is_none());
        assert!(polls
            .winner_gitlab(
                &authored,
                Ok(crate::gitlab::queues::FetchedList {
                    viewer: None,
                    mrs: vec![],
                    total: Some(0),
                    coverage: Coverage::Complete
                })
            )
            .is_err());
        assert!(polls.success_publication(&fresh).await.is_some());
    }

    #[tokio::test]
    async fn newer_gitlab_foreground_success_rejects_old_background_publication_only_for_its_key() {
        let polls = SourcePolls::default();
        let source = Source {
            provider: Provider::Gitlab,
            host: "gitlab.com".into(),
        };
        let (background, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let (reviewing, _) = polls
            .begin_attempt(source.clone(), CachedList::Reviewing)
            .await;
        let (other_host, _) = polls
            .begin_attempt(
                Source {
                    host: "gitlab.example".into(),
                    ..source.clone()
                },
                CachedList::Authored,
            )
            .await;
        let (foreground, _) = polls.begin_attempt(source, CachedList::Authored).await;
        let receipt = crate::gitlab::queues::FetchedList {
            viewer: None,
            mrs: vec![],
            total: Some(0),
            coverage: Coverage::Complete,
        };
        let publication = polls.success_publication(&foreground).await.unwrap();
        polls.complete_gitlab(publication, Ok(receipt.clone()), |_| {});
        assert!(polls.success_publication(&background).await.is_none());
        assert_eq!(
            polls
                .winner_gitlab(&background, Err("older failed".into()))
                .unwrap(),
            receipt
        );
        assert!(polls.success_publication(&reviewing).await.is_some());
        assert!(polls.success_publication(&other_host).await.is_some());
    }

    #[tokio::test]
    async fn gitlab_receipt_has_its_own_rows_and_partial_coverage() {
        let polls = SourcePolls::default();
        let source = Source {
            provider: Provider::Gitlab,
            host: "gitlab.com".into(),
        };
        let (attempt, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let receipt = crate::gitlab::queues::FetchedList {
            viewer: None,
            mrs: vec![gitlab_mr(&source)],
            total: Some(2),
            coverage: Coverage::Partial { total: Some(2) },
        };
        let permit = polls.success_publication(&attempt).await.unwrap();
        polls.complete_gitlab(permit, Ok(receipt), |_| {});
        let update = polls.snapshot(&attempt.source, CachedList::Authored).await;
        assert_eq!(update.status.phase, Phase::Partial);
        assert!(update.prs.is_none());
        assert_eq!(update.mrs.as_ref().unwrap().len(), 1);
        assert_eq!(update.mrs.as_ref().unwrap()[0].unresolved_threads, None);
        assert_eq!(
            serde_json::to_value(&update).unwrap()["mrs"][0]["ci"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn correlated_snapshot_orders_rows_and_status_without_changing_legacy_replies() {
        let polls = SourcePolls::default();
        let source = Source::default();
        let a = polls
            .begin_and_emit(
                source.clone(),
                CachedList::Reviewing,
                Some("phone-a".into()),
                |status| {
                    assert_eq!(status.request_id.as_deref(), Some("phone-a"));
                    assert_eq!(status.revision, 1);
                },
            )
            .await;
        let (b, _) = polls
            .begin_attempt(source.clone(), CachedList::Reviewing)
            .await;
        let permit = polls.publication(&b).await.unwrap();
        polls.complete(permit, Err(failure()), |_| {});
        let failed = polls.snapshot(&source, CachedList::Reviewing).await;
        assert_eq!(failed.status.revision, 3);
        assert!(failed.prs.is_none());
        let permit = polls.success_publication(&a).await.unwrap();
        polls.complete(permit, Ok(receipt(1)), |_| {});
        let updated = polls.snapshot(&source, CachedList::Reviewing).await;
        assert_eq!(updated.status.revision, 4);
        assert_eq!(updated.status.receipt_revision, Some(4));
        assert_eq!(updated.status.error, failed.status.error);
        assert_eq!(updated.prs.as_ref().unwrap()[0].number, 1);
        // A same-attempt recheck still has a distinct receipt revision.
        let permit = polls.success_publication(&a).await.unwrap();
        polls.complete(permit, Ok(receipt(2)), |_| {});
        let rechecked = polls.snapshot(&source, CachedList::Reviewing).await;
        assert_eq!(rechecked.status.receipt_revision, Some(5));
        assert_eq!(rechecked.session, updated.session);
        let legacy = crate::commands::RefreshReply::Legacy(Vec::new());
        assert_eq!(serde_json::to_value(legacy).unwrap(), serde_json::json!([]));
        let reply = crate::commands::RefreshReply::Correlated {
            request_id: "phone-a".into(),
            update: Box::new(rechecked),
        };
        let json = serde_json::to_value(reply).unwrap();
        assert_eq!(json["request_id"], "phone-a");
        assert_eq!(json["update"]["revision"], 5);
        assert_eq!(json["update"]["prs"][0]["number"], 2);
    }

    #[tokio::test]
    async fn successful_data_does_not_wait_for_a_later_pending_or_aborted_attempt() {
        let polls = SourcePolls::default();
        let source = Source::default();
        let (foreground, _) = polls
            .begin_attempt(source.clone(), CachedList::Reviewing)
            .await;
        let (background, _) = polls
            .begin_attempt(source.clone(), CachedList::Reviewing)
            .await;
        let publication = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            polls.success_publication(&foreground),
        )
        .await
        .unwrap()
        .unwrap();
        polls.complete(publication, Ok(receipt(1)), |_| {});
        assert_eq!(
            polls.get(&source, CachedList::Reviewing).phase,
            Phase::Fetching
        );
        // A later request may never complete. No join waits on it.
        assert_eq!(
            polls.winner(&foreground, Ok(receipt(1))).unwrap().prs[0].number,
            1
        );
        let publication = polls.success_publication(&background).await.unwrap();
        polls.complete(publication, Ok(receipt(2)), |_| {});
        assert!(polls.success_publication(&foreground).await.is_none());
        assert_eq!(
            polls.winner(&foreground, Ok(receipt(1))).unwrap().prs[0].number,
            2
        );
        assert_eq!(
            polls
                .winner(&foreground, Err("older failure".into()))
                .unwrap()
                .prs[0]
                .number,
            2
        );
    }

    #[tokio::test]
    async fn newest_success_survives_a_later_failure_and_an_older_completion() {
        let polls = SourcePolls::default();
        let source = Source::default();
        let (a, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let (b, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let (c, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let publication = polls.publication(&c).await.unwrap();
        polls.complete(publication, Err(failure()), |_| {});
        let publication = polls.success_publication(&b).await.unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let data = receipt(2);
        crate::store::source_cache::save_source_snapshot(
            &conn,
            &source,
            CachedList::Authored,
            &data.prs,
            &data.coverage,
        )
        .unwrap();
        polls.complete(publication, Ok(data), |status| {
            assert_eq!(status.phase, Phase::Retrying, "C's failure stays visible");
            assert_eq!(status.coverage, Some(Coverage::Complete));
            assert!(status.last_received_at.is_some());
            assert_eq!(
                crate::store::load_snapshot(&conn, CachedList::Authored).unwrap()[0].number,
                2
            );
        });
        assert!(polls.success_publication(&a).await.is_none());
        assert_eq!(polls.winner(&a, Ok(receipt(1))).unwrap().prs[0].number, 2);
        assert_eq!(
            crate::store::load_snapshot(&conn, CachedList::Authored).unwrap()[0].number,
            2
        );
        assert_eq!(
            polls.get(&source, CachedList::Authored).phase,
            Phase::Retrying
        );
    }

    #[tokio::test]
    async fn fetching_and_failure_events_keep_the_publication_gate_until_emitted() {
        let polls = SourcePolls::default();
        let source = Source::default();
        let events = Mutex::new(Vec::new());
        for not_asked in [false, true] {
            let attempt = polls
                .begin_and_emit(source.clone(), CachedList::Authored, None, |status| {
                    assert!(polls
                        .gate(&source, CachedList::Authored)
                        .try_lock()
                        .is_err());
                    events.lock().unwrap().push(status.phase);
                })
                .await;
            let publication = polls.publication(&attempt).await.unwrap();
            polls.complete(
                publication,
                Err(Failure {
                    message: "fixture".into(),
                    transient: false,
                    not_asked,
                }),
                |status| {
                    assert!(polls
                        .gate(&source, CachedList::Authored)
                        .try_lock()
                        .is_err());
                    assert_eq!(polls.get(&source, CachedList::Authored), status);
                    events.lock().unwrap().push(status.phase);
                },
            );
        }
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                Phase::Fetching,
                Phase::Failed,
                Phase::Fetching,
                Phase::NotAsked
            ]
        );
    }

    #[tokio::test]
    async fn publication_rejects_old_results_and_serializes_only_its_own_queue() {
        let polls = SourcePolls::default();
        let source = Source::default();
        let (old, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        let (manual, _) = polls
            .begin_attempt(source.clone(), CachedList::Authored)
            .await;
        assert!(
            polls.publication(&old).await.is_none(),
            "stale data must never reach the writer"
        );
        let writing = polls.publication(&manual).await.unwrap();
        let waiting = polls.begin_attempt(source.clone(), CachedList::Authored);
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), &mut waiting)
                .await
                .is_err(),
            "new generation must wait until write and terminal event have finished"
        );
        let (reviewing, _) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            polls.begin_attempt(source, CachedList::Reviewing),
        )
        .await
        .unwrap();
        assert!(
            polls.publication(&reviewing).await.is_some(),
            "another queue is independent"
        );
        polls.finish(&manual, Ok(Coverage::Complete));
        drop(writing);
        let (next, _) = waiting.await;
        assert!(polls.publication(&manual).await.is_none());
        assert!(polls.publication(&next).await.is_some());
    }

    #[test]
    fn failure_retry_and_recovery_belong_to_one_source_and_list() {
        let polls = SourcePolls::default();
        let gh = Source::default();
        let gl = Source {
            provider: Provider::Gitlab,
            host: "gitlab.com".into(),
        };
        assert_eq!(
            polls.get(&gl, CachedList::Authored).phase,
            Phase::NotRequested
        );
        let a = attempt(&polls, &gl, CachedList::Authored);
        assert_eq!(polls.get(&gl, CachedList::Authored).phase, Phase::Fetching);
        polls.finish(&a, Ok(Coverage::Partial { total: None }));
        let receipt = polls.get(&gl, CachedList::Authored).last_received_at;
        let a = attempt(&polls, &gl, CachedList::Authored);
        assert_eq!(
            polls.finish(&a, Err(failure())).unwrap().phase,
            Phase::Retrying
        );
        let a = attempt(&polls, &gh, CachedList::Authored);
        polls.finish(&a, Ok(Coverage::Complete));
        assert_eq!(polls.get(&gl, CachedList::Authored).consecutive_failures, 1);
        assert_eq!(
            polls.get(&gl, CachedList::Reviewing).phase,
            Phase::NotRequested
        );
        let a = attempt(&polls, &gl, CachedList::Authored);
        let failed = polls.finish(&a, Err(failure())).unwrap();
        assert_eq!(failed.phase, Phase::Failed);
        assert_eq!(failed.last_received_at, receipt);
        assert_eq!(failed.coverage, Some(Coverage::Partial { total: None }));
        let a = attempt(&polls, &gl, CachedList::Authored);
        let recovered = polls.finish(&a, Ok(Coverage::Complete)).unwrap();
        assert_eq!(recovered.consecutive_failures, 0);
        assert_eq!(recovered.error, None);
    }
    #[test]
    fn old_inflight_failure_cannot_overwrite_a_newer_manual_refresh() {
        let polls = SourcePolls::default();
        let source = Source::default();
        let old = attempt(&polls, &source, CachedList::Authored);
        let manual = attempt(&polls, &source, CachedList::Authored);
        polls.finish(&manual, Ok(Coverage::Complete));
        assert!(polls.finish(&old, Err(failure())).is_none());
        assert_eq!(polls.get(&source, CachedList::Authored).phase, Phase::Ready);
    }
    #[test]
    fn declining_to_request_is_not_a_provider_failure() {
        let polls = SourcePolls::default();
        let source = Source::default();
        let a = attempt(&polls, &source, CachedList::Authored);
        let result = polls
            .finish(
                &a,
                Err(Failure {
                    message: "not enabled".into(),
                    transient: false,
                    not_asked: true,
                }),
            )
            .unwrap();
        assert_eq!(result.phase, Phase::NotAsked);
        assert_eq!(result.consecutive_failures, 0);
        assert_eq!(result.last_received_at, None);
    }
}
