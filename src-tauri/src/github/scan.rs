//! Finite queue steps. Rich rows are observations from this step only.
use super::{
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
type Receipt = Option<Result<FetchedList, Arc<ClientError>>>;
#[derive(Default)]
pub(super) struct Reads(Mutex<HashMap<(bool, i64), Weak<ScanSlot>>>);
type ScanSlot = tokio::sync::Mutex<Receipt>;

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
        let _active = self.active_queue_read();
        let slot = {
            let mut reads = self.scans.0.lock().unwrap_or_else(|e| e.into_inner());
            reads.retain(|_, weak| weak.strong_count() > 0);
            let key = (list == CachedList::Reviewing, loaded.revision);
            reads.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
                let slot = Arc::new(tokio::sync::Mutex::new(None));
                reads.insert(key, Arc::downgrade(&slot));
                slot
            })
        };
        let mut receipt = tokio::time::timeout_at(self.read_context().deadline, slot.lock())
            .await
            .map_err(|_| ClientError::Timeout(30))?;
        if let Some(result) = receipt.as_ref() {
            return result.clone().map_err(ClientError::shared);
        }
        let result = self
            .advance_scan_inner(list, loaded, previous, now)
            .await
            .map_err(Arc::new);
        *receipt = Some(result.clone());
        result.map_err(ClientError::shared)
    }
    async fn advance_scan_inner(
        &self,
        list: CachedList,
        loaded: Loaded,
        previous: &[PullRequest],
        now: i64,
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
        if state.done && now >= state.eligible_at {
            state.fresh_pass();
        }
        state.receipt_id = Some(queue_scan::new_receipt_id());
        let due_confirmation = state.candidates.iter().any(|c| c.eligible_at <= now);
        if due_confirmation && client.attempts_remaining() > 0 {
            removals = client
                .with_attempt_limit(1)
                .confirm(list, &mut state, owner, now)
                .await;
        }
        let mut full_single_page = false;
        if !state.done && now >= state.eligible_at {
            for _ in 0..2 {
                if client.attempts_remaining() == 0 {
                    break;
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
                        break;
                    }
                };
                if raw["viewer"]["login"].as_str().is_some_and(|v| v != owner) {
                    return Err(ClientError::Graphql(
                        "queue account changed during traversal".into(),
                    ));
                }
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
                }
                for row in rows {
                    let id = row.identity();
                    state.candidates.retain(|c| c.identity != id);
                    if !state.seen.contains(&id) && state.seen.len() < 1000 {
                        state.seen.push(id);
                    }
                    merge_observation(&mut prs, row);
                }
                let next = page["pageInfo"]["hasNextPage"].as_bool();
                let cursor = page["pageInfo"]["endCursor"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 4096);
                if next == Some(false) {
                    state.pages += 1;
                    state.failures = 0;
                    state.done = true;
                    state.ceiling = total.is_some_and(|n| n > 1000);
                    full_single_page = state.pages == 1
                        && valid
                        && !state.tainted
                        && total == Some(state.seen.len() as u64)
                        && !state.ceiling;
                    state.eligible_at = now + queue_scan::CONFIRM_DELAY;
                    seed_candidates(&mut state, previous, now);
                    break;
                }
                if state.seen.len() >= 1000 || state.pages >= 39 {
                    state.done = true;
                    state.ceiling = true;
                    state.tainted = true;
                    state.eligible_at = now + queue_scan::CONFIRM_DELAY;
                    seed_candidates(&mut state, previous, now);
                    break;
                }
                if next != Some(true)
                    || cursor.is_none()
                    || cursor == state.after.as_deref()
                    || cursor.is_some_and(|c| state.cursors.iter().any(|old| old == c))
                {
                    state.failure(now);
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
        if !due_confirmation
            && now >= state.eligible_at
            && !state.done
            && state.after.is_some()
            && client.attempts_remaining() > 0
        {
            if let Ok(raw) = client.with_attempt_limit(1).scan_page(list, None).await {
                if raw["viewer"]["login"].as_str().is_some_and(|v| v != owner) {
                    return Err(ClientError::Graphql(
                        "queue account changed during head refresh".into(),
                    ));
                }
                state.observe_count(raw["authored"]["issueCount"].as_u64());
                for row in map_list(&raw, "authored") {
                    state.candidates.retain(|c| c.identity != row.identity());
                    merge_observation(&mut prs, row);
                }
            }
        }
        // A positive measurement in this step always beats a tentative negative.
        removals.retain(|id| !prs.iter().any(|r| &r.identity() == id));
        let total = state.total;
        let coverage = if full_single_page {
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
        self.graphql_partial_ok(&json!({"query":super::query::PRS_QUERY,"variables":{"q":predicate(list),"first":25,"after":after}})).await
    }
    async fn confirm(
        &self,
        list: CachedList,
        state: &mut State,
        owner: &str,
        now: i64,
    ) -> Vec<PrIdentity> {
        let mut batch = vec![];
        let cap = if state.isolate { 1 } else { 4 };
        for _ in 0..state.candidates.len() {
            let c = state.candidates.pop_front().unwrap();
            if c.eligible_at <= now && batch.len() < cap {
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
            state.isolate = true;
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
                    c.negative_at.get_or_insert(now);
                    c.failures = 0;
                    c.eligible_at = now + queue_scan::CONFIRM_DELAY;
                    state.candidates.push_back(c);
                }
                Verdict::Unknown => {
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
fn explicit_invalid_cursor(error: &ClientError) -> bool {
    match error {
        ClientError::Graphql(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("invalid cursor")
                || message.contains("cursor is invalid")
                || message.contains("not a valid cursor")
        }
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
fn seed_candidates(state: &mut State, previous: &[PullRequest], now: i64) {
    for row in previous {
        if state.candidates.len() >= queue_scan::CANDIDATES {
            break;
        }
        if !state.seen.contains(&row.identity())
            && !state
                .candidates
                .iter()
                .any(|c| c.identity == row.identity())
            && !row.id.is_empty()
        {
            state.candidates.push_back(Candidate {
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
    }
    fn candidate(number: usize) -> Candidate {
        let raw = node(number);
        Candidate {
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
        Mock::given(method("POST")).respond_with(|request:&wiremock::Request| {
            let body:Value=request.body_json().unwrap();let start=body["variables"]["after"].as_str().and_then(|s|s.strip_prefix("cursor-")).and_then(|s|s.parse::<usize>().ok()).unwrap_or(0);let end=(start+25).min(1000);
            ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":1001,"nodes":(start..end).map(node).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":true,"endCursor":format!("cursor-{end}")}}}}))
        }).mount(&server).await;
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
        }
        assert!(state.done && state.ceiling);
        assert_eq!(state.seen.len(), 1000);
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
}
