//! Finite queue steps. Rich rows are observations from this step only.
use super::{
    admission::{FirstAttempt, LiveRead, ReadClass, ReadContext},
    client::{refused_fields, ClientError, FetchedList, GitHubClient},
    map::map_list,
    model::PullRequest,
};
use crate::{
    identity::{PrIdentity, Source},
    queue_scan::{self, Candidate, Commit, Loaded, State},
    store::{source_cache::Coverage, CachedList},
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
};
#[cfg(test)]
mod demand_tests;
mod partition;

/// Intent is independent of the shared (list, revision) key. A finished
/// continuation must never authorize a fresh discovery pass.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanMode {
    Refresh,
    Continue,
}

type Receipt = Option<Result<FetchedList, Arc<ClientError>>>;
#[derive(Default)]
pub(super) struct Reads(Mutex<HashMap<(bool, i64), Weak<ScanSlot>>>);
struct ScanSlot {
    #[cfg(feature = "enterprise-harness")]
    metric: crate::enterprise_harness::metrics::Scope,
    state: Mutex<SlotState>,
    live: tokio::sync::watch::Sender<Option<LiveRead>>,
    completed: tokio::sync::Notify,
}
#[derive(Default)]
struct SlotState {
    callers: HashMap<u64, (ReadContext, ScanMode)>,
    next_id: u64,
    task: Option<tokio::task::AbortHandle>,
    receipt: Receipt,
    abandoned: bool,
    dispatch_deadline: Option<tokio::time::Instant>,
}
impl ScanSlot {
    fn new() -> Self {
        Self {
            #[cfg(feature = "enterprise-harness")]
            metric: crate::enterprise_harness::metrics::Scope::new("scan-slot", 0),
            state: Mutex::new(SlotState::default()),
            live: tokio::sync::watch::channel(None).0,
            completed: tokio::sync::Notify::new(),
        }
    }
    fn publish_demand(&self, state: &SlotState) {
        let callers: Vec<_> = state
            .callers
            .values()
            .filter(|(c, _)| c.deadline > tokio::time::Instant::now())
            .collect();
        let demand = callers
            .iter()
            .map(|(c, _)| c.deadline)
            .max()
            .map(|deadline| LiveRead {
                deadline: state
                    .dispatch_deadline
                    .map_or(deadline, |cap| deadline.min(cap)),
                class: if callers
                    .iter()
                    .any(|(c, _)| c.class == ReadClass::Foreground)
                {
                    ReadClass::Foreground
                } else if callers
                    .iter()
                    .any(|(c, _)| c.class == ReadClass::Background)
                {
                    ReadClass::Background
                } else {
                    ReadClass::Advisory
                },
            });
        self.live.send_replace(demand);
    }
}
struct ScanCaller {
    slot: Arc<ScanSlot>,
    id: u64,
}
impl Drop for ScanCaller {
    fn drop(&mut self) {
        let mut state = self.slot.state.lock().unwrap_or_else(|e| e.into_inner());
        state.callers.remove(&self.id);
        self.slot.publish_demand(&state);
        if state.callers.is_empty() {
            state.abandoned = true;
            if let Some(task) = state.task.take() {
                task.abort();
            }
        }
    }
}

fn predicate(list: CachedList) -> &'static str {
    match list {
        CachedList::Authored => "is:pr is:open author:@me",
        CachedList::Reviewing => "is:pr is:open review-requested:@me",
    }
}
fn identity(node: &Value) -> Option<PrIdentity> {
    Some(PrIdentity {
        source: Source::default(),
        repo: node["repository"]["nameWithOwner"].as_str()?.into(),
        number: node["number"].as_u64()?,
    })
}
fn clean(value: &Value) -> bool {
    refused_fields(value) == 0 && value["__partial_errors"].as_u64().unwrap_or(0) == 0
}

impl GitHubClient {
    pub(crate) async fn advance_scan(
        &self,
        list: CachedList,
        loaded: Loaded,
        previous: &[PullRequest],
        now: i64,
    ) -> Result<FetchedList, ClientError> {
        self.advance_scan_mode(list, loaded, previous, now, ScanMode::Refresh)
            .await
    }
    pub(crate) async fn advance_scan_mode(
        &self,
        list: CachedList,
        loaded: Loaded,
        previous: &[PullRequest],
        now: i64,
        mode: ScanMode,
    ) -> Result<FetchedList, ClientError> {
        let _active = self.active_queue_read();
        let context = self.read_context();
        let (slot, caller) = {
            let mut reads = self.scans.0.lock().unwrap_or_else(|e| e.into_inner());
            reads.retain(|_, weak| weak.strong_count() > 0);
            let key = (list == CachedList::Reviewing, loaded.revision);
            loop {
                let slot = reads.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
                    let slot = Arc::new(ScanSlot::new());
                    reads.insert(key, Arc::downgrade(&slot));
                    slot
                });
                let mut state = slot.state.lock().unwrap_or_else(|e| e.into_inner());
                // Last-consumer retirement may race registry selection. No
                // registration or receipt can revive an abandoned producer.
                if state.abandoned {
                    reads.remove(&key);
                    continue;
                }
                let id = state.next_id;
                state.next_id += 1;
                #[cfg(feature = "enterprise-harness")]
                crate::enterprise_harness::metrics::record(
                    slot.metric.id(),
                    "caller",
                    "scan-slot",
                    state.callers.len() as u64,
                );
                state.callers.insert(id, (context.clone(), mode));
                slot.publish_demand(&state);
                if state.task.is_none() && state.receipt.is_none() {
                    let shared = slot.clone();
                    let previous = previous.to_vec();
                    // A slot has one finite allowance. Queue lifetime follows live
                    // consumers; its execution cap starts at first dispatch.
                    let mut operation = context.clone();
                    operation.live = Some(slot.live.subscribe());
                    let dispatched = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let mark = dispatched.clone();
                    let observers = Arc::downgrade(&slot);
                    operation.first_attempt = Some(FirstAttempt::new(move || {
                        mark.store(true, std::sync::atomic::Ordering::Release);
                        if let Some(slot) = observers.upgrade() {
                            let callbacks: Vec<_> = {
                                let mut state =
                                    slot.state.lock().unwrap_or_else(|e| e.into_inner());
                                state.dispatch_deadline = Some(
                                    tokio::time::Instant::now()
                                        + std::time::Duration::from_secs(30),
                                );
                                slot.publish_demand(&state);
                                state
                                    .callers
                                    .values()
                                    .filter_map(|(c, _)| c.first_attempt.clone())
                                    .collect()
                            };
                            for callback in callbacks {
                                callback.observe();
                            }
                        }
                    }));
                    let client = self.with_scan_context(operation).with_attempt_limit(3);
                    let task = tokio::spawn(async move {
                        #[cfg(feature = "enterprise-harness")]
                        let mut metric =
                            crate::enterprise_harness::metrics::Scope::new("scan-producer", 0);
                        #[cfg(feature = "enterprise-harness")]
                        metric.mark("slot", shared.metric.id());
                        let mut changes = shared.live.subscribe();
                        let run = async {
                            loop {
                                changes.borrow_and_update();
                                let mode = {
                                    let state =
                                        shared.state.lock().unwrap_or_else(|e| e.into_inner());
                                    if state.callers.values().any(|(c, m)| {
                                        c.deadline > tokio::time::Instant::now()
                                            && *m == ScanMode::Refresh
                                    }) {
                                        ScanMode::Refresh
                                    } else {
                                        ScanMode::Continue
                                    }
                                };
                                let step = client.advance_scan_inner(
                                    list,
                                    Loaded {
                                        revision: loaded.revision,
                                        state: loaded.state.clone(),
                                    },
                                    &previous,
                                    now,
                                    mode,
                                );
                                tokio::pin!(step);
                                loop {
                                    if dispatched.load(std::sync::atomic::Ordering::Acquire) {
                                        // Every admission, full response body and
                                        // retry wait observes the execution cap.
                                        // Let those errors return through the scan
                                        // so received rows/checkpoints survive;
                                        // an outer timeout would drop that progress.
                                        return step.await;
                                    }
                                    tokio::select! {
                                        biased;
                                        _ = changes.changed(), if !dispatched.load(std::sync::atomic::Ordering::Acquire) => {
                                            // Only a proven zero-dispatch plan can
                                            // be replaced. Its shared allowance is
                                            // retained across all demand changes.
                                            if !dispatched.load(std::sync::atomic::Ordering::Acquire) { break; }
                                        }
                                        result = &mut step => return result,
                                    }
                                }
                            }
                        };
                        let result = run.await.map_err(Arc::new);
                        #[cfg(feature = "enterprise-harness")]
                        metric.finish(if result.is_ok() { "receipt" } else { "failed" });
                        shared
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .receipt = Some(result);
                        shared.completed.notify_waiters();
                    });
                    state.task = Some(task.abort_handle());
                }
                drop(state);
                break (slot.clone(), ScanCaller { slot, id });
            }
        };
        let wait = async {
            loop {
                let notified = slot.completed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some(result) = slot
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .receipt
                    .clone()
                {
                    return result.map_err(ClientError::shared);
                }
                notified.await;
            }
        };
        let result = tokio::time::timeout_at(context.deadline, wait)
            .await
            .map_err(|_| ClientError::Timeout(30))?;
        drop(caller);
        result
    }
    async fn advance_scan_inner(
        &self,
        list: CachedList,
        loaded: Loaded,
        previous: &[PullRequest],
        now: i64,
        mode: ScanMode,
    ) -> Result<FetchedList, ClientError> {
        let client = self.with_attempt_limit(3);
        let mut state = loaded.state;
        let mut prs = vec![];
        let mut removals = vec![];
        let mut viewer = self
            .known_viewer()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        // The caller normally verified ownership before reading checkpoint data.
        if viewer.is_none() {
            viewer = Some(client.fetch_viewer().await?);
        }
        let owner = viewer
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ClientError::Graphql("no verified queue owner".into()))?;
        let unchanged = state.clone();
        let attempts_before = client.attempts_remaining();
        if state.done {
            if let Some(finished_at) = state.finished_at {
                state.eligible_at = finished_at + state.pass_delay.max(queue_scan::CONFIRM_DELAY);
            }
        }
        if mode == ScanMode::Refresh && state.done && now >= state.eligible_at {
            state.fresh_pass();
        }
        let began_at_head = state.after.is_none();
        state.no_work = true;
        state.received = false;
        let previous_failure = state.step_failure.take();
        state.receipt_id = Some(queue_scan::new_receipt_id());
        let due_confirmation = state.candidates.iter().any(|c| c.eligible_at <= now);
        if due_confirmation && client.attempts_remaining() > 0 {
            state.no_work = false;
            removals = client
                .with_attempt_limit(1)
                .confirm(list, &mut state, owner, now)
                .await;
        }

        if !state.done && now >= state.eligible_at {
            state.started_at.get_or_insert(now);
            for _ in 0..2 {
                if client.attempts_remaining() == 0 {
                    break;
                }
                state.no_work = false;
                if state.github_partition.is_some() {
                    let (q, first, after) = partition::request(&mut state, list);
                    match client.scan_query(&q, first, after.as_deref()).await {
                        Ok(raw) => {
                            if raw["viewer"]["login"].as_str().is_some_and(|v| v != owner) {
                                return Err(ClientError::Graphql(
                                    "queue account changed during traversal".into(),
                                ));
                            }
                            partition::accept(&mut state, &raw, &mut prs, now);
                            if state.done {
                                seed_candidates(
                                    &mut state,
                                    previous,
                                    now,
                                    &removals.iter().cloned().collect(),
                                );
                                break;
                            }
                            if state.failures > 0 {
                                break;
                            }
                        }
                        Err(error) => {
                            if after.is_some() && explicit_invalid_cursor(&error) {
                                partition::reset_leaf(&mut state);
                            }
                            state.failure(now);
                            state.step_failure = Some(scan_failure(&error));
                            break;
                        }
                    }
                    continue;
                }
                let raw = match client.scan_page(list, state.after.as_deref()).await {
                    Ok(raw) => raw,
                    Err(error) => {
                        if state.after.is_some() && explicit_invalid_cursor(&error) {
                            let receipt_id = state.receipt_id.take();
                            state.fresh_pass();
                            state.receipt_id = receipt_id;
                        }
                        state.failure(now);
                        state.step_failure = Some(scan_failure(&error));
                        break;
                    }
                };
                if raw["viewer"]["login"].as_str().is_some_and(|v| v != owner) {
                    return Err(ClientError::Graphql(
                        "queue account changed during traversal".into(),
                    ));
                }
                if state.after.is_some() && raw["__queue_cursor_invalid"] == true {
                    let receipt_id = state.receipt_id.take();
                    state.fresh_pass();
                    state.receipt_id = receipt_id;
                    state.failure(now);
                    state.step_failure = Some(queue_scan::ScanFailure {
                        message: "The queue cursor could not be validated; traversal will retry."
                            .into(),
                        transient: true,
                        not_asked: false,
                    });
                    break;
                }
                state.received = true;
                let rows = map_list(&raw, "authored");
                let page = &raw["authored"];
                let total = page["issueCount"].as_u64();
                state.observe_count(total);
                let valid = clean(&raw)
                    && page["nodes"]
                        .as_array()
                        .is_some_and(|n| n.len() == rows.len())
                    && rows.iter().all(|r| !r.id.is_empty())
                    && rows
                        .iter()
                        .map(|r| r.identity())
                        .collect::<HashSet<_>>()
                        .len()
                        == rows.len();
                if !valid {
                    state.tainted = true;
                    state.coverage_valid = false;
                }
                for row in rows {
                    let id = row.identity();
                    state.candidates.retain(|c| c.identity != id);
                    if !state.seen.contains(&id) && state.seen.len() < 1000 {
                        state.seen.push(id);
                    }
                    merge_observation(&mut prs, row);
                }
                if total.is_some_and(|n| n > 1000) {
                    state.github_partition = Some(queue_scan::GithubPartition::default());
                    state.after = None;
                    state.cursors.clear();
                    state.pages = 0;
                    state.coverage_valid = false;
                    for candidate in &mut state.candidates {
                        candidate.negative_at = None;
                    }
                    continue;
                }
                let next = page["pageInfo"]["hasNextPage"].as_bool();
                let cursor = page["pageInfo"]["endCursor"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 4096);
                if next == Some(false) {
                    state.pages += 1;
                    state.failures = 0;
                    state.done = true;
                    state.finished_at = Some(now);
                    state.ceiling = total.is_some_and(|n| n > 1000);
                    let complete = valid
                        && !state.tainted
                        && total == Some(state.seen.len() as u64)
                        && !state.ceiling;
                    state.coverage_valid = complete;
                    if complete {
                        state.completed_at = Some(now);
                        state.completed_total = total;
                    }
                    state.eligible_at = now + state.pass_delay.max(queue_scan::CONFIRM_DELAY);
                    seed_candidates(
                        &mut state,
                        previous,
                        now,
                        &removals.iter().cloned().collect(),
                    );
                    break;
                }
                if state.seen.len() >= 1000 || state.pages >= 39 {
                    state.done = true;
                    state.finished_at = Some(now);
                    state.ceiling = true;
                    state.tainted = true;
                    state.eligible_at = now + state.pass_delay.max(queue_scan::CONFIRM_DELAY);
                    seed_candidates(
                        &mut state,
                        previous,
                        now,
                        &removals.iter().cloned().collect(),
                    );
                    break;
                }
                if next != Some(true)
                    || cursor.is_none()
                    || cursor == state.after.as_deref()
                    || cursor.is_some_and(|c| state.cursors.iter().any(|old| old == c))
                {
                    state.failure(now);
                    state.step_failure = Some(queue_scan::ScanFailure {
                        message: "The queue cursor could not be validated; traversal will retry."
                            .into(),
                        transient: true,
                        not_asked: false,
                    });
                    break;
                }
                state.pages += 1;
                state.failures = 0;
                let cursor = cursor.unwrap().to_string();
                state.cursors.push(cursor.clone());
                state.after = Some(cursor);
            }
        }
        // Optional head freshness never changes the pending tail or pass membership.
        if state.github_partition.is_none()
            && !due_confirmation
            && now >= state.eligible_at
            && !state.done
            && !began_at_head
            && state.after.is_some()
            && client.attempts_remaining() > 0
        {
            match client.with_attempt_limit(1).scan_page(list, None).await {
                Ok(raw) => {
                    if raw["viewer"]["login"].as_str().is_some_and(|v| v != owner) {
                        return Err(ClientError::Graphql(
                            "queue account changed during head refresh".into(),
                        ));
                    }
                    state.observe_count(raw["authored"]["issueCount"].as_u64());
                    if !clean(&raw) {
                        state.tainted = true;
                    }
                    for row in map_list(&raw, "authored") {
                        state.candidates.retain(|c| c.identity != row.identity());
                        merge_observation(&mut prs, row);
                    }
                }
                Err(error) => {
                    state.failure(now);
                    state.step_failure = Some(scan_failure(&error));
                }
            }
        }
        // Admission refusal is not a provider observation or a failed response.
        // Preserve the accepted receipt exactly when no HTTP attempt was admitted.
        if client.attempts_remaining() == attempts_before {
            state = unchanged;
            state.no_work = true;
            state.received = false;
        }

        if !state.no_work && !state.done && state.failures == 0 {
            state.eligible_at = now + 15;
        }
        // A positive measurement in this step always beats a tentative negative.
        removals.retain(|id| !prs.iter().any(|r| &r.identity() == id));
        state
            .candidates
            .retain(|c| !prs.iter().any(|r| r.identity() == c.identity));
        let total = state.total;
        if state.no_work && state.step_failure.is_none() {
            state.step_failure = previous_failure;
        }
        if state.tainted {
            state.coverage_valid = false;
        }
        let coverage = if state.coverage_valid {
            Coverage::Complete
        } else {
            Coverage::Partial { total }
        };
        Ok(FetchedList {
            viewer,
            prs,
            total,
            coverage,
            scan: Some(Commit {
                expected_revision: loaded.revision,
                state,
                removals,
            }),
        })
    }
    async fn scan_page(&self, list: CachedList, after: Option<&str>) -> Result<Value, ClientError> {
        self.scan_query(predicate(list), 25, after).await
    }
    async fn scan_query(
        &self,
        q: &str,
        first: u64,
        after: Option<&str>,
    ) -> Result<Value, ClientError> {
        self.graphql_partial_ok(&json!({"query":super::query::PRS_QUERY,"variables":{"q":q,"first":first,"after":after}})).await
    }
    async fn confirm(
        &self,
        list: CachedList,
        state: &mut State,
        owner: &str,
        now: i64,
    ) -> Vec<PrIdentity> {
        let mut batch = vec![];
        // The first due entry owns the turn. An isolated entry gets a singleton;
        // otherwise healthy candidates may batch without re-poisoning known failures.
        let isolated = state
            .candidates
            .iter()
            .find(|c| c.eligible_at <= now)
            .is_some_and(|c| c.isolated);
        let cap = if isolated { 1 } else { 4 };
        for _ in 0..state.candidates.len() {
            let c = state.candidates.pop_front().unwrap();
            if c.eligible_at <= now && batch.len() < cap && c.isolated == isolated {
                batch.push(c);
            } else {
                state.candidates.push_back(c);
            }
        }
        if batch.is_empty() {
            return vec![];
        }
        let mut vars = serde_json::Map::new();
        for (i, c) in batch.iter().enumerate() {
            let (org, repo) = c.identity.repo.split_once('/').unwrap_or(("", ""));
            vars.insert(format!("o{i}"), json!(org));
            vars.insert(format!("r{i}"), json!(repo));
            vars.insert(format!("n{i}"), json!(c.identity.number));
            vars.insert(
                format!("q{i}"),
                json!(format!(
                    "{} repo:{} created:{}",
                    predicate(list),
                    c.identity.repo,
                    c.created_at
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                )),
            );
        }
        let raw = self.graphql_partial_ok(&json!({"query":super::query::membership_confirmation_query(batch.len()),"variables":vars})).await;
        let healthy = raw
            .as_ref()
            .is_ok_and(|v| clean(v) && v["viewer"]["login"].as_str() == Some(owner));
        if !healthy {
            state.tainted = true;
            state.step_failure = Some(match &raw {
                Err(error) => scan_failure(error),
                Ok(_) => queue_scan::ScanFailure {
                    message: "Queue membership could not be confirmed; saved rows were retained."
                        .into(),
                    transient: true,
                    not_asked: false,
                },
            });
        }
        let mut removed = vec![];
        for (i, mut c) in batch.into_iter().enumerate() {
            let verdict = if healthy {
                confirmation(
                    &raw.as_ref().unwrap()[format!("p{i}")]["pullRequest"],
                    &raw.as_ref().unwrap()[format!("m{i}")],
                    &mut c,
                )
            } else {
                Verdict::Unknown
            };
            match verdict {
                Verdict::Terminal => removed.push(c.identity),
                Verdict::Present => {}
                Verdict::Absent
                    if c.negative_at
                        .is_some_and(|t| now >= t + queue_scan::CONFIRM_DELAY) =>
                {
                    removed.push(c.identity)
                }
                Verdict::Absent => {
                    c.isolated = false;
                    c.negative_at.get_or_insert(now);
                    c.failures = 0;
                    c.eligible_at = now + queue_scan::CONFIRM_DELAY;
                    state.candidates.push_back(c);
                }
                Verdict::Unknown => {
                    c.isolated = true;
                    c.negative_at = None;
                    c.failures = c.failures.saturating_add(1);
                    c.eligible_at = now + queue_scan::backoff(c.failures);
                    state.candidates.push_back(c);
                }
            }
        }
        removed
    }
}
fn scan_failure(error: &ClientError) -> queue_scan::ScanFailure {
    queue_scan::ScanFailure {
        message: error.to_string(),
        transient: error.is_transient(),
        not_asked: matches!(error, ClientError::NotDispatched(_)),
    }
}

fn explicit_invalid_cursor(error: &ClientError) -> bool {
    match error {
        ClientError::Graphql(message) => super::client::invalid_cursor_message(message),
        _ => false,
    }
}
fn merge_observation(rows: &mut Vec<PullRequest>, row: PullRequest) {
    if let Some(old) = rows.iter_mut().find(|old| old.identity() == row.identity()) {
        *old = row;
    } else {
        rows.push(row);
    }
}
fn seed_candidates(
    state: &mut State,
    previous: &[PullRequest],
    now: i64,
    excluded: &HashSet<PrIdentity>,
) {
    if previous.is_empty() {
        return;
    }
    let start = state.candidate_position % previous.len();
    for offset in 0..previous.len() {
        let index = (start + offset) % previous.len();
        let row = &previous[index];
        state.candidate_position = (index + 1) % previous.len();
        if excluded.contains(&row.identity())
            || state.seen.contains(&row.identity())
            || row.id.is_empty()
            || state
                .candidates
                .iter()
                .any(|c| c.identity == row.identity())
        {
            continue;
        }
        if state.candidates.len() >= queue_scan::CANDIDATES {
            // Only recycle candidates already given an inconclusive opportunity.
            // Untested and first-negative entries retain their turn/proof spacing.
            let Some(evict) = state
                .candidates
                .iter()
                .position(|c| c.failures > 0 && c.negative_at.is_none() && c.eligible_at <= now)
            else {
                state.candidate_position = index;
                break;
            };
            state.candidates.remove(evict);
        }
        state.candidates.push_back(Candidate {
            isolated: false,
            identity: row.identity(),
            id: row.id.clone(),
            head: row.head_oid.clone(),
            created_at: row.created_at,
            negative_at: None,
            eligible_at: now,
            failures: 0,
        });
    }
}

enum Verdict {
    Present,
    Absent,
    Terminal,
    Unknown,
}
fn confirmation(direct: &Value, bucket: &Value, candidate: &mut Candidate) -> Verdict {
    if direct["id"].as_str() != Some(candidate.id.as_str())
        || identity(direct).as_ref() != Some(&candidate.identity)
    {
        return Verdict::Unknown;
    }
    let Some(created) = direct["createdAt"].as_str().and_then(|s| s.parse().ok()) else {
        return Verdict::Unknown;
    };
    let Some(head) = direct["headRefOid"].as_str().filter(|s| !s.is_empty()) else {
        return Verdict::Unknown;
    };
    if created != candidate.created_at || head != candidate.head {
        candidate.created_at = created;
        candidate.head = head.into();
        return Verdict::Unknown;
    }
    if matches!(direct["state"].as_str(), Some("CLOSED" | "MERGED")) {
        return Verdict::Terminal;
    }
    if direct["state"].as_str() != Some("OPEN") {
        return Verdict::Unknown;
    }
    let Some(nodes) = bucket["nodes"].as_array() else {
        return Verdict::Unknown;
    };
    if nodes.len() > 25
        || bucket["issueCount"].as_u64() != Some(nodes.len() as u64)
        || bucket["pageInfo"]["hasNextPage"].as_bool() != Some(false)
    {
        return Verdict::Unknown;
    }
    let mut ids = HashSet::new();
    for node in nodes {
        let Some(id) = node["id"].as_str().filter(|s| !s.is_empty()) else {
            return Verdict::Unknown;
        };
        if identity(node).is_none() || !ids.insert(id) {
            return Verdict::Unknown;
        }
        if id == candidate.id {
            return if identity(node).as_ref() == Some(&candidate.identity) {
                Verdict::Present
            } else {
                Verdict::Unknown
            };
        }
    }
    Verdict::Absent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue_scan::State;
    use serde_json::{json, Value};
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    #[tokio::test]
    async fn queued_scan_promotes_live_foreground_joiner_with_one_receipt() {
        use crate::github::admission::{ReadClass, ReadContext};
        use std::time::Duration;
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}}}))).mount(&server).await;
        let base = client(&server);
        base.fetch_viewer().await.unwrap();
        Mock::given(wiremock::matchers::body_partial_json(
            json!({"query":"blocker"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(5))
                .set_body_json(json!({"data":{}})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
        let mut blockers = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let client = base.with_read_context(ReadContext::new(
                ReadClass::Background,
                Duration::from_secs(5),
            ));
            blockers.spawn(async move { client.stats_graphql(&json!({"query":"blocker"})).await });
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while server.received_requests().await.unwrap().len() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let background = base.with_read_context(ReadContext::new(
            ReadClass::Background,
            Duration::from_secs(2),
        ));
        let worker = tokio::spawn(async move {
            background
                .advance_scan_mode(
                    CachedList::Authored,
                    Loaded {
                        revision: 0,
                        state: State::default(),
                    },
                    &[],
                    1000,
                    ScanMode::Continue,
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let result = tokio::time::timeout(
            Duration::from_millis(300),
            base.advance_scan(
                CachedList::Authored,
                Loaded {
                    revision: 0,
                    state: State::default(),
                },
                &[],
                1000,
            ),
        )
        .await
        .expect("foreground joiner must use reserved capacity")
        .unwrap();
        assert_eq!(result.scan, worker.await.unwrap().unwrap().scan);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            4,
            "one identity, two blockers and exactly one shared page"
        );
        blockers.abort_all();
        while blockers.join_next().await.is_some() {}
    }
    fn client(server: &MockServer) -> GitHubClient {
        GitHubClient::new(
            octocrab::Octocrab::builder()
                .base_uri(server.uri())
                .unwrap()
                .personal_token("synthetic".to_string())
                .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
                .build()
                .unwrap(),
        )
    }
    fn node(number: usize) -> Value {
        let mut fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/search.json")).unwrap();
        let mut node = fixture["authored"]["nodes"][0].take();
        node["id"] = json!(format!("PR-{number}"));
        node["number"] = json!(number);
        node["headRefOid"] = json!(format!("head-{number}"));
        node
    }
    // External search enforces GitHub's per-query ceiling, including on ranged queries.
    fn search_response(body: &Value, members: &[Value]) -> ResponseTemplate {
        let q = body["variables"]["q"].as_str().unwrap_or("");
        let mut selected = members.iter().collect::<Vec<_>>();
        if let Some(range) = q
            .split_whitespace()
            .find_map(|s| s.strip_prefix("created:"))
        {
            let (lo, hi) = range.split_once("..").expect("documented inclusive range");
            let lo = lo.parse::<chrono::DateTime<chrono::Utc>>().unwrap();
            let hi = hi.parse::<chrono::DateTime<chrono::Utc>>().unwrap();
            selected.retain(|n| {
                let t = n["createdAt"]
                    .as_str()
                    .unwrap()
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .unwrap();
                t >= lo && t <= hi
            });
        }
        selected.sort_by_key(|n| n["createdAt"].as_str().unwrap());
        if q.contains("sort:created-desc") {
            selected.reverse();
        }
        let total = selected.len();
        let first = body["variables"]["first"].as_u64().unwrap_or(25) as usize;
        let start = body["variables"]["after"]
            .as_str()
            .and_then(|s| s.strip_prefix("cursor-"))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        let end = (start + first).min(total.min(1000));
        let nodes = selected
            .into_iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .cloned()
            .collect::<Vec<_>>();
        ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{
            "issueCount":total,"nodes":nodes,"pageInfo":{"hasNextPage":end<total.min(1000),"endCursor":format!("cursor-{end}")}
        }}}))
    }
    fn dated_members(count: usize) -> Vec<Value> {
        (1..=count)
            .map(|i| {
                let mut n = node(i);
                n["createdAt"] = json!(chrono::DateTime::from_timestamp(
                    1_600_000_000 + i as i64 * 60,
                    500_000_000
                )
                .unwrap()
                .to_rfc3339());
                n
            })
            .collect()
    }
    fn persist_step(
        path: &std::path::Path,
        result: FetchedList,
        inventory: &mut Vec<PullRequest>,
    ) -> State {
        let commit = result.scan.unwrap();
        inventory.retain(|r| !commit.removals.contains(&r.identity()));
        *inventory = crate::inventory::reconcile(
            std::mem::take(inventory),
            result.prs,
            false,
            chrono::Utc::now(),
        );
        let conn = crate::store::open_db(path).unwrap();
        assert!(queue_scan::commit(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "fixture",
            &commit,
            |tx| {
                crate::store::source_cache::save_owned_source_snapshot(
                    tx,
                    &Source::default(),
                    CachedList::Reviewing,
                    inventory,
                    &result.coverage,
                    Some("fixture"),
                )
            }
        )
        .unwrap());
        assert!(queue_scan::accepted(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
            "fixture",
            &commit
        )
        .unwrap());
        drop(conn);
        let conn = crate::store::open_db(path).unwrap();
        let saved = crate::store::source_cache::load_source_snapshot(
            &conn,
            &Source::default(),
            CachedList::Reviewing,
        )
        .unwrap();
        let crate::store::source_cache::SnapshotData::Available { prs, .. } = saved.data else {
            panic!("saved inventory must survive restart")
        };
        *inventory = prs;
        commit.state
    }
    async fn oversized_inventory(count: usize) {
        let server = MockServer::start().await;
        let mut members = dated_members(count);
        if count == 1025 {
            // Exact endpoints/midpoint, plus a fractional row just beyond midpoint.
            for i in [1, 513, 1025] {
                members[i - 1]["createdAt"] = json!(chrono::DateTime::from_timestamp(
                    1_600_000_000 + i as i64 * 60,
                    0
                )
                .unwrap()
                .to_rfc3339());
            }
            members[513]["createdAt"] = json!(chrono::DateTime::from_timestamp(
                1_600_000_000 + 513 * 60,
                500_000_000
            )
            .unwrap()
            .to_rfc3339());
        }
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                search_response(&request.body_json().unwrap(), &members)
            })
            .mount(&server)
            .await;
        let client = client(&server);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partition.sqlite");
        let mut inventory = vec![];
        let mut state = State::default();
        let mut phases = HashSet::new();
        for step in 0..150 {
            let conn = crate::store::open_db(&path).unwrap();
            let loaded =
                queue_scan::load(&conn, &Source::default(), CachedList::Reviewing, "fixture")
                    .unwrap();
            drop(conn);
            let before = server.received_requests().await.unwrap().len();
            let step_client = client.with_attempt_limit(if count == 1025 { 1 } else { 3 });
            // Verify ownership separately so a single admitted discovery can checkpoint each phase.
            if step == 0 {
                client.fetch_viewer().await.unwrap();
            }
            let result = step_client
                .advance_scan(CachedList::Reviewing, loaded, &inventory, 1000 + step * 60)
                .await
                .unwrap();
            assert!(server.received_requests().await.unwrap().len() - before <= 3);
            state = persist_step(&path, result, &mut inventory);
            if let Some(p) = &state.github_partition {
                phases.insert(format!("{:?}", p.phase));
            }
            if state.done {
                break;
            }
        }
        assert_eq!(
            inventory.len(),
            count,
            "all provider identities, beyond the first 1,000"
        );
        assert_eq!(
            inventory.iter().map(|r| r.number).collect::<HashSet<_>>(),
            (1..=count as u64).collect()
        );
        assert!(state.done && state.coverage_valid);
        if count == 1025 {
            for phase in ["LowerBound", "UpperBound", "Leaves", "FinalCheck"] {
                assert!(phases.contains(phase), "restart at {phase}");
            }
        }
        assert_eq!(state.completed_total, Some(count as u64));
    }
    #[tokio::test]
    async fn oversized_1025_members_converge_across_sqlite_restart() {
        oversized_inventory(1025).await;
    }
    #[tokio::test]
    async fn oversized_2750_members_converge_across_sqlite_restart() {
        oversized_inventory(2750).await;
    }
    #[tokio::test]
    async fn overflow_terminal_gets_a_fair_proof_without_removing_unknown_rows() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let mut data = json!({"viewer":{"login":"fixture"},"authored":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}});
            for i in 0..4 {
                if body["variables"][format!("n{i}")].as_u64() == Some(1001) {
                    let mut direct = node(1001); direct["state"] = json!("CLOSED");
                    data[format!("p{i}")] = json!({"pullRequest":direct});
                }
            }
            ResponseTemplate::new(200).set_body_json(json!({"data":data}))
        }).mount(&server).await;
        let client = client(&server);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fair.sqlite");
        let mut inventory = map_list(
            &json!({"authored":{"nodes":(1..=1001).map(node).collect::<Vec<_>>()}}),
            "authored",
        );
        for step in 0..260 {
            let conn = crate::store::open_db(&path).unwrap();
            let loaded =
                queue_scan::load(&conn, &Source::default(), CachedList::Reviewing, "fixture")
                    .unwrap();
            drop(conn);
            let before = server.received_requests().await.unwrap().len();
            let result = client
                .advance_scan(CachedList::Reviewing, loaded, &inventory, 1000 + step * 60)
                .await
                .unwrap();
            assert!(server.received_requests().await.unwrap().len() - before <= 3);
            persist_step(&path, result, &mut inventory);
            assert!(inventory
                .iter()
                .filter(|r| r.number <= 1000)
                .all(|r| r.head_oid == format!("head-{}", r.number)));
            if inventory.len() == 1000 {
                break;
            }
        }
        assert_eq!(
            inventory.len(),
            1000,
            "overflow terminal must eventually be confirmed"
        );
        assert!(inventory.iter().all(|r| r.number <= 1000));
    }
    #[tokio::test]
    async fn partition_second_request_failure_preserves_success_and_recovers_leaf_budget() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let server = MockServer::start().await;
        let failing = Arc::new(AtomicBool::new(true));
        let fail = failing.clone();
        let members = dated_members(75);
        Mock::given(method("POST"))
            .respond_with(move |r: &wiremock::Request| {
                let body: Value = r.body_json().unwrap();
                if body["variables"]["after"] == "cursor-25" && fail.load(Ordering::SeqCst) {
                    return ResponseTemplate::new(503);
                }
                search_response(&body, &members)
            })
            .mount(&server)
            .await;
        let client = client(&server);
        client.fetch_viewer().await.unwrap();
        let window = queue_scan::Window {
            lo: 1_600_000_000,
            hi: 1_600_010_000,
        };
        let state = State {
            total: Some(75),
            count_seen: true,
            github_partition: Some(queue_scan::GithubPartition {
                phase: queue_scan::PartitionPhase::Leaves,
                lower: Some(window.lo),
                upper: Some(window.hi),
                active: Some(queue_scan::Leaf::new(window)),
                windows_started: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = client
            .advance_scan(
                CachedList::Reviewing,
                Loaded { revision: 0, state },
                &[],
                1000,
            )
            .await
            .unwrap();
        assert_eq!(result.prs.len(), 25);
        let state = result.scan.unwrap().state;
        assert_eq!(
            state
                .github_partition
                .as_ref()
                .unwrap()
                .active
                .as_ref()
                .unwrap()
                .after
                .as_deref(),
            Some("cursor-25")
        );
        assert!(state.step_failure.is_some() && state.tainted);
        failing.store(false, Ordering::SeqCst);
        let before = server.received_requests().await.unwrap().len();
        let recovered = client
            .advance_scan(
                CachedList::Reviewing,
                Loaded { revision: 1, state },
                &result.prs,
                1200,
            )
            .await
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len() - before, 2);
        assert_eq!(
            recovered.prs.len(),
            50,
            "both successful discovery slots resume after backoff"
        );
        assert_eq!(
            recovered
                .scan
                .unwrap()
                .state
                .github_partition
                .unwrap()
                .phase,
            queue_scan::PartitionPhase::FinalCheck
        );
    }
    #[tokio::test]
    async fn partition_expired_cursor_restarts_only_its_leaf_and_keeps_siblings() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":null},"errors":[{"path":["authored"],"message":"The cursor is invalid"}]}))).mount(&server).await;
        let client = client(&server);
        client.fetch_viewer().await.unwrap();
        let window = queue_scan::Window { lo: 0, hi: 100 };
        let sibling = queue_scan::Window { lo: 100, hi: 200 };
        let mut leaf = queue_scan::Leaf::new(window);
        leaf.after = Some("expired".into());
        leaf.cursors.push("expired".into());
        leaf.pages = 1;
        let state = State {
            seen: vec![candidate(1).identity],
            github_partition: Some(queue_scan::GithubPartition {
                phase: queue_scan::PartitionPhase::Leaves,
                lower: Some(0),
                upper: Some(200),
                active: Some(leaf),
                pending: [sibling].into(),
                windows_started: 2,
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = client
            .advance_scan(
                CachedList::Reviewing,
                Loaded { revision: 1, state },
                &[],
                1000,
            )
            .await
            .unwrap();
        let state = result.scan.unwrap().state;
        let p = state.github_partition.unwrap();
        assert_eq!(p.active.unwrap(), queue_scan::Leaf::new(window));
        assert_eq!(p.pending, [sibling]);
        assert_eq!(state.seen.len(), 1);
        assert!(state.tainted && state.eligible_at > 1000 && !state.coverage_valid);
    }
    #[tokio::test]
    async fn recurring_partition_discovers_old_pr_requested_after_its_leaf_was_visited() {
        let server = MockServer::start().await;
        let members = Arc::new(Mutex::new(dated_members(1025)));
        let mock_members = members.clone();
        Mock::given(method("POST"))
            .respond_with(move |r: &wiremock::Request| {
                search_response(&r.body_json().unwrap(), &mock_members.lock().unwrap())
            })
            .mount(&server)
            .await;
        let client = client(&server);
        client.fetch_viewer().await.unwrap();
        let mut state = State::default();
        let mut inventory = vec![];
        let mut introduced = false;
        let mut passes = 0;
        for step in 0..150 {
            let result = client
                .with_attempt_limit(1)
                .advance_scan(
                    CachedList::Reviewing,
                    Loaded {
                        revision: step,
                        state,
                    },
                    &inventory,
                    1000 + step * 60,
                )
                .await
                .unwrap();
            state =
                serde_json::from_value(serde_json::to_value(result.scan.unwrap().state).unwrap())
                    .unwrap();
            inventory =
                crate::inventory::reconcile(inventory, result.prs, false, chrono::Utc::now());
            if !introduced
                && state.github_partition.as_ref().is_some_and(|p| {
                    p.active.is_none() && p.pending.front().is_some_and(|w| Some(w.lo) > p.lower)
                })
            {
                let mut old = node(2000);
                old["createdAt"] = json!(chrono::DateTime::from_timestamp(1_600_000_080, 0)
                    .unwrap()
                    .to_rfc3339());
                members.lock().unwrap().push(old);
                introduced = true;
            }
            if state.done {
                passes += 1;
                if passes == 1 {
                    assert!(
                        !state.coverage_valid,
                        "final count detects changed membership"
                    );
                }
                if passes == 2 {
                    break;
                }
            }
        }
        assert!(introduced && state.done && state.coverage_valid);
        assert_eq!(passes, 2);
        assert_eq!(inventory.len(), 1026);
        assert!(inventory.iter().any(|r| r.number == 2000));
    }
    #[tokio::test]
    async fn dense_leaf_remains_partial_while_healthy_siblings_are_discovered() {
        let server = MockServer::start().await;
        let mut members = (1..=1001).map(node).collect::<Vec<_>>();
        for n in &mut members {
            n["createdAt"] = json!("2020-01-01T00:00:00Z");
        }
        let mut sibling = node(1002);
        sibling["createdAt"] = json!("2020-01-01T00:00:08Z");
        members.push(sibling);
        Mock::given(method("POST"))
            .respond_with(move |r: &wiremock::Request| {
                search_response(&r.body_json().unwrap(), &members)
            })
            .mount(&server)
            .await;
        let client = client(&server);
        let mut state = State::default();
        let mut inventory = vec![];
        for step in 0..30 {
            let result = client
                .advance_scan(
                    CachedList::Reviewing,
                    Loaded {
                        revision: step,
                        state,
                    },
                    &inventory,
                    1000 + step * 60,
                )
                .await
                .unwrap();
            assert!(matches!(result.coverage, Coverage::Partial { .. }));
            state =
                serde_json::from_value(serde_json::to_value(result.scan.unwrap().state).unwrap())
                    .unwrap();
            inventory =
                crate::inventory::reconcile(inventory, result.prs, false, chrono::Utc::now());
            if state.done {
                break;
            }
        }
        assert!(state.done && state.ceiling);
        assert!(inventory.iter().any(|r| r.number == 1002));
        assert!(state
            .github_partition
            .unwrap()
            .blocked
            .iter()
            .any(|(_, reason)| *reason == queue_scan::BlockReason::TimestampResolution));
    }
    #[test]
    fn candidate_rotation_keeps_untested_and_spaced_proofs_but_restarts_discarded_evidence() {
        let mut state = State {
            candidates: (1..=1000).map(candidate).collect(),
            ..Default::default()
        };
        let previous = map_list(
            &json!({"authored":{"nodes":(1..=1001).map(node).collect::<Vec<_>>()}}),
            "authored",
        );
        state.candidates[0].negative_at = Some(1000);
        state.candidates[1].failures = 1;
        seed_candidates(&mut state, &previous, 1010, &HashSet::new());
        assert_eq!(state.candidates.len(), 1000);
        assert!(state
            .candidates
            .iter()
            .any(|c| c.identity.number == 1 && c.negative_at == Some(1000)));
        assert!(state
            .candidates
            .iter()
            .any(|c| c.identity.number == 1001 && c.negative_at.is_none()));
        assert!(!state.candidates.iter().any(|c| c.identity.number == 2));
        state.candidates[2].failures = 1;
        state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        state.fresh_pass();
        seed_candidates(&mut state, &previous, 1100, &HashSet::new());
        assert!(state
            .candidates
            .iter()
            .any(|c| c.identity.number == 2 && c.negative_at.is_none()));
    }
    #[tokio::test]
    async fn reachable_275_rows_advance_with_three_actual_attempts_per_step() {
        let server = MockServer::start().await;
        let head_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = request.body_json().unwrap();
                let after = body["variables"]["after"].as_str().unwrap_or("");
                let start: usize = after
                    .strip_prefix("cursor-")
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0);
                let end = (start + 25).min(275);
                let mut nodes = (start..end).map(node).collect::<Vec<_>>();
                if start == 0 {
                    nodes[0]["headRefOid"] = json!(format!(
                        "moving-head-{}",
                        head_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    ));
                }
                ResponseTemplate::new(200).set_body_json(
                    json!({"data":{"viewer":{"login":"fixture"},"authored":{
                        "issueCount":275,"nodes":nodes,
                        "pageInfo":{"hasNextPage":end<275,"endCursor":format!("cursor-{end}")}
                    }}}),
                )
            })
            .mount(&server)
            .await;
        let client = client(&server);
        let mut state = State::default();
        let mut inventory = vec![];
        let mut head_versions = HashSet::new();
        for step in 0..6 {
            let before = server.received_requests().await.unwrap().len();
            let result = client
                .advance_scan(
                    CachedList::Authored,
                    Loaded {
                        revision: step,
                        state,
                    },
                    &inventory,
                    1000 + step * 60,
                )
                .await
                .unwrap();
            assert!(
                server.received_requests().await.unwrap().len() - before <= 3,
                "actual attempt cap"
            );
            if let Some(head) = result.prs.iter().find(|r| r.number == 0) {
                head_versions.insert(head.head_oid.clone());
            }
            state = result.scan.as_ref().unwrap().state.clone();
            inventory =
                crate::inventory::reconcile(inventory, result.prs, false, chrono::Utc::now());
        }
        assert!(
            head_versions.len() > 1,
            "early changes must not reset tail progress"
        );
        assert_eq!(inventory.len(), 275);
        assert!(state.done);
        assert!(state.github_partition.is_none());
        for request in server.received_requests().await.unwrap() {
            let body: Value = request.body_json().unwrap();
            if body["variables"]["q"].is_string() {
                assert_eq!(body["variables"]["first"], 25);
                assert_eq!(body["variables"]["q"], "is:pr is:open author:@me");
            }
        }
    }
    #[tokio::test]
    async fn confirmation_failure_is_qualified_without_authorizing_removal() {
        let server = MockServer::start().await;
        let client = client(&server);
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"data":{"viewer":{"login":"fixture"}}})),
            )
            .mount(&server)
            .await;
        client.fetch_viewer().await.unwrap();
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let result = client
            .advance_scan(
                CachedList::Reviewing,
                Loaded {
                    revision: 1,
                    state: State {
                        done: true,
                        eligible_at: i64::MAX,
                        candidates: [candidate(7)].into(),
                        ..Default::default()
                    },
                },
                &[],
                1000,
            )
            .await
            .unwrap();
        let scan = result.scan.unwrap();
        assert!(scan.removals.is_empty());
        assert!(scan.state.step_failure.is_some());
        assert_eq!(scan.state.candidates.len(), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    pub(super) fn candidate(number: usize) -> Candidate {
        let raw = node(number);
        Candidate {
            isolated: false,
            identity: identity(&raw).unwrap(),
            id: raw["id"].as_str().unwrap().into(),
            head: raw["headRefOid"].as_str().unwrap().into(),
            created_at: raw["createdAt"].as_str().unwrap().parse().unwrap(),
            negative_at: None,
            eligible_at: 0,
            failures: 0,
        }
    }
    #[tokio::test]
    async fn forty_absent_members_need_two_spaced_proofs_and_finish_in_twenty_bounded_steps() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = request.body_json().unwrap();
                let mut data = json!({"viewer":{"login":"fixture"}});
                for i in 0..4 {
                    if let Some(n) = body["variables"][format!("n{i}")].as_u64() {
                        assert!(body["variables"][format!("q{i}")]
                            .as_str()
                            .unwrap()
                            .starts_with("is:pr is:open review-requested:@me repo:"));
                        let mut direct = node(n as usize);
                        direct["state"] = json!("OPEN");
                        data[format!("p{i}")] = json!({"pullRequest":direct});
                        data[format!("m{i}")] =
                            json!({"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}});
                    }
                }
                ResponseTemplate::new(200).set_body_json(json!({"data":data}))
            })
            .mount(&server)
            .await;
        let client = client(&server);
        let mut state = State {
            done: true,
            eligible_at: i64::MAX,
            candidates: (1..=40).map(candidate).collect(),
            ..State::default()
        };
        let mut removed = vec![];
        for step in 0..20 {
            let before = server.received_requests().await.unwrap().len();
            let result = client
                .advance_scan(
                    CachedList::Reviewing,
                    Loaded {
                        revision: step,
                        state,
                    },
                    &[],
                    1000 + step * 60,
                )
                .await
                .unwrap();
            assert!(
                server.received_requests().await.unwrap().len() - before <= 2,
                "identity plus at most one proof"
            );
            let commit = result.scan.unwrap();
            state = commit.state;
            removed.extend(commit.removals);
            if step < 10 {
                assert!(removed.is_empty());
            }
            // Simulate process restart by crossing the persisted representation each step.
            state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        }
        assert_eq!(removed.len(), 40);
        assert!(state.candidates.is_empty());
    }
    #[test]
    fn exact_bucket_and_direct_identity_are_both_required_and_new_heads_reset_proof() {
        let mut c = candidate(7);
        let mut direct = node(7);
        direct["state"] = json!("OPEN");
        let mut bucket = json!({"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}});
        assert!(matches!(
            confirmation(&direct, &bucket, &mut c),
            Verdict::Absent
        ));
        bucket["issueCount"] = json!(26);
        assert!(matches!(
            confirmation(&direct, &bucket, &mut c),
            Verdict::Unknown
        ));
        bucket["issueCount"] = json!(0);
        bucket["pageInfo"]["hasNextPage"] = Value::Null;
        assert!(matches!(
            confirmation(&direct, &bucket, &mut c),
            Verdict::Unknown
        ));
        bucket["pageInfo"]["hasNextPage"] = json!(false);
        direct["headRefOid"] = json!("new-head");
        assert!(matches!(
            confirmation(&direct, &bucket, &mut c),
            Verdict::Unknown
        ));
        assert_eq!(c.head, "new-head");
        direct["createdAt"] = json!("2026-01-01T00:00:00Z");
        assert!(matches!(
            confirmation(&direct, &bucket, &mut c),
            Verdict::Unknown
        ));
        direct["id"] = json!("wrong");
        direct["state"] = json!("CLOSED");
        assert!(matches!(
            confirmation(&direct, &bucket, &mut c),
            Verdict::Unknown
        ));
        direct["id"] = json!(c.id);
        assert!(matches!(
            confirmation(&direct, &bucket, &mut c),
            Verdict::Terminal
        ));
    }
    #[tokio::test]
    async fn overlapping_steps_share_one_receipt_but_later_refresh_does_not() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(20)).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}}}))).mount(&server).await;
        let client = client(&server);
        let (a, b) = tokio::join!(
            client.advance_scan(
                CachedList::Authored,
                Loaded {
                    revision: 0,
                    state: State::default()
                },
                &[],
                1000
            ),
            client.advance_scan(
                CachedList::Authored,
                Loaded {
                    revision: 0,
                    state: State::default()
                },
                &[],
                1000
            )
        );
        assert_eq!(a.unwrap().scan, b.unwrap().scan);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "one identity and one page"
        );
        client
            .advance_scan(
                CachedList::Authored,
                Loaded {
                    revision: 0,
                    state: State::default(),
                },
                &[],
                1000,
            )
            .await
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }
    #[tokio::test]
    async fn one_errored_alias_is_isolated_so_healthy_candidates_finish() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(|request:&wiremock::Request| {
            let body:Value=request.body_json().unwrap();let mut data=json!({"viewer":{"login":"fixture"}});let mut errors=vec![];
            for i in 0..4 {
                if let Some(n)=body["variables"][format!("n{i}")].as_u64() {
                    if n==1 { errors.push(json!({"message":"synthetic alias failure","path":[format!("p{i}")]})); }
                    else { let mut direct=node(n as usize);direct["state"]=json!("OPEN");data[format!("p{i}")]=json!({"pullRequest":direct});data[format!("m{i}")]=json!({"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}); }
                }
            }
            ResponseTemplate::new(200).set_body_json(json!({"data":data,"errors":errors}))
        }).mount(&server).await;
        let client = client(&server);
        let mut state = State {
            done: true,
            eligible_at: i64::MAX,
            candidates: (1..=4).map(candidate).collect(),
            ..State::default()
        };
        let mut removed = vec![];
        for step in 0..12 {
            let result = client
                .advance_scan(
                    CachedList::Reviewing,
                    Loaded {
                        revision: step,
                        state,
                    },
                    &[],
                    1000 + 60 * step,
                )
                .await
                .unwrap()
                .scan
                .unwrap();
            state = result.state;
            removed.extend(result.removals);
        }
        assert_eq!(removed.len(), 3);
        assert_eq!(state.candidates.len(), 1);
        assert_eq!(state.candidates[0].identity.number, 1);
    }
    #[tokio::test]
    async fn late_failure_preserves_tail_and_does_not_relabel_prior_pages_observed() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let server = MockServer::start().await;
        let fail = Arc::new(AtomicBool::new(true));
        let mock_fail = fail.clone();
        Mock::given(method("POST")).respond_with(move |request:&wiremock::Request| {
            let body:Value=request.body_json().unwrap();let after=body["variables"]["after"].as_str().unwrap_or("");
            if after=="cursor-50" && mock_fail.load(Ordering::SeqCst) {return ResponseTemplate::new(503);}
            let start=after.strip_prefix("cursor-").and_then(|n|n.parse::<usize>().ok()).unwrap_or(0);let end=(start+25).min(75);
            ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":75,"nodes":(start..end).map(node).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":end<75,"endCursor":format!("cursor-{end}")}}}}))
        }).mount(&server).await;
        let client = client(&server);
        client.fetch_viewer().await.unwrap();
        let first = client
            .advance_scan(
                CachedList::Authored,
                Loaded {
                    revision: 0,
                    state: State::default(),
                },
                &[],
                1000,
            )
            .await
            .unwrap();
        assert_eq!(
            first.scan.as_ref().unwrap().state.after.as_deref(),
            Some("cursor-50")
        );
        let before = server.received_requests().await.unwrap().len();
        let failed = client
            .advance_scan(
                CachedList::Authored,
                Loaded {
                    revision: 1,
                    state: first.scan.unwrap().state,
                },
                &first.prs,
                1060,
            )
            .await
            .unwrap();
        assert!(failed.prs.is_empty());
        assert!(server.received_requests().await.unwrap().len() - before <= 3);
        let state = failed.scan.unwrap().state;
        assert_eq!(state.after.as_deref(), Some("cursor-50"));
        assert!(state.tainted);
        fail.store(false, Ordering::SeqCst);
        let finished = client
            .advance_scan(
                CachedList::Authored,
                Loaded { revision: 2, state },
                &first.prs,
                1200,
            )
            .await
            .unwrap();
        assert_eq!(finished.prs.len(), 25);
        assert!(finished.prs.iter().all(|r| r.number >= 50));
        assert!(finished.scan.unwrap().state.done);
    }
    #[tokio::test]
    async fn search_ceiling_stays_partial_and_bad_cursor_never_claims_a_terminal_pass() {
        let server = MockServer::start().await;
        let members = (1..=1001).map(node).collect::<Vec<_>>();
        Mock::given(method("POST"))
            .respond_with(move |request: &wiremock::Request| {
                search_response(&request.body_json().unwrap(), &members)
            })
            .mount(&server)
            .await;
        let client = client(&server);
        let mut state = State::default();
        for step in 0..20 {
            let result = client
                .advance_scan(
                    CachedList::Authored,
                    Loaded {
                        revision: step,
                        state,
                    },
                    &[],
                    1000 + step * 60,
                )
                .await
                .unwrap();
            assert!(matches!(
                result.coverage,
                Coverage::Partial { total: Some(1001) }
            ));
            state = result.scan.unwrap().state;
            if state.done {
                break;
            }
        }
        assert!(state.done && state.ceiling);
        assert!(!state.coverage_valid);
        assert!(
            !state.seen.is_empty(),
            "retain useful rows from a dense window"
        );
        server.reset().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":50,"nodes":[node(26)],"pageInfo":{"hasNextPage":true,"endCursor":"cursor-25"}}}}))).mount(&server).await;
        let state = State {
            after: Some("cursor-25".into()),
            cursors: vec!["cursor-25".into()],
            pages: 1,
            ..State::default()
        };
        let result = client
            .advance_scan(
                CachedList::Authored,
                Loaded {
                    revision: 21,
                    state,
                },
                &[],
                3000,
            )
            .await
            .unwrap()
            .scan
            .unwrap();
        assert!(!result.state.done);
        assert_eq!(result.state.pages, 1);
        assert_eq!(result.state.after.as_deref(), Some("cursor-25"));
        assert!(result.state.eligible_at > 3000);
    }
    #[tokio::test]
    async fn explicit_expired_cursor_restarts_only_traversal_and_keeps_inventory_qualified() {
        let server = MockServer::start().await;
        let client = client(&server);
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"data":{"viewer":{"login":"fixture"}}})),
            )
            .mount(&server)
            .await;
        client.fetch_viewer().await.unwrap();
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"errors":[{"message":"The cursor is invalid"}]})),
            )
            .mount(&server)
            .await;
        let state = State {
            after: Some("expired".into()),
            pages: 9,
            ..State::default()
        };
        let result = client
            .advance_scan(
                CachedList::Authored,
                Loaded { revision: 4, state },
                &[],
                1000,
            )
            .await
            .unwrap();
        assert!(result.prs.is_empty());
        assert!(matches!(result.coverage, Coverage::Partial { .. }));
        let state = result.scan.unwrap().state;
        assert!(state.after.is_none());
        assert_eq!(state.pages, 0);
        assert!(state.tainted);
        assert!(state.receipt_id.is_some());
        assert!(state.eligible_at > 1000);
    }
    #[tokio::test]
    async fn round1_partial_invalid_cursor_retires_only_matching_search_and_recovers() {
        for (message, path, reset) in [
            ("The cursor is invalid", "authored", true),
            ("Temporary resolver failure", "authored", false),
            ("The cursor is invalid", "unrelated", false),
        ] {
            let server = MockServer::start().await;
            let client = client(&server);
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"data":{"viewer":{"login":"fixture"}}})),
                )
                .mount(&server)
                .await;
            client.fetch_viewer().await.unwrap();
            server.reset().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"rateLimit":{"cost":1},"authored":null},"errors":[{"path":[path],"message":message}]}))).mount(&server).await;
            let mut pending = candidate(7);
            pending.eligible_at = i64::MAX;
            let state = State {
                after: Some("expired".into()),
                pages: 9,
                candidates: [pending.clone()].into(),
                ..State::default()
            };
            let previous = map_list(&json!({"authored":{"nodes":[node(7)]}}), "authored");
            let result = client
                .advance_scan(
                    CachedList::Authored,
                    Loaded { revision: 4, state },
                    &previous,
                    1000,
                )
                .await
                .unwrap();
            assert!(server.received_requests().await.unwrap().len() <= 3);
            assert!(matches!(result.coverage, Coverage::Partial { .. }));
            let retained =
                crate::inventory::reconcile(previous, result.prs, false, chrono::Utc::now());
            assert_eq!(retained.len(), 1);
            assert_eq!(
                retained[0].observation.as_ref().unwrap().state,
                crate::inventory::ObservationState::Retained
            );
            let state = result.scan.unwrap().state;
            assert_eq!(state.after.is_none(), reset, "{message} at {path}");
            assert_eq!(state.candidates[0], pending);
            assert!(state.receipt_id.is_some());
            assert!(state.eligible_at > 1000);
            if reset {
                server.reset().await;
                Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":1,"nodes":[node(8)],"pageInfo":{"hasNextPage":false,"endCursor":"new"}}}}))).mount(&server).await;
                let recovered = client
                    .advance_scan(
                        CachedList::Authored,
                        Loaded { revision: 5, state },
                        &retained,
                        1100,
                    )
                    .await
                    .unwrap();
                let requests = server.received_requests().await.unwrap();
                assert!(requests.len() <= 3);
                assert!(requests[0].body_json::<Value>().unwrap()["variables"]["after"].is_null());
                assert_eq!(recovered.prs[0].number, 8);
            }
        }
    }
}
