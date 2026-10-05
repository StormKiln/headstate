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
    #[serde(skip)]
    settled_phase: Option<Phase>,
    #[serde(skip)]
    published_scan_receipt: Option<String>,
    #[serde(skip)]
    published_scan_generation: Option<u64>,
    #[serde(skip)]
    settled_generation: u64,
    #[serde(skip)]
    settled_scan_receipt: Option<String>,
    #[serde(skip)]
    github_receipt_epoch: u64,
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
            settled_phase: None,
            published_scan_receipt: None,
            published_scan_generation: None,
            settled_generation: 0,
            settled_scan_receipt: None,
            github_receipt_epoch: 0,
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
    pub owner: Option<String>,
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
    std::sync::atomic::AtomicU64,
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
            std::sync::atomic::AtomicU64::new(0),
        )
    }
}

pub struct Publication {
    attempt: Attempt,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Authority captured while the original successful publication gate is held.
#[derive(Clone)]
pub(crate) struct CheckingCapture {
    pub targets: Vec<crate::github::client::CheckingTarget>,
    pub owner: String,
    attempt: Attempt,
    receipt_revision: u64,
    receipt_epoch: u64,
    scan_revision: Option<i64>,
}

impl SourcePolls {
    fn capture_checking(&self, attempt: &Attempt) -> Option<CheckingCapture> {
        if attempt.source != Source::default() || attempt.list != CachedList::Authored {
            return None;
        }
        let status = self.get(&attempt.source, attempt.list);
        let receipts = self.2.lock().unwrap_or_else(|e| e.into_inner());
        let (generation, receipt) = receipts.get(&(attempt.source.clone(), attempt.list))?;
        if *generation != attempt.generation {
            return None;
        }
        let owner = receipt.viewer.clone().filter(|v| !v.is_empty())?;
        let targets = crate::github::client::checking_targets(&receipt.prs);
        if targets.is_empty() {
            return None;
        }
        Some(CheckingCapture {
            targets,
            owner,
            attempt: attempt.clone(),
            receipt_revision: status.receipt_revision?,
            receipt_epoch: status.github_receipt_epoch,
            scan_revision: receipt
                .scan
                .as_ref()
                .map(|s| s.expected_revision + i64::from(!s.state.no_work)),
        })
    }

    /// Caller holds this source/list's publication gate.
    fn checking_receipt(&self, capture: &CheckingCapture) -> Option<FetchedList> {
        let key = (capture.attempt.source.clone(), capture.attempt.list);
        let entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (generation, status) = entries.get(&key)?;
        if *generation != capture.attempt.generation
            || status.receipt_revision != Some(capture.receipt_revision)
            || status.github_receipt_epoch != capture.receipt_epoch
        {
            return None;
        }
        let receipts = self.2.lock().unwrap_or_else(|e| e.into_inner());
        let (generation, receipt) = receipts.get(&key)?;
        (*generation == capture.attempt.generation
            && receipt.viewer.as_deref() == Some(&capture.owner))
        .then(|| receipt.clone())
    }

    /// Unlike `complete`, this does not settle traversal or clear its errors.
    /// The dedicated row transaction must have succeeded before this is called.
    fn complete_checking(
        &self,
        publication: Publication,
        receipt: FetchedList,
        emit: impl FnOnce(Status),
    ) {
        let attempt = &publication.attempt;
        let key = (attempt.source.clone(), attempt.list);
        self.2
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone(), (attempt.generation, receipt));
        let status = {
            let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let (_, status) = entries.get_mut(&key).expect("published source status");
            status.github_receipt_epoch += 1;
            status.revision += 1;
            status.receipt_revision = Some(status.revision);
            status.last_received_at = Some(chrono::Utc::now().to_rfc3339());
            status.clone()
        };
        emit(status);
    }

    fn readback_is_current(&self, source: &Source, list: CachedList, generation: u64) -> bool {
        *self
            .5
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(source.clone(), list))
            .unwrap_or(&0)
            == generation
    }
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
    /// Caller holds the publication gate; the effect retires pre-write work.
    fn effect_generation(&self, source: &Source, list: CachedList) -> u64 {
        let (generation, _) = self.begin_request(source, list, None);
        self.5
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((source.clone(), list), generation);
        generation
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
        let scan_state = result
            .as_ref()
            .ok()
            .and_then(|r| r.scan.as_ref())
            .map(|s| s.state.clone());
        let status_result = match result {
            Ok(result) => {
                let coverage = result.coverage.clone();
                // Scan adoption can replace a receipt without advancing its
                // public receipt revision. Retire captured observation work
                // on every accepted replacement, including that case.
                if let Some((_, status)) = self
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(&(attempt.source.clone(), attempt.list))
                {
                    status.github_receipt_epoch += 1;
                }
                self.2.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    (attempt.source.clone(), attempt.list),
                    (attempt.generation, result),
                );
                Ok(coverage)
            }
            Err(error) => Err(error),
        };
        let status = match (scan_state, status_result) {
            (Some(state), Ok(coverage)) => self.finish_scan(attempt, &state, coverage),
            (_, result) => self.finish(attempt, result),
        };
        if let Some(status) = status {
            emit(status);
        }
        // The publication permit remains held through both status mutation and emission.
    }

    fn finish_scan(
        &self,
        attempt: &Attempt,
        state: &crate::queue_scan::State,
        coverage: Coverage,
    ) -> Option<Status> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (generation, status) = entries.get_mut(&(attempt.source.clone(), attempt.list))?;
        let current = *generation == attempt.generation;
        let published =
            state.receipt_id.is_some() && status.published_scan_receipt == state.receipt_id;
        if state.no_work {
            if !current {
                return None;
            }
            status.revision += 1;
            status.phase = status.settled_phase.clone().unwrap_or(Phase::NotRequested);
            if status.coverage.is_none() {
                status.coverage = Some(coverage.clone());
            }
            // A later checkpoint read can discover an operation published by an
            // older caller while this request was pending. Adopt that validated
            // outcome once, but never over an independently newer settlement.
            let unsettled_outcome = published
                && status.settled_scan_receipt != state.receipt_id
                && status
                    .published_scan_generation
                    .is_some_and(|published| published > status.settled_generation);
            if !unsettled_outcome {
                // Disk readback on restart carries no invented provider receipt time.
                if status.phase == Phase::NotRequested {
                    status.phase = if state.step_failure.is_some() {
                        Phase::Failed
                    } else if matches!(status.coverage, Some(Coverage::Complete)) {
                        Phase::Ready
                    } else {
                        Phase::Partial
                    };
                    status.error = state
                        .step_failure
                        .as_ref()
                        .map(|f| f.message.clone())
                        .or_else(|| state.partition_reason());
                }
                return Some(status.clone());
            }
        } else {
            // Coalesced callers share the operation's data receipt, but only the
            // current request may settle its outcome. A stale caller may publish
            // useful rows (including changed qualification) beside newer work.
            if !current && published {
                return None;
            }
            status.revision += 1;
            if !published {
                status.receipt_revision = Some(status.revision);
                status.coverage = Some(coverage.clone());
                status.published_scan_receipt = state.receipt_id.clone();
                status.published_scan_generation = Some(attempt.generation);
                if state.received || state.step_failure.is_none() {
                    status.last_received_at = Some(chrono::Utc::now().to_rfc3339());
                }
            }
        }
        if current {
            let settled =
                state.receipt_id.is_some() && status.settled_scan_receipt == state.receipt_id;
            if let Some(failure) = &state.step_failure {
                status.phase = if failure.not_asked {
                    Phase::NotAsked
                } else {
                    if !settled {
                        status.consecutive_failures = status.consecutive_failures.saturating_add(1);
                    }
                    if failure.transient && status.consecutive_failures < 2 {
                        Phase::Retrying
                    } else {
                        Phase::Failed
                    }
                };
                status.error = Some(failure.message.clone());
            } else {
                status.phase = match coverage {
                    Coverage::Complete => Phase::Ready,
                    Coverage::Partial { .. } => Phase::Partial,
                    Coverage::Unknown => Phase::Unknown,
                };
                status.error = state.partition_reason();
                status.consecutive_failures = 0;
            }
            status.settled_generation = attempt.generation;
            status.settled_scan_receipt = state.receipt_id.clone();
            status.settled_phase = Some(status.phase.clone());
        }
        Some(status.clone())
    }
    fn complete_gitlab(
        &self,
        publication: Publication,
        result: Result<crate::gitlab::queues::FetchedList, Failure>,
        emit: impl FnOnce(Status),
    ) {
        let session = result.as_ref().ok().and_then(|r| r.session.clone());
        let complete = || {
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
        };
        if let Some(session) = session {
            session.with_current(complete);
        } else {
            complete();
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

    pub(crate) fn session_id(&self) -> &str {
        &self.3
    }

    fn update(&self, status: Status, completed_request: Option<String>) -> Update {
        let receipt = self
            .2
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(status.source.clone(), status.list))
            .map(|(_, receipt)| (receipt.prs.clone(), receipt.viewer.clone()));
        let (prs, owner) = receipt
            .map(|(rows, owner)| (Some(rows), owner))
            .unwrap_or_default();
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
            owner,
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
        if status.phase != Phase::Fetching {
            status.settled_phase = Some(status.phase.clone());
        }
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
        status.settled_generation = attempt.generation;
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

/// Capture the exact accepted receipt inside `complete`'s publication permit,
/// before another publication can supersede it.
pub(crate) fn complete_with_checking(
    app: &AppHandle,
    publication: Publication,
    result: FetchedList,
) -> Option<CheckingCapture> {
    let attempt = publication.attempt.clone();
    let polls = app.state::<SourcePolls>();
    let mut capture = None;
    polls.complete(publication, Ok(result), |status| {
        capture = polls.capture_checking(&attempt);
        emit_status(app, status, attempt.request_id.clone());
    });
    capture
}

pub(crate) async fn checking_publication(
    app: &AppHandle,
    capture: &CheckingCapture,
) -> Option<Publication> {
    let polls = app.state::<SourcePolls>();
    let permit = polls.publication(&capture.attempt).await?;
    polls.checking_receipt(capture)?;
    Some(permit)
}

pub(crate) async fn apply_checking(
    app: &AppHandle,
    publication: &Publication,
    capture: &CheckingCapture,
    rows: Vec<crate::github::model::PullRequest>,
) -> Result<Option<FetchedList>, Failure> {
    if publication.attempt.source != capture.attempt.source
        || publication.attempt.list != capture.attempt.list
        || publication.attempt.generation != capture.attempt.generation
    {
        return Ok(None);
    }
    let Some(previous) = app.state::<SourcePolls>().checking_receipt(capture) else {
        return Ok(None);
    };
    let capture = capture.clone();
    let path = crate::commands::db_path(app);
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::store::open_db(&path)
            .map_err(|_| inventory_failure("The saved inventory could not be read."))?;
        apply_checking_snapshot(&conn, &capture, previous, rows)
    })
    .await
    .map_err(|_| inventory_failure("The targeted observation could not be saved."))?
}

pub(crate) fn complete_checking(app: &AppHandle, publication: Publication, result: FetchedList) {
    app.state::<SourcePolls>()
        .complete_checking(publication, result, |status| emit_status(app, status, None));
}

/// A row-only transaction. Never invoke queue_scan::commit here: even an
/// unchanged scan payload would advance its revision and operation authority.
fn apply_checking_snapshot(
    conn: &rusqlite::Connection,
    capture: &CheckingCapture,
    mut previous: FetchedList,
    rows: Vec<crate::github::model::PullRequest>,
) -> Result<Option<FetchedList>, Failure> {
    let save = || -> Result<Option<FetchedList>, crate::store::StoreError> {
        let tx = conn.unchecked_transaction()?;
        let source = &capture.attempt.source;
        let list = capture.attempt.list;
        if previous.viewer.as_deref() != Some(&capture.owner)
            || crate::store::source_cache::snapshot_owner(&tx, source, list)?.as_deref()
                != Some(&capture.owner)
            || capture.scan_revision.is_some_and(|revision| {
                crate::queue_scan::load(&tx, source, list, &capture.owner)
                    .map_or(true, |s| s.revision != revision)
            })
        {
            return Ok(None);
        }
        let mut changed = false;
        // Preserve the exact order and qualification of every omitted row.
        for old in &mut previous.prs {
            let Some(target) = capture.targets.iter().find(|t| t.matches(old)) else {
                continue;
            };
            if old.merge != crate::github::model::MergeState::Checking {
                continue;
            }
            let Some(new) = rows.iter().find(|row| target.matches(row)) else {
                continue;
            };
            *old = crate::inventory::reconcile_delta(
                vec![old.clone()],
                vec![new.clone()],
                chrono::Utc::now(),
            )
            .pop()
            .expect("one existing matched row");
            changed = true;
        }
        if !changed {
            return Ok(None);
        }
        crate::store::source_cache::save_owned_source_snapshot(
            &tx,
            source,
            list,
            &previous.prs,
            &previous.coverage,
            Some(&capture.owner),
        )?;
        tx.commit()?;
        Ok(Some(previous))
    };
    save().map_err(|_| inventory_failure("The targeted observation could not be saved."))
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

/// Load a durable checkpoint only after authenticating this immutable client.
/// The operation allowance also covers a cold viewer lookup.
pub async fn fetch_github_step(
    app: &AppHandle,
    client: &crate::github::client::GitHubClient,
    list: CachedList,
    budget: std::time::Duration,
) -> Result<FetchedList, crate::github::client::ClientError> {
    fetch_github_step_mode(app, client, list, budget, false).await
}

pub(crate) async fn fetch_github_continuation(
    app: &AppHandle,
    client: &crate::github::client::GitHubClient,
    list: CachedList,
    budget: std::time::Duration,
) -> Result<FetchedList, crate::github::client::ClientError> {
    fetch_github_step_mode(app, client, list, budget, true).await
}

async fn fetch_github_step_mode(
    app: &AppHandle,
    client: &crate::github::client::GitHubClient,
    list: CachedList,
    budget: std::time::Duration,
    continuation: bool,
) -> Result<FetchedList, crate::github::client::ClientError> {
    fetch_github_step_at(
        crate::commands::db_path(app),
        client,
        list,
        budget,
        continuation,
        || {
            (
                app.state::<crate::poll::PollInterval>()
                    .0
                    .load(std::sync::atomic::Ordering::Relaxed),
                chrono::Utc::now().timestamp(),
            )
        },
    )
    .await
}

/// Shared durable fetch boundary for Tauri scheduling and synthetic runtime
/// tests. The caller supplies the clock reading and configured interval; all
/// authentication, owner checks, attempt bounds and continuation rechecks live here.
pub(crate) async fn fetch_github_step_at(
    path: std::path::PathBuf,
    client: &crate::github::client::GitHubClient,
    list: CachedList,
    budget: std::time::Duration,
    continuation: bool,
    timing: impl FnOnce() -> (u64, i64),
) -> Result<FetchedList, crate::github::client::ClientError> {
    use crate::github::{admission::ReadContext, client::ClientError};
    let client = client
        .with_read_context(ReadContext::new(client.read_context().class, budget))
        .with_attempt_limit(3);
    let viewer = match client.known_viewer().filter(|v| !v.is_empty()) {
        Some(v) => v.to_string(),
        None => client.fetch_viewer().await?,
    };
    let owner = viewer.clone();
    let (mut loaded, rows) =
        tauri::async_runtime::spawn_blocking(move || -> Result<_, crate::store::StoreError> {
            let conn = crate::store::open_db(&path)?;
            let conn = conn.unchecked_transaction()?;
            let source = Source::default();
            let checkpoint = crate::queue_scan::load(&conn, &source, list, &owner)?;
            let rows = if crate::store::source_cache::snapshot_owner(&conn, &source, list)?
                .as_deref()
                == Some(&owner)
            {
                match crate::store::source_cache::load_source_snapshot(&conn, &source, list)?.data {
                    crate::store::source_cache::SnapshotData::Available { prs, .. } => prs,
                    _ => vec![],
                }
            } else {
                vec![]
            };
            Ok((checkpoint, rows))
        })
        .await
        .map_err(|_| ClientError::Graphql("queue checkpoint could not be loaded".into()))?
        .map_err(|_| ClientError::Graphql("queue checkpoint could not be loaded".into()))?;
    // Match the desktop path's timing: a blocked checkpoint load must not
    // freeze an old clock reading or a superseded interval setting.
    let (interval_secs, now) = timing();
    loaded.state.pass_delay = crate::poll::clamp_interval(interval_secs) as i64;
    if continuation && !crate::poll::queue_continuation_due(&loaded.state, now, true, true, true) {
        loaded.state.no_work = true;
        return Ok(FetchedList {
            viewer: Some(viewer),
            prs: vec![],
            total: loaded.state.total,
            coverage: if loaded.state.coverage_valid {
                Coverage::Complete
            } else {
                Coverage::Partial {
                    total: loaded.state.total,
                }
            },
            scan: Some(crate::queue_scan::Commit {
                expected_revision: loaded.revision,
                state: loaded.state,
                removals: vec![],
            }),
        });
    }
    if continuation {
        client
            .advance_scan_mode(
                list,
                loaded,
                &rows,
                now,
                crate::github::scan::ScanMode::Continue,
            )
            .await
    } else {
        client.advance_scan(list, loaded, &rows, now).await
    }
}

/// Called with the source publication permit held, before persistence and emission.
/// The last committed receipt wins over disk; disk is only the restart baseline.
pub async fn reconcile_github(
    app: &AppHandle,
    publication: &Publication,
    result: FetchedList,
) -> Result<FetchedList, Failure> {
    reconcile_github_at(
        crate::commands::db_path(app),
        &app.state::<SourcePolls>(),
        publication,
        result,
    )
    .await
}

pub(crate) async fn reconcile_github_at(
    path: std::path::PathBuf,
    polls: &SourcePolls,
    publication: &Publication,
    result: FetchedList,
) -> Result<FetchedList, Failure> {
    let list = publication.attempt.list;
    let source = publication.attempt.source.clone();
    let previous = polls
        .2
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(source.clone(), list))
        .map(|(_, receipt)| receipt.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::store::open_db(&path)
            .map_err(|_| inventory_failure("The saved inventory could not be read."))?;
        reconcile_github_snapshot(&conn, &source, list, result, previous)
    })
    .await
    .map_err(|_| inventory_failure("The inventory refresh could not be completed."))?
}

fn inventory_failure(message: &str) -> Failure {
    Failure {
        message: message.into(),
        transient: true,
        not_asked: false,
    }
}

/// Publication preparation is shared by foreground, background and recheck.
/// Never guess ownership from disk: an unverified response is an error, not a
/// replacement receipt. The caller completes that failure without saving rows.
fn reconcile_github_snapshot(
    conn: &rusqlite::Connection,
    source: &Source,
    list: CachedList,
    mut result: FetchedList,
    previous: Option<FetchedList>,
) -> Result<FetchedList, Failure> {
    let owner = result
        .viewer
        .as_deref()
        .filter(|viewer| !viewer.is_empty())
        .ok_or_else(|| {
            inventory_failure(
                "The account could not be confirmed. Showing the last known inventory.",
            )
        })?;
    if let Some(scan) = &result.scan {
        if crate::queue_scan::accepted(conn, source, list, owner, scan)
            .map_err(|_| inventory_failure("The saved queue receipt could not be read."))?
        {
            if let crate::store::source_cache::SnapshotData::Available { prs, coverage, .. } =
                crate::store::source_cache::load_source_snapshot(conn, source, list)
                    .map_err(|_| inventory_failure("The saved inventory could not be read."))?
                    .data
            {
                result.prs = prs;
                result.coverage = coverage;
                // This is adoption of an actual accepted operation, not a
                // cooldown/no-work result. Its outcome still belongs to the caller.
                return Ok(result);
            }
            return Err(inventory_failure(
                "The accepted queue inventory is unavailable.",
            ));
        }
    }
    let previous = if let Some(receipt) =
        previous.filter(|receipt| receipt.viewer.as_deref() == Some(owner))
    {
        receipt.prs
    } else if crate::store::source_cache::snapshot_owner(conn, source, list)
        .map_err(|_| inventory_failure("The saved inventory owner could not be read."))?
        .as_deref()
        == Some(owner)
    {
        match crate::store::source_cache::load_source_snapshot(conn, source, list)
            .map_err(|_| inventory_failure("The saved inventory could not be read."))?
            .data
        {
            crate::store::source_cache::SnapshotData::Available { prs, .. } => prs,
            _ => vec![],
        }
    } else {
        vec![]
    };
    if let Some(scan) = result.scan.as_ref().filter(|s| s.state.no_work) {
        let loaded = crate::queue_scan::load(conn, source, list, owner)
            .map_err(|_| inventory_failure("The queue checkpoint could not be read."))?;
        if loaded.revision != scan.expected_revision {
            return Err(inventory_failure(
                "The queue changed during this step. Its newer progress was retained.",
            ));
        }
        result.prs = previous;
        if let crate::store::source_cache::SnapshotData::Available { coverage, .. } =
            crate::store::source_cache::load_source_snapshot(conn, source, list)
                .map_err(|_| inventory_failure("The saved inventory could not be read."))?
                .data
        {
            result.coverage = coverage;
        }
        return Ok(result);
    }
    // A finite step observes only its returned rows. A failed page (including
    // the optional head probe) does not invalidate other accepted observations.
    // The scan failure remains on the list status and durable checkpoint.
    result.prs = if result.scan.is_some() {
        crate::inventory::reconcile_delta(previous, result.prs, chrono::Utc::now())
    } else {
        crate::inventory::reconcile(
            previous,
            result.prs,
            matches!(result.coverage, Coverage::Complete),
            chrono::Utc::now(),
        )
    };
    if let Some(scan) = &result.scan {
        result
            .prs
            .retain(|row| !scan.removals.contains(&row.identity()));
        if scan.state.done {
            for row in &mut result.prs {
                if !scan.state.seen.contains(&row.identity()) {
                    if let Some(observation) = row.observation.as_mut() {
                        observation.state = crate::inventory::ObservationState::Retained;
                    }
                }
            }
        }
        let committed = crate::queue_scan::commit(conn, source, list, owner, scan, |tx| {
            crate::store::github_facts::apply(tx, list, owner, &mut result.prs)?;
            if scan.state.step_failure.is_some() && !scan.state.received {
                return crate::store::source_cache::save_owned_source_failure(
                    tx,
                    source,
                    list,
                    &result.prs,
                    &result.coverage,
                    owner,
                );
            }
            crate::store::source_cache::save_owned_source_snapshot(
                tx,
                source,
                list,
                &result.prs,
                &result.coverage,
                Some(owner),
            )
        })
        .map_err(|_| inventory_failure("The queue progress could not be saved."))?;
        if !committed {
            return Err(inventory_failure(
                "The queue changed during this step. Its newer progress was retained.",
            ));
        }
    } else {
        let mut save = || -> Result<(), crate::store::StoreError> {
            let tx = conn.unchecked_transaction()?;
            crate::store::github_facts::apply(&tx, list, owner, &mut result.prs)?;
            crate::store::source_cache::save_owned_source_snapshot(
                &tx,
                source,
                list,
                &result.prs,
                &result.coverage,
                Some(owner),
            )?;
            tx.commit()?;
            Ok(())
        };
        save()
            .map_err(|_| inventory_failure("Saved pull request facts could not be reconciled."))?;
    }
    Ok(result)
}

/// GitLab uses the same qualified inventory semantics as GitHub.
pub async fn reconcile_gitlab(
    app: &AppHandle,
    publication: &Publication,
    result: crate::gitlab::queues::FetchedList,
) -> Result<crate::gitlab::queues::FetchedList, Failure> {
    let source = publication.attempt.source.clone();
    let list = publication.attempt.list;
    let previous = app
        .state::<SourcePolls>()
        .4
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(source.clone(), list))
        .map(|(_, r)| r.clone());
    let path = crate::commands::db_path(app);
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::store::open_db(&path)
            .map_err(|_| inventory_failure("The saved inventory could not be read."))?;
        reconcile_gitlab_snapshot(&conn, &source, list, result, previous)
    })
    .await
    .map_err(|_| inventory_failure("The queue step could not be completed."))?
}
fn reconcile_gitlab_snapshot(
    conn: &rusqlite::Connection,
    source: &Source,
    list: CachedList,
    result: crate::gitlab::queues::FetchedList,
    previous: Option<crate::gitlab::queues::FetchedList>,
) -> Result<crate::gitlab::queues::FetchedList, Failure> {
    let session = result.session.clone();
    let reconcile = || reconcile_gitlab_snapshot_current(conn, source, list, result, previous);
    match session {
        Some(owner) => owner.with_current(reconcile).unwrap_or_else(|| {
            Err(inventory_failure(
                "The GitLab account changed before publication.",
            ))
        }),
        None => reconcile(),
    }
}
fn reconcile_gitlab_snapshot_current(
    conn: &rusqlite::Connection,
    source: &Source,
    list: CachedList,
    mut result: crate::gitlab::queues::FetchedList,
    previous: Option<crate::gitlab::queues::FetchedList>,
) -> Result<crate::gitlab::queues::FetchedList, Failure> {
    let owner = result
        .viewer
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| inventory_failure("The GitLab account could not be confirmed."))?;
    if let Some(scan) = &result.scan {
        if crate::queue_scan::accepted(conn, source, list, owner, scan)
            .map_err(|_| inventory_failure("The saved GitLab queue receipt could not be read."))?
        {
            if let crate::store::source_cache::SnapshotData::GitLabAvailable {
                mrs, coverage, ..
            } = crate::store::source_cache::load_source_snapshot(conn, source, list)
                .map_err(|_| inventory_failure("The saved inventory could not be read."))?
                .data
            {
                result.mrs = mrs;
                result.coverage = coverage;
                return Ok(result);
            }
            return Err(inventory_failure(
                "The accepted GitLab inventory is unavailable.",
            ));
        }
    }
    let previous = if let Some(old) = previous.filter(|r| r.viewer.as_deref() == Some(owner)) {
        old.mrs
    } else if crate::store::source_cache::snapshot_owner(conn, source, list)
        .map_err(|_| inventory_failure("The saved owner could not be read."))?
        .as_deref()
        == Some(owner)
    {
        match crate::store::source_cache::load_source_snapshot(conn, source, list)
            .map_err(|_| inventory_failure("The saved inventory could not be read."))?
            .data
        {
            crate::store::source_cache::SnapshotData::GitLabAvailable { mrs, .. } => mrs,
            _ => vec![],
        }
    } else {
        vec![]
    };
    for row in &mut result.mrs {
        row.observation = Some(crate::inventory::gitlab_observation(row));
    }
    result.mrs = crate::inventory::reconcile(
        previous
            .into_iter()
            .filter(|r| r.viewer.as_deref() == Some(owner))
            .collect(),
        result.mrs,
        matches!(result.coverage, Coverage::Complete),
        chrono::Utc::now(),
    );
    if let Some(scan) = &result.scan {
        result
            .mrs
            .retain(|r| !scan.removals.contains(&r.identity()));
        let committed = crate::queue_scan::commit(conn, source, list, owner, scan, |tx| {
            crate::store::source_cache::save_owned_gitlab_snapshot(
                tx,
                source,
                list,
                &result.mrs,
                &result.coverage,
                Some(owner),
            )
        })
        .map_err(|_| inventory_failure("The GitLab queue progress could not be saved."))?;
        if !committed {
            return Err(inventory_failure(
                "The GitLab queue changed during this step. Newer progress was retained.",
            ));
        }
    }
    Ok(result)
}

enum GithubEffect {
    Facts(crate::store::github_facts::Observation),
    ReviewReadback(Box<crate::github::model::PrDetail>, [u64; 2]),
    QualifyReview(crate::github::mutate::SubmittedReview),
    Review(crate::inventory::ConfirmedReview),
    Remove(crate::github::mutate::ConfirmedRemoval),
}
pub async fn fact_operation(
    app: &AppHandle,
    repo: &str,
    number: u64,
) -> crate::store::github_facts::Operation {
    app.state::<SourcePolls>()
        .fact_operation_for(repo, number)
        .await
}
impl SourcePolls {
    #[cfg(test)]
    async fn fact_operation(&self) -> crate::store::github_facts::Operation {
        self.fact_operation_for("", 0).await
    }
    async fn fact_operation_for(
        &self,
        repo: &str,
        number: u64,
    ) -> crate::store::github_facts::Operation {
        let mut heads_at_start = [None, None];
        let mut receipts_at_start = [None, None];
        for (index, list) in [CachedList::Authored, CachedList::Reviewing]
            .into_iter()
            .enumerate()
        {
            // Snapshot this identity and receipt coherently with publication.
            // Each gate is released before taking the next; independent batch
            // acknowledgments are not fenced by unrelated receipt revisions.
            let _guard = self.gate(&Source::default(), list).lock_owned().await;
            heads_at_start[index] = self
                .2
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(Source::default(), list))
                .and_then(|(_, receipt)| {
                    receipt
                        .prs
                        .iter()
                        .find(|row| row.repo.eq_ignore_ascii_case(repo) && row.number == number)
                })
                .map(|row| crate::store::github_facts::HeadAtStart {
                    id: row.id.clone(),
                    head_oid: row.head_oid.clone(),
                });
            receipts_at_start[index] = self.get(&Source::default(), list).receipt_revision;
        }
        crate::store::github_facts::Operation {
            session: self.3.clone(),
            sequence: self.6.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1,
            heads_at_start,
            receipts_at_start,
        }
    }
}
pub fn review_read_generation(app: &AppHandle) -> [u64; 2] {
    let polls = app.state::<SourcePolls>();
    let generations = polls.5.lock().unwrap_or_else(|e| e.into_inner());
    [CachedList::Authored, CachedList::Reviewing]
        .map(|list| *generations.get(&(Source::default(), list)).unwrap_or(&0))
}
pub async fn record_review_readback(
    app: &AppHandle,
    detail: &crate::github::model::PrDetail,
    viewer: &str,
    generations: [u64; 2],
) {
    record_github_effect(
        app,
        &detail.repo,
        detail.number,
        viewer,
        GithubEffect::ReviewReadback(Box::new(detail.clone()), generations),
    )
    .await;
}
pub async fn qualify_confirmed_review(
    app: &AppHandle,
    receipt: &crate::github::mutate::SubmittedReview,
) {
    record_github_effect(
        app,
        &receipt.repo,
        receipt.number,
        &receipt.actor,
        GithubEffect::QualifyReview(receipt.clone()),
    )
    .await;
}
pub async fn record_confirmed_review(
    app: &AppHandle,
    repo: &str,
    number: u64,
    viewer: &str,
    effect: crate::inventory::ConfirmedReview,
) {
    record_github_effect(app, repo, number, viewer, GithubEffect::Review(effect)).await;
}
pub async fn record_confirmed_action(
    app: &AppHandle,
    repo: &str,
    number: u64,
    action: crate::github::mutate::ConfirmedAction,
    operation: crate::store::github_facts::Operation,
) {
    match action {
        crate::github::mutate::ConfirmedAction::Removal(effect) => {
            record_confirmed_removal(app, repo, number, effect).await
        }
        crate::github::mutate::ConfirmedAction::Facts {
            viewer,
            mut observation,
        } => {
            for fact in &mut observation.facts {
                fact.operation = Some(operation.clone());
            }
            record_github_effect(app, repo, number, &viewer, GithubEffect::Facts(observation)).await
        }
    }
}

pub async fn record_confirmed_removal(
    app: &AppHandle,
    repo: &str,
    number: u64,
    effect: crate::github::mutate::ConfirmedRemoval,
) {
    let viewer = effect.viewer.clone();
    record_github_effect(app, repo, number, &viewer, GithubEffect::Remove(effect)).await;
}

fn persist_github_effect(
    conn: &rusqlite::Connection,
    source: &Source,
    list: CachedList,
    receipt: &FetchedList,
) -> Result<(), crate::store::StoreError> {
    let tx = conn.unchecked_transaction()?;
    if crate::store::source_cache::snapshot_owner(&tx, source, list)?.as_deref()
        == receipt.viewer.as_deref()
        && receipt.viewer.is_some()
    {
        crate::store::source_cache::save_owned_source_failure(
            &tx,
            source,
            list,
            &receipt.prs,
            &receipt.coverage,
            receipt.viewer.as_deref().unwrap_or_default(),
        )?;
    } else {
        crate::store::source_cache::save_owned_source_snapshot(
            &tx,
            source,
            list,
            &receipt.prs,
            &receipt.coverage,
            receipt.viewer.as_deref(),
        )?;
    }
    crate::queue_scan::taint(
        &tx,
        source,
        list,
        receipt.viewer.as_deref().unwrap_or_default(),
    )?;
    tx.commit()?;
    Ok(())
}

async fn record_github_effect(
    app: &AppHandle,
    repo: &str,
    number: u64,
    viewer: &str,
    effect: GithubEffect,
) {
    record_github_effect_at(
        crate::commands::db_path(app),
        &app.state::<SourcePolls>(),
        repo,
        number,
        viewer,
        effect,
        |event| match event {
            Ok(update) => {
                let _ = app.emit("source-poll-status", update);
            }
            Err(message) => {
                let _ = app.emit("store-error", message);
            }
        },
    )
    .await;
}

// Shared receipt transaction; the platform shell only delivers its events.
async fn record_github_effect_at(
    path: std::path::PathBuf,
    polls: &SourcePolls,
    repo: &str,
    number: u64,
    viewer: &str,
    effect: GithubEffect,
    mut emit: impl FnMut(Result<Update, &'static str>),
) {
    let source = Source::default();
    for list in [CachedList::Authored, CachedList::Reviewing] {
        let _guard = polls.gate(&source, list).lock_owned().await;
        let existing = polls
            .2
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(source.clone(), list))
            .filter(|(_, r)| r.viewer.as_deref() == Some(viewer))
            .map(|(_, r)| r.clone());
        let mut receipt = if let Some(receipt) = existing {
            receipt
        } else {
            let path = path.clone();
            let source = source.clone();
            let owner = viewer.to_string();
            let rows = tauri::async_runtime::spawn_blocking(move || {
                let conn = crate::store::open_db(&path).ok()?;
                if crate::store::source_cache::snapshot_owner(&conn, &source, list)
                    .ok()?
                    .as_deref()
                    != Some(&owner)
                {
                    return None;
                }
                match crate::store::source_cache::load_source_snapshot(&conn, &source, list)
                    .ok()?
                    .data
                {
                    crate::store::source_cache::SnapshotData::Available {
                        prs, coverage, ..
                    } => Some((prs, coverage)),
                    _ => None,
                }
            })
            .await
            .ok()
            .flatten();
            let Some((prs, coverage)) = rows else {
                continue;
            };
            FetchedList {
                scan: None,
                viewer: Some(viewer.into()),
                prs,
                total: None,
                coverage,
            }
        };
        let observation = match &effect {
            GithubEffect::Facts(observation) => Some(observation.clone()),
            GithubEffect::ReviewReadback(detail, generations) => {
                let index = if list == CachedList::Authored { 0 } else { 1 };
                if detail
                    .inventory_facts
                    .as_ref()
                    .is_none_or(|o| o.facts.iter().any(|f| f.operation.is_none()))
                    && !polls.readback_is_current(&source, list, generations[index])
                {
                    continue;
                }
                detail.inventory_facts.clone()
            }
            _ => None,
        };
        let mut facts_changed = false;
        if let Some(observation) = observation {
            // Positive replacement identity/head evidence cannot be changed by
            // an older per-PR response. Terminal journal entries also permit a
            // later targeted reopen even though no open row remains.
            let has_row = receipt
                .prs
                .iter()
                .any(|row| row.repo.eq_ignore_ascii_case(repo) && row.number == number);
            let matches = receipt.prs.iter().any(|row| {
                row.repo.eq_ignore_ascii_case(repo)
                    && row.number == number
                    && row.id == observation.id
                    && observation.facts.iter().all(|f| {
                        (f.head_oid == row.head_oid
                            || f.updated_at.is_some_and(|version| version > row.updated_at)
                            || (f.operation.as_ref().is_some_and(|operation| {
                                let index = if list == CachedList::Authored { 0 } else { 1 };
                                operation.session == polls.3
                                    && match &operation.heads_at_start[index] {
                                        Some(start) => {
                                            start.id == row.id && start.head_oid == row.head_oid
                                        }
                                        None => {
                                            operation.receipts_at_start[index]
                                                == polls.get(&source, list).receipt_revision
                                        }
                                    }
                            }) && f.updated_at == Some(row.updated_at)
                                && row
                                    .observation
                                    .as_ref()
                                    .and_then(|o| o.last_observed_at)
                                    .is_none_or(|time| f.observed_at >= time)))
                            && f.updated_at.is_none_or(|version| version >= row.updated_at)
                    })
            });
            let path = path.clone();
            let repo = repo.to_owned();
            let owner = viewer.to_owned();
            let mut next = receipt.clone();
            let saved = tauri::async_runtime::spawn_blocking(
                move || -> Result<_, crate::store::StoreError> {
                    let conn = crate::store::open_db(&path)?;
                    let tx = conn.unchecked_transaction()?;
                    // If membership is absent, accept only an identity already
                    // journaled by this owner, never invent a new open row.
                    if !matches
                        && (has_row
                            || !crate::store::github_facts::contains(
                                &tx,
                                list,
                                &owner,
                                &repo,
                                number,
                                &observation.id,
                            )?)
                    {
                        return Ok(None);
                    }
                    let changed = crate::store::github_facts::accept(
                        &tx,
                        list,
                        &owner,
                        &repo,
                        number,
                        &observation,
                    )?;
                    if !changed {
                        return Ok(None);
                    }
                    crate::store::github_facts::apply_targeted(
                        &tx,
                        list,
                        &owner,
                        &mut next.prs,
                        &observation,
                    )?;
                    crate::store::source_cache::save_owned_source_failure(
                        &tx,
                        &Source::default(),
                        list,
                        &next.prs,
                        &next.coverage,
                        &owner,
                    )?;
                    crate::queue_scan::taint(&tx, &Source::default(), list, &owner)?;
                    tx.commit()?;
                    Ok(Some(next))
                },
            )
            .await;
            match saved {
                Ok(Ok(Some(next))) => { receipt = next; facts_changed = true; }
                Ok(Ok(None)) => {}
                _ => emit(Err("New pull request facts could not be saved. The detail remains available; saved lists may be older.")),
            }
        }
        let changed = match &effect {
            GithubEffect::Facts(..) => false,
            GithubEffect::QualifyReview(submitted) => {
                let mut changed = false;
                for row in &mut receipt.prs {
                    if row.repo == repo && row.number == number {
                        if let Some(effect) = row
                            .observation
                            .as_mut()
                            .and_then(|o| o.confirmed_review.as_mut())
                        {
                            if effect.receipt.as_ref().is_some_and(|r| {
                                r.review_id == submitted.review_id
                                    && r.actor == submitted.actor
                                    && r.commit_oid == submitted.commit_oid
                            }) {
                                let before = effect.unresolved;
                                crate::inventory::qualify_review_effect(effect, chrono::Utc::now());
                                changed |= effect.unresolved != before;
                            }
                        }
                    }
                }
                changed
            }
            GithubEffect::ReviewReadback(detail, generations) => {
                let index = if list == CachedList::Authored { 0 } else { 1 };
                let mut changed = false;
                if polls.readback_is_current(&source, list, generations[index]) {
                    for row in &mut receipt.prs {
                        if row.repo == repo && row.number == number && row.id == detail.id {
                            changed |= crate::inventory::reconcile_detail_review(row, detail);
                        }
                    }
                }
                changed
            }
            GithubEffect::Review(effect) => {
                let mut changed = false;
                for row in &mut receipt.prs {
                    if row.repo == repo && row.number == number {
                        changed |= crate::inventory::apply_confirmed_review(row, effect);
                    }
                }
                changed
            }
            GithubEffect::Remove(effect) => {
                crate::inventory::apply_github_removal(&mut receipt.prs, repo, number, effect)
            }
        };
        if !changed && !facts_changed {
            continue;
        }
        // The effect itself advances generation before any old request can publish.
        let generation = polls.effect_generation(&source, list);
        let path = path.clone();
        let saved = receipt.clone();
        let saved_source = source.clone();
        let persisted = tauri::async_runtime::spawn_blocking(move || {
            let conn = crate::store::open_db(&path)?;
            persist_github_effect(&conn, &saved_source, list, &saved)
        })
        .await;
        if !matches!(persisted, Ok(Ok(()))) {
            emit(Err(
                "The confirmed review could not be saved for offline use.",
            ));
        }
        polls
            .2
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((source.clone(), list), (generation, receipt.clone()));
        let mut entries = polls.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, status)) = entries.get_mut(&(source.clone(), list)) {
            status.revision += 1;
            status.receipt_revision = Some(status.revision);
            status.coverage = Some(receipt.coverage);
            status.phase = Phase::Unknown;
            status.settled_generation = generation;
            let update = polls.update(status.clone(), None);
            emit(Ok(update));
        }
    }
}

/// GitLab execute has already validated expected viewer/head and read back the
/// write. Unverified receipts do not modify durable inventory.
pub async fn record_gitlab_action(
    app: &AppHandle,
    request: &crate::gitlab::actions::ActionRequest,
    action: &crate::gitlab::actions::Receipt,
) {
    if action.outcome != crate::gitlab::actions::Outcome::Verified {
        return;
    }
    let Some(session) = action.session.as_ref() else {
        return;
    };
    let Some(viewer) = request.expected_viewer.as_ref().filter(|v| !v.is_empty()) else {
        return;
    };
    let source = &request.identity.source;
    let polls = app.state::<SourcePolls>();
    for list in [CachedList::Authored, CachedList::Reviewing] {
        let _guard = polls.gate(source, list).lock_owned().await;
        let existing = polls
            .4
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(source.clone(), list))
            .filter(|(_, r)| r.viewer.as_ref() == Some(viewer))
            .map(|(_, r)| r.clone());
        let mut receipt = if let Some(receipt) = existing {
            receipt
        } else {
            let path = crate::commands::db_path(app);
            let source = source.clone();
            let owner = viewer.clone();
            let cached = tauri::async_runtime::spawn_blocking(move || {
                let conn = crate::store::open_db(&path).ok()?;
                if crate::store::source_cache::snapshot_owner(&conn, &source, list)
                    .ok()?
                    .as_ref()
                    != Some(&owner)
                {
                    return None;
                }
                match crate::store::source_cache::load_source_snapshot(&conn, &source, list)
                    .ok()?
                    .data
                {
                    crate::store::source_cache::SnapshotData::GitLabAvailable {
                        mrs,
                        coverage,
                        ..
                    } => Some((mrs, coverage)),
                    _ => None,
                }
            })
            .await
            .ok()
            .flatten();
            let Some((mrs, coverage)) = cached else {
                continue;
            };
            crate::gitlab::queues::FetchedList {
                session: None,
                scan: None,
                viewer: Some(viewer.clone()),
                mrs,
                total: None,
                coverage,
            }
        };
        if !crate::inventory::apply_gitlab_action(&mut receipt.mrs, request, action) {
            continue;
        }
        receipt.session = Some(session.clone());
        let path = crate::commands::db_path(app);
        let saved_source = source.clone();
        let saved = receipt.clone();
        let saved_session = session.clone();
        let persisted = tauri::async_runtime::spawn_blocking(move || {
            saved_session.with_current(|| {
                let conn = crate::store::open_db(&path)?;
                let tx = conn.unchecked_transaction()?;
                crate::store::source_cache::save_owned_gitlab_snapshot(
                    &tx,
                    &saved_source,
                    list,
                    &saved.mrs,
                    &saved.coverage,
                    saved.viewer.as_deref(),
                )?;
                crate::queue_scan::taint(
                    &tx,
                    &saved_source,
                    list,
                    saved.viewer.as_deref().unwrap_or_default(),
                )?;
                tx.commit()?;
                Ok::<_, crate::store::StoreError>(())
            })
        })
        .await;
        if matches!(persisted, Ok(None)) {
            continue;
        }
        if !matches!(persisted, Ok(Some(Ok(())))) {
            let _ = app.emit(
                "store-error",
                "The confirmed action could not be saved for offline use.",
            );
        }
        session.with_current(|| {
            let generation = polls.effect_generation(source, list);
            polls
                .4
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((source.clone(), list), (generation, receipt.clone()));
            let mut entries = polls.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, status)) = entries.get_mut(&(source.clone(), list)) {
                status.revision += 1;
                status.receipt_revision = Some(status.revision);
                status.coverage = Some(receipt.coverage);
                status.phase = Phase::Unknown;
                let _ = app.emit("source-poll-status", polls.update(status.clone(), None));
            }
        });
    }
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
            scan: None,
            viewer: None,
            prs: vec![pr],
            total: Some(1),
            coverage: Coverage::Complete,
        }
    }

    #[tokio::test]
    async fn checking_recheck_reads_captured_row_after_completed_or_unfinished_scan() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        for done in [true, false] {
            let server = MockServer::start().await;
            let client = crate::github::client::GitHubClient::new(
                octocrab::Octocrab::builder()
                    .base_uri(server.uri())
                    .unwrap()
                    .personal_token("synthetic")
                    .build()
                    .unwrap(),
            );
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"data":{"viewer":{"login":"fixture"}}})),
                )
                .mount(&server)
                .await;
            client.fetch_viewer().await.unwrap();
            server.reset().await;
            let raw: serde_json::Value =
                serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
            let mut node = raw["authored"]["nodes"][0].clone();
            node["id"] = "target-id".into();
            node["headRefOid"] = "head-a".into();
            node["state"] = "OPEN".into();
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"data":{"viewer":{"login":"fixture"},"checking":node}}),
                ))
                .mount(&server)
                .await;
            let mut baseline = FetchedList {
                scan: None,
                viewer: Some("fixture".into()),
                prs: crate::github::map::map_search(
                    &serde_json::json!({"authored":{"nodes":[node]}}),
                ),
                total: Some(1),
                coverage: Coverage::Complete,
            };
            baseline.prs[0].merge = crate::github::model::MergeState::Checking;
            baseline.scan = Some(crate::queue_scan::Commit {
                expected_revision: 0,
                state: crate::queue_scan::State {
                    done,
                    coverage_valid: done,
                    started_at: Some(1000),
                    eligible_at: 1120,
                    completed_at: done.then_some(1000),
                    completed_total: done.then_some(1),
                    total: Some(1),
                    count_seen: true,
                    seen: vec![baseline.prs[0].identity()],
                    after: (!done).then(|| "tail".into()),
                    ..Default::default()
                },
                removals: vec![],
            });
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("checking.db");
            let conn = crate::store::open_db(&path).unwrap();
            let baseline = reconcile_github_snapshot(
                &conn,
                &Source::default(),
                CachedList::Authored,
                baseline,
                None,
            )
            .unwrap_or_else(|f| panic!("{}", f.message));
            let before =
                crate::queue_scan::load(&conn, &Source::default(), CachedList::Authored, "fixture")
                    .unwrap();
            let fresh = client
                .fetch_checking(
                    &crate::github::client::checking_targets(&baseline.prs),
                    "fixture",
                    std::time::Duration::from_secs(2),
                )
                .await;
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                1,
                "T+5 must read the captured Checking identity even when traversal is cadence-gated"
            );
            assert_eq!(fresh.len(), 1);
            assert_eq!(fresh[0].merge, crate::github::model::MergeState::Mergeable);
            assert_eq!(fresh[0].id, baseline.prs[0].id);
            let after =
                crate::queue_scan::load(&conn, &Source::default(), CachedList::Authored, "fixture")
                    .unwrap();
            assert_eq!(before.revision, after.revision);
            assert_eq!(before.state, after.state);
        }
    }

    async fn checking_seed(
        conn: &rusqlite::Connection,
        polls: &SourcePolls,
    ) -> (CheckingCapture, FetchedList) {
        let source = Source::default();
        let list = CachedList::Authored;
        let mut baseline = receipt(1);
        baseline.viewer = Some("fixture".into());
        baseline.prs[0].id = "id-1".into();
        baseline.prs[0].head_oid = "head".into();
        baseline.prs.push(receipt(2).prs.remove(0));
        baseline.coverage = Coverage::Partial { total: Some(100) };
        baseline.total = Some(100);
        baseline.scan = Some(crate::queue_scan::Commit {
            expected_revision: 0,
            state: crate::queue_scan::State {
                after: Some("tail-50".into()),
                cursors: vec!["tail-25".into()],
                github_partition: Some(Default::default()),
                completed_at: Some(500),
                completed_total: Some(100),
                receipt_id: Some("receipt-proof".into()),
                ..Default::default()
            },
            removals: vec![],
        });
        let baseline = reconcile_github_snapshot(conn, &source, list, baseline, None)
            .unwrap_or_else(|e| panic!("{}", e.message));
        let (attempt, _) = polls.begin_attempt(source, list).await;
        let mut capture = None;
        polls.complete(
            polls.publication(&attempt).await.unwrap(),
            Ok(baseline.clone()),
            |_| {
                capture = polls.capture_checking(&attempt);
            },
        );
        (capture.unwrap(), baseline)
    }
    fn raw_checkpoint(conn: &rusqlite::Connection) -> (i64, String) {
        conn.query_row("SELECT revision,payload FROM queue_scan", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
    }
    #[tokio::test]
    async fn checking_row_transaction_preserves_traversal_error_coverage_order_and_omitted_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let polls = SourcePolls::default();
        let (capture, mut baseline) = checking_seed(&conn, &polls).await;
        // Existing accepted memory wins over disk, including omitted row values.
        baseline.prs[1].title = "newer accepted memory".into();
        polls
            .2
            .lock()
            .unwrap()
            .get_mut(&(Source::default(), CachedList::Authored))
            .unwrap()
            .1 = baseline.clone();
        {
            let mut entries = polls.0.lock().unwrap();
            let status = &mut entries
                .get_mut(&(Source::default(), CachedList::Authored))
                .unwrap()
                .1;
            status.error = Some("earlier traversal failure".into());
            status.consecutive_failures = 2;
        }
        let checkpoint = raw_checkpoint(&conn);
        let status = polls.get(&Source::default(), CachedList::Authored);
        let unrequested = serde_json::to_value(&baseline.prs[1]).unwrap();
        let mut incoming = baseline.prs[0].clone();
        incoming.merge = crate::github::model::MergeState::Mergeable;
        let permit = polls.publication(&capture.attempt).await.unwrap();
        let current = polls.checking_receipt(&capture).unwrap();
        let result = apply_checking_snapshot(&conn, &capture, current, vec![incoming])
            .unwrap_or_else(|e| panic!("{}", e.message))
            .unwrap();
        assert_eq!(
            result.prs[0].merge,
            crate::github::model::MergeState::Mergeable
        );
        assert_eq!(serde_json::to_value(&result.prs[1]).unwrap(), unrequested);
        assert_eq!(result.coverage, baseline.coverage);
        assert_eq!(result.total, baseline.total);
        assert_eq!(raw_checkpoint(&conn), checkpoint);
        let mut emitted = false;
        polls.complete_checking(permit, result, |updated| {
            assert_eq!(updated.phase, status.phase);
            assert_eq!(updated.error, status.error);
            assert_eq!(updated.consecutive_failures, status.consecutive_failures);
            assert_eq!(updated.coverage, status.coverage);
            assert_eq!(
                updated.published_scan_receipt,
                status.published_scan_receipt
            );
            assert!(updated.receipt_revision > status.receipt_revision);
            emitted = true;
        });
        assert!(emitted);
        assert!(
            polls.checking_receipt(&capture).is_none(),
            "same-attempt receipt turnover retires original work"
        );
        assert_eq!(raw_checkpoint(&conn), checkpoint);
    }
    #[tokio::test]
    async fn checking_authority_rejects_attempt_receipt_owner_and_confirmed_action_turnover() {
        for change in ["attempt", "receipt", "owner", "action"] {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            crate::store::migrate(&conn).unwrap();
            let polls = SourcePolls::default();
            let (capture, mut baseline) = checking_seed(&conn, &polls).await;
            let source = Source::default();
            let list = CachedList::Authored;
            match change {
                "attempt" => {
                    polls.begin_attempt(source.clone(), list).await;
                }
                "action" => {
                    let _permit = polls.publication(&capture.attempt).await.unwrap();
                    polls.effect_generation(&source, list);
                }
                _ => {
                    if change == "owner" {
                        baseline.viewer = Some("new-owner".into());
                    }
                    polls.complete(
                        polls.publication(&capture.attempt).await.unwrap(),
                        Ok(baseline),
                        |_| {},
                    );
                }
            }
            assert!(polls.checking_receipt(&capture).is_none(), "{change}");
        }
    }
    #[tokio::test]
    async fn checking_transaction_skips_replaced_removed_heads_and_rolls_back_save_failure() {
        for change in [
            "head",
            "id",
            "removed",
            "owner",
            "disk_owner",
            "scan",
            "empty",
            "save",
        ] {
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            crate::store::migrate(&conn).unwrap();
            let polls = SourcePolls::default();
            let (capture, mut current) = checking_seed(&conn, &polls).await;
            let mut incoming = current.prs[0].clone();
            incoming.merge = crate::github::model::MergeState::Mergeable;
            match change {
                "head" => current.prs[0].head_oid = "new-head".into(),
                "id" => current.prs[0].id = "replacement".into(),
                "removed" => {
                    current.prs.remove(0);
                }
                "owner" => current.viewer = Some("other".into()),
                "disk_owner" => {
                    conn.execute("UPDATE snapshot SET owner='other'", [])
                        .unwrap();
                }
                "scan" => {
                    crate::queue_scan::taint(
                        &conn,
                        &Source::default(),
                        CachedList::Authored,
                        "fixture",
                    )
                    .unwrap();
                }
                "save" => {
                    conn.execute_batch("CREATE TRIGGER fail_checking BEFORE UPDATE ON snapshot BEGIN SELECT RAISE(FAIL, 'fixture save failed'); END;").unwrap();
                }
                _ => {}
            }
            let checkpoint = raw_checkpoint(&conn);
            let before = serde_json::to_value(
                crate::store::source_cache::load_source_snapshot(
                    &conn,
                    &Source::default(),
                    CachedList::Authored,
                )
                .unwrap(),
            )
            .unwrap();
            let result = apply_checking_snapshot(
                &conn,
                &capture,
                current,
                if change == "empty" {
                    vec![]
                } else {
                    vec![incoming]
                },
            );
            if change == "save" {
                assert!(result.is_err());
            } else {
                assert!(
                    result.unwrap_or_else(|e| panic!("{}", e.message)).is_none(),
                    "{change}"
                );
            }
            assert_eq!(raw_checkpoint(&conn), checkpoint);
            assert_eq!(
                serde_json::to_value(
                    crate::store::source_cache::load_source_snapshot(
                        &conn,
                        &Source::default(),
                        CachedList::Authored
                    )
                    .unwrap()
                )
                .unwrap(),
                before
            );
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
    async fn unread_viewer_cannot_destroy_owned_publication_or_restart_baseline() {
        use crate::store::source_cache::{
            load_source_snapshot, save_owned_source_snapshot, snapshot_owner, SnapshotData,
        };
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        for known in [false, true] {
            for empty in [false, true] {
                let server = MockServer::start().await;
                let client = crate::github::client::GitHubClient::new(
                    octocrab::Octocrab::builder()
                        .base_uri(server.uri())
                        .unwrap()
                        .personal_token("synthetic")
                        .build()
                        .unwrap(),
                );
                if known {
                    Mock::given(method("POST"))
                        .respond_with(ResponseTemplate::new(200).set_body_json(
                            serde_json::json!({"data":{"viewer":{"login":"fixture"}}}),
                        ))
                        .mount(&server)
                        .await;
                    assert_eq!(client.fetch_viewer().await.unwrap(), "fixture");
                    server.reset().await;
                }
                let mut raw: serde_json::Value =
                    serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
                for node in raw["authored"]["nodes"].as_array_mut().unwrap() {
                    node["headRefOid"] = serde_json::json!("head-a");
                }
                let mut baseline = FetchedList {
                    scan: None,
                    viewer: Some("fixture".into()),
                    prs: crate::github::map::map_list(&raw, "authored"),
                    total: Some(3),
                    coverage: Coverage::Complete,
                };
                crate::inventory::apply_confirmed_review(
                    &mut baseline.prs[1],
                    &crate::inventory::ConfirmedReview {
                        head_oid: "head-a".into(),
                        review: crate::github::model::ReviewState::Approved,
                        confirmed_at: chrono::Utc::now(),
                        receipt: None,
                        unresolved: false,
                        confirmed_by_read: false,
                    },
                );
                if empty {
                    baseline.prs.clear();
                    baseline.total = Some(0);
                }
                let nodes = if empty {
                    vec![]
                } else {
                    vec![raw["authored"]["nodes"][0].clone()]
                };
                let mut response = serde_json::json!({
                    "data": {"viewer":null, "authored":{"nodes":nodes,"issueCount":3}}, "errors":[{"path":["viewer"]}]
                });
                if empty {
                    response["data"].as_object_mut().unwrap().remove("viewer");
                }
                Mock::given(method("POST"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(response))
                    .mount(&server)
                    .await;
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("inventory.db");
                let conn = crate::store::open_db(&path).unwrap();
                let source = Source::default();
                let list = CachedList::Reviewing;
                save_owned_source_snapshot(
                    &conn,
                    &source,
                    list,
                    &baseline.prs,
                    &baseline.coverage,
                    baseline.viewer.as_deref(),
                )
                .unwrap();
                let polls = SourcePolls::default();
                let first = attempt(&polls, &source, list);
                polls.complete(
                    polls.publication(&first).await.unwrap(),
                    Ok(baseline.clone()),
                    |_| {},
                );
                // Deliberately use disk as the preparation baseline, as after a restart.
                let next = attempt(&polls, &source, list);
                let permit = polls.publication(&next).await.unwrap();
                let fetched = client.fetch_reviewing_snapshot().await.unwrap();
                let prepared = reconcile_github_snapshot(&conn, &source, list, fetched, None);
                assert_eq!(prepared.is_ok(), known);
                if let Ok(receipt) = &prepared {
                    save_owned_source_snapshot(
                        &conn,
                        &source,
                        list,
                        &receipt.prs,
                        &receipt.coverage,
                        receipt.viewer.as_deref(),
                    )
                    .unwrap();
                }
                let mut emitted = None;
                polls.complete(permit, prepared, |status| {
                    let update = polls.update(status, None);
                    let rows = update.prs.unwrap();
                    assert_eq!(rows.len(), baseline.prs.len());
                    if !empty {
                        assert!(rows.iter().any(|r| r
                            .observation
                            .as_ref()
                            .is_some_and(|o| o.confirmed_review.is_some())));
                    }
                    assert_eq!(update.status.error.is_some(), !known);
                    emitted = Some(serde_json::to_value(rows).unwrap());
                });
                drop(conn);
                let restarted = crate::store::open_db(&path).unwrap();
                assert_eq!(
                    snapshot_owner(&restarted, &source, list)
                        .unwrap()
                        .as_deref(),
                    Some("fixture")
                );
                let SnapshotData::Available { prs, .. } =
                    load_source_snapshot(&restarted, &source, list)
                        .unwrap()
                        .data
                else {
                    panic!("lost owned snapshot")
                };
                assert_eq!(serde_json::to_value(&prs).unwrap(), emitted.unwrap());
                assert_eq!(prs.len(), baseline.prs.len());
                if !empty {
                    assert!(prs.iter().any(|r| r
                        .observation
                        .as_ref()
                        .is_some_and(|o| o.confirmed_review.is_some())));
                }
            }
        }
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
                session: None,
                scan: None,
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
    async fn confirmed_effect_retires_old_success_without_blocking_the_next_attempt() {
        for source in [
            Source::default(),
            Source {
                provider: Provider::Gitlab,
                host: "gitlab.com".into(),
            },
        ] {
            let polls = SourcePolls::default();
            let (old, _) = polls
                .begin_attempt(source.clone(), CachedList::Reviewing)
                .await;
            {
                let _guard = polls
                    .gate(&source, CachedList::Reviewing)
                    .lock_owned()
                    .await;
                polls.effect_generation(&source, CachedList::Reviewing);
            }
            assert!(polls.success_publication(&old).await.is_none());
            assert!(
                !polls.readback_is_current(&source, CachedList::Reviewing, 0),
                "a pre-review detail receipt cannot clear the newer effect"
            );
            let (new, _) = polls.begin_attempt(source, CachedList::Reviewing).await;
            assert!(polls.success_publication(&new).await.is_some());
        }
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
                    session: None,
                    scan: None,
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
            session: None,
            scan: None,
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
            session: None,
            scan: None,
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
    #[test]
    fn partition_limit_is_partial_with_a_reason_not_a_provider_failure() {
        use crate::queue_scan::{BlockReason, GithubPartition, PartitionPhase, State, Window};
        let polls = SourcePolls::default();
        let source = Source::default();
        let a = attempt(&polls, &source, CachedList::Reviewing);
        let mut state = State {
            received: true,
            done: true,
            github_partition: Some(GithubPartition {
                phase: PartitionPhase::FinalCheck,
                lower: Some(0),
                upper: Some(1),
                windows_started: 1,
                blocked: vec![(Window { lo: 0, hi: 1 }, BlockReason::TimestampResolution)],
                ..Default::default()
            }),
            ..Default::default()
        };
        let status = polls
            .finish_scan(&a, &state, Coverage::Partial { total: Some(1001) })
            .unwrap();
        assert_eq!(status.phase, Phase::Partial);
        assert_eq!(status.consecutive_failures, 0);
        assert!(status
            .error
            .as_deref()
            .is_some_and(|s| s.contains("timestamp")));
        state.no_work = true;
        let restarted = SourcePolls::default();
        let a = attempt(&restarted, &source, CachedList::Reviewing);
        let status = restarted
            .finish_scan(&a, &state, Coverage::Partial { total: Some(1001) })
            .unwrap();
        assert_eq!(status.phase, Phase::Partial);
        assert!(status.error.is_some());
        assert!(status.last_received_at.is_none());
    }
    #[test]
    fn accepted_scan_persists_exact_inventory_without_downgrading_earlier_pages() {
        use crate::{
            inventory::ObservationState,
            queue_scan::{Commit, State},
            store::source_cache::{load_source_snapshot, SnapshotData},
        };
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source::default();
        let list = CachedList::Reviewing;
        let scan = |revision, after: &str| {
            Some(Commit {
                expected_revision: revision,
                state: State {
                    after: Some(after.into()),
                    ..State::default()
                },
                removals: vec![],
            })
        };
        let mut first = receipt(1);
        first.viewer = Some("fixture".into());
        first.prs[0].head_oid = "head-1".into();
        first.coverage = Coverage::Partial { total: Some(2) };
        first.scan = scan(0, "tail-25");
        let first = reconcile_github_snapshot(&conn, &source, list, first, None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        let stamp = first.prs[0].observation.as_ref().unwrap().last_observed_at;
        let mut second = receipt(2);
        second.viewer = Some("fixture".into());
        second.prs[0].head_oid = "head-2".into();
        second.coverage = Coverage::Partial { total: Some(2) };
        second.scan = scan(1, "tail-50");
        let stale = second.clone();
        let accepted = reconcile_github_snapshot(&conn, &source, list, second, None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        let old = accepted
            .prs
            .iter()
            .find(|r| r.number == 1)
            .unwrap()
            .observation
            .as_ref()
            .unwrap();
        assert_eq!(old.state, ObservationState::Observed);
        assert_eq!(old.last_observed_at, stamp);
        assert_eq!(
            accepted
                .prs
                .iter()
                .find(|r| r.number == 2)
                .unwrap()
                .observation
                .as_ref()
                .unwrap()
                .state,
            ObservationState::Observed
        );
        let SnapshotData::Available { prs, .. } =
            load_source_snapshot(&conn, &source, list).unwrap().data
        else {
            panic!("missing persisted rows")
        };
        assert_eq!(prs, accepted.prs);
        assert!(reconcile_github_snapshot(&conn, &source, list, stale, None).is_err());
        let checkpoint = crate::queue_scan::load(&conn, &source, list, "fixture").unwrap();
        assert_eq!(checkpoint.revision, 2);
        assert_eq!(checkpoint.state.after.as_deref(), Some("tail-50"));
        // A confirmed action retires the pending publication but keeps traversal progress.
        crate::queue_scan::taint(&conn, &source, list, "fixture").unwrap();
        let mut before_action = accepted.clone();
        before_action.scan = scan(2, "tail-75");
        assert!(reconcile_github_snapshot(&conn, &source, list, before_action, None).is_err());
        let after_action = crate::queue_scan::load(&conn, &source, list, "fixture").unwrap();
        assert_eq!(after_action.state.after.as_deref(), Some("tail-50"));
        assert!(after_action.state.tainted);
    }
    #[test]
    fn duplicate_step_reuses_saved_rows_without_advancing_measurement_and_new_steps_retire_it() {
        use crate::queue_scan::{Commit, State};
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source::default();
        let list = CachedList::Authored;
        let mut fresh = receipt(7);
        fresh.viewer = Some("fixture".into());
        fresh.prs[0].head_oid = "head".into();
        fresh.scan = Some(Commit {
            expected_revision: 0,
            state: State {
                receipt_id: Some(crate::queue_scan::new_receipt_id()),
                ..State::default()
            },
            removals: vec![],
        });
        let accepted = reconcile_github_snapshot(&conn, &source, list, fresh.clone(), None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        let saved = crate::store::source_cache::load_source_snapshot(&conn, &source, list).unwrap();
        let duplicate = reconcile_github_snapshot(&conn, &source, list, fresh.clone(), None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        assert_eq!(duplicate.prs, accepted.prs);
        assert_eq!(
            crate::queue_scan::load(&conn, &source, list, "fixture")
                .unwrap()
                .revision,
            1
        );
        assert_eq!(
            crate::store::source_cache::load_source_snapshot(&conn, &source, list).unwrap(),
            saved
        );
        let mut next = fresh.clone();
        let scan = next.scan.as_mut().unwrap();
        scan.expected_revision = 1;
        scan.state.receipt_id = Some(crate::queue_scan::new_receipt_id());
        reconcile_github_snapshot(&conn, &source, list, next.clone(), None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        assert!(reconcile_github_snapshot(&conn, &source, list, fresh, None).is_err());
        crate::queue_scan::taint(&conn, &source, list, "fixture").unwrap();
        assert!(reconcile_github_snapshot(&conn, &source, list, next.clone(), None).is_err());
        assert!(!crate::queue_scan::accepted(
            &conn,
            &source,
            list,
            "other",
            next.scan.as_ref().unwrap()
        )
        .unwrap());
        let mut wrong_query = next.scan.unwrap();
        wrong_query.expected_revision = 2;
        wrong_query.state.version += 1;
        assert!(
            !crate::queue_scan::accepted(&conn, &source, list, "fixture", &wrong_query).unwrap()
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn gitlab_owner_rotation_while_publication_waits_refuses_persist_and_emit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(
            &program,
            r#"#!/bin/sh
id=1; test ! -f "$0.other" || id=2
printf 'HTTP/2 200\n\n{"id":%s,"username":"fixture"}' "$id"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let source = Source {
            provider: Provider::Gitlab,
            host: "publication-fixture.example".into(),
        };
        let list = CachedList::Reviewing;
        let old = crate::gitlab::queues::verified_test_session(&program, &source.host).await;
        let receipt = crate::gitlab::queues::FetchedList {
            session: Some(old),
            scan: None,
            viewer: Some("fixture".into()),
            mrs: vec![gitlab_mr(&source)],
            total: Some(1),
            coverage: Coverage::Complete,
        };
        let polls = SourcePolls::default();
        let (attempt, _) = polls.begin_attempt(source.clone(), list).await;
        let held = polls.gate(&source, list).lock_owned().await;
        let waiting = polls.success_publication(&attempt);
        tokio::pin!(waiting);
        tokio::select! { biased; _ = &mut waiting => panic!("publication bypassed held gate"), _ = tokio::task::yield_now() => {} }
        std::fs::write(program.with_extension("other"), "").unwrap();
        crate::gitlab::queues::verified_test_session(&program, &source.host).await;
        drop(held);
        let publication = waiting.await.unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        assert!(reconcile_gitlab_snapshot(&conn, &source, list, receipt.clone(), None).is_err());
        assert_eq!(
            crate::store::source_cache::snapshot_owner(&conn, &source, list).unwrap(),
            None
        );
        let mut emitted = false;
        polls.complete_gitlab(publication, Ok(receipt), |_| emitted = true);
        assert!(!emitted);
        assert!(!polls.4.lock().unwrap().contains_key(&(source, list)));
    }

    #[test]
    fn gitlab_progress_and_enrichment_commit_separately_and_duplicates_reuse_exact_rows() {
        use crate::queue_scan::{Commit, State};
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source {
            provider: crate::identity::Provider::Gitlab,
            host: "gitlab.com".into(),
        };
        let list = CachedList::Reviewing;
        let mut mr = gitlab_mr(&source);
        mr.viewer = Some("fixture".into());
        mr.head_oid = Some("head".into());
        let progress = crate::gitlab::queues::FetchedList {
            session: None,
            viewer: Some("fixture".into()),
            mrs: vec![mr],
            total: Some(2),
            coverage: Coverage::Partial { total: Some(2) },
            scan: Some(Commit {
                expected_revision: 0,
                state: State {
                    receipt_id: Some(crate::queue_scan::new_receipt_id()),
                    after: Some("2".into()),
                    tainted: true,
                    ..State::default()
                },
                removals: vec![],
            }),
        };
        let core = reconcile_gitlab_snapshot(&conn, &source, list, progress.clone(), None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        let duplicate = reconcile_gitlab_snapshot(&conn, &source, list, progress.clone(), None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        assert_eq!(core, duplicate);
        let mut enriched = progress;
        enriched.scan.as_mut().unwrap().expected_revision = 1;
        enriched.mrs[0].ci = Some(crate::github::model::CiState::Success);
        let final_rows =
            reconcile_gitlab_snapshot(&conn, &source, list, enriched.clone(), Some(core))
                .unwrap_or_else(|f| panic!("{}", f.message));
        assert_eq!(
            final_rows.mrs[0].ci,
            Some(crate::github::model::CiState::Success)
        );
        assert_eq!(
            crate::queue_scan::load(&conn, &source, list, "fixture")
                .unwrap()
                .revision,
            2
        );
        let duplicate = reconcile_gitlab_snapshot(&conn, &source, list, enriched.clone(), None)
            .unwrap_or_else(|f| panic!("{}", f.message));
        assert_eq!(duplicate, final_rows);
        // Even already-tainted progress cannot mistake an action's revision for
        // an accepted enrichment phase: actions clear the receipt identity.
        crate::queue_scan::taint(&conn, &source, list, "fixture").unwrap();
        assert!(crate::queue_scan::load(&conn, &source, list, "fixture")
            .unwrap()
            .state
            .receipt_id
            .is_none());
        assert!(reconcile_gitlab_snapshot(&conn, &source, list, enriched, None).is_err());
    }
    #[test]
    fn review_readback_persists_own_fact_and_taints_accepted_scan_without_losing_tail() {
        use crate::queue_scan::{Commit, State};
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        let source = Source::default();
        let list = CachedList::Reviewing;
        let mut incoming = receipt(1);
        incoming.viewer = Some("fixture".into());
        incoming.prs[0].head_oid = "head".into();
        let scan = Commit {
            expected_revision: 0,
            state: State {
                after: Some("synthetic-tail".into()),
                receipt_id: Some(crate::queue_scan::new_receipt_id()),
                ..State::default()
            },
            removals: vec![],
        };
        incoming.scan = Some(scan.clone());
        let mut accepted = reconcile_github_snapshot(&conn, &source, list, incoming, None)
            .unwrap_or_else(|e| panic!("{}", e.message));
        let now = chrono::Utc::now();
        let row = &mut accepted.prs[0];
        let written = crate::github::mutate::SubmittedReview {
            review_id: "review".into(),
            state: "APPROVED".into(),
            actor: "fixture".into(),
            commit_oid: "head".into(),
            submitted_at: Some(now),
            pr_id: row.id.clone(),
            repo: row.repo.clone(),
            number: row.number,
        };
        crate::inventory::apply_confirmed_review(
            row,
            &crate::inventory::ConfirmedReview {
                head_oid: "head".into(),
                review: crate::github::model::ReviewState::Approved,
                confirmed_at: now,
                receipt: Some(written),
                unresolved: false,
                confirmed_by_read: false,
            },
        );
        persist_github_effect(&conn, &source, list, &accepted).unwrap();
        let checkpoint = crate::queue_scan::load(&conn, &source, list, "fixture").unwrap();
        assert_eq!(checkpoint.state.after.as_deref(), Some("synthetic-tail"));
        assert!(checkpoint.state.receipt_id.is_none());
        assert!(!crate::queue_scan::accepted(&conn, &source, list, "fixture", &scan).unwrap());
        let crate::store::source_cache::SnapshotData::Available { prs, .. } =
            crate::store::source_cache::load_source_snapshot(&conn, &source, list)
                .unwrap()
                .data
        else {
            panic!("snapshot");
        };
        let effect = prs[0]
            .observation
            .as_ref()
            .unwrap()
            .confirmed_review
            .as_ref()
            .unwrap();
        assert_eq!(effect.receipt.as_ref().unwrap().review_id, "review");
        assert_eq!(
            crate::store::source_cache::snapshot_owner(&conn, &source, list)
                .unwrap()
                .as_deref(),
            Some("fixture")
        );
    }
}

#[cfg(test)]
#[path = "source_poll_integration_tests.rs"]
mod integration_tests;

#[cfg(test)]
#[path = "source_poll_freshness_tests.rs"]
mod freshness_tests;
