//! Bounded current-head evidence for queue rows. Core MR reads are cached
//! independently of list receipts; a list refresh never implies a CI refresh.
use super::{detail, queues::MergeRequest};
use crate::github::model::{CiState, ReviewState};
use futures_util::{stream, StreamExt};
use std::{
    collections::HashMap,
    path::Path,
    sync::{LazyLock, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct Evidence {
    ci: Option<CiState>,
    review: Option<ReviewState>,
    unresolved: Option<u64>,
    unresolved_floor: bool,
    needs_my_review: Option<bool>,
    in_train: Option<bool>,
    can_enqueue: Option<bool>,
}
impl Evidence {
    fn apply(&self, row: &mut MergeRequest) {
        row.ci = self.ci;
        row.review = self.review;
        row.unresolved_threads = self.unresolved;
        row.unresolved_threads_floor = self.unresolved_floor;
        row.needs_my_review = self.needs_my_review;
        row.in_merge_queue = self.in_train;
        row.can_enqueue_train = self.can_enqueue;
    }
}
static CACHE: LazyLock<Mutex<HashMap<String, (tokio::time::Instant, Evidence)>>> =
    LazyLock::new(Mutex::default);
fn cache_insert(
    cache: &mut HashMap<String, (tokio::time::Instant, Evidence)>,
    key: String,
    evidence: Evidence,
) {
    if cache.len() >= 1000 && !cache.contains_key(&key) {
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(key, (tokio::time::Instant::now(), evidence));
}
const TTL: Duration = Duration::from_secs(60);
const MAX_READS: usize = 4;

fn evidence(raw: &serde_json::Value, head: Option<&str>) -> Evidence {
    if head.is_none() || raw["sha"].as_str() != head {
        return Evidence::default();
    }
    let ci = if raw["head_pipeline"]["sha"].as_str() == head {
        match raw["head_pipeline"]["status"].as_str() {
            Some("success") => Some(CiState::Success),
            Some("failed" | "canceled") => Some(CiState::Failure),
            Some(
                "created"
                | "waiting_for_resource"
                | "preparing"
                | "pending"
                | "running"
                | "scheduled"
                | "manual",
            ) => Some(CiState::Pending),
            _ => None,
        }
    } else {
        None
    };
    let review = match raw["detailed_merge_status"].as_str() {
        Some("not_approved") => Some(ReviewState::ReviewRequired),
        Some("requested_changes") => Some(ReviewState::ChangesRequested),
        _ => None,
    };
    Evidence {
        ci,
        review,
        ..Evidence::default()
    }
}

const QUERY: &str = "query($path: ID!, $iid: String!) { currentUser { username } project(fullPath: $path) { mergeRequest(iid: $iid) { iid webUrl diffHeadSha approved approvedBy(first: 1) { nodes { username } } changeRequesters(first: 1) { nodes { username } } headPipeline { sha status } discussions(first: 100) { nodes { resolvable resolved } pageInfo { hasNextPage } } reviewers(first: 100) { nodes { username mergeRequestInteraction { reviewState } } pageInfo { hasNextPage } } mergeTrainCar { index } availableAutoMergeStrategies userPermissions { canMerge } } } }";

// Schema capability failures are host-local. Removing only named optional
// fields preserves CI and discussion evidence on older self-managed versions.
fn reduced_query(query: &str, response: &serde_json::Value) -> Option<String> {
    let errors = response["errors"].as_array()?;
    if errors.is_empty() {
        return None;
    }
    let mut next = query.to_owned();
    for error in errors {
        if error["extensions"]["code"] != "undefinedField" {
            return None;
        }
        let field = error["extensions"]["fieldName"].as_str()?;
        let fragment = match field {
            "changeRequesters" => "changeRequesters(first: 1) { nodes { username } }",
            "mergeTrainCar" => "mergeTrainCar { index }",
            "availableAutoMergeStrategies" => "availableAutoMergeStrategies",
            "mergeRequestInteraction" => "mergeRequestInteraction { reviewState }",
            _ => return None,
        };
        next = next.replace(fragment, "");
    }
    (next != query).then_some(next)
}

static QUERIES: LazyLock<Mutex<HashMap<String, (tokio::time::Instant, String)>>> =
    LazyLock::new(Mutex::default);
#[cfg(all(test, unix))]
async fn measure(
    program: &Path,
    identity: &crate::identity::PrIdentity,
    head: Option<&str>,
    budget: Duration,
    attempts: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> Option<Evidence> {
    let started = tokio::time::Instant::now();
    let mut context = super::transport::Context::new(
        program,
        &identity.source.host,
        super::transport::Class::Advisory,
        started + budget,
    );
    context.allowance = Some(attempts.clone());
    measure_context(program, identity, head, &context).await
}
async fn measure_context(
    program: &Path,
    identity: &crate::identity::PrIdentity,
    head: Option<&str>,
    context: &super::transport::Context,
) -> Option<Evidence> {
    let started = tokio::time::Instant::now();
    let budget = context.deadline.saturating_duration_since(started);
    let host_key = format!(
        "{program:?}:{}:{}",
        identity.source.host, context.generation
    );
    let mut query = {
        let mut queries = QUERIES.lock().unwrap_or_else(|e| e.into_inner());
        queries.retain(|_, (at, _)| started.duration_since(*at) < Duration::from_secs(600));
        queries
            .get(&host_key)
            .map(|(_, query)| query.clone())
            .unwrap_or_else(|| QUERY.into())
    };
    for _ in 0..4 {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return None;
        }
        let input = serde_json::json!({"query":query,"variables":{"path":identity.repo,"iid":identity.number.to_string()}});
        match detail::request_json_context(
            program,
            &identity.source.host,
            "graphql",
            "POST",
            Some(input),
            context,
        )
        .await
        .map_err(|e| e.issue)
        {
            Ok(response) => {
                if let Some(measured) = graph_evidence(&response.body, identity, head) {
                    return Some(measured);
                }
                if let Some(reduced) = reduced_query(&query, &response.body) {
                    query = reduced;
                    let mut queries = QUERIES.lock().unwrap_or_else(|e| e.into_inner());
                    if queries.len() >= 100 {
                        if let Some(oldest) = queries
                            .iter()
                            .min_by_key(|(_, (at, _))| *at)
                            .map(|(key, _)| key.clone())
                        {
                            queries.remove(&oldest);
                        }
                    }
                    queries.insert(host_key.clone(), (started, query.clone()));
                    continue;
                }
                break;
            }
            Err(_) => return None,
        }
    }
    let remaining = budget.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return None;
    }
    // The legacy REST fallback is a real API process too; it shares the same
    // per-step allowance instead of silently exceeding the whole-step bound.
    let endpoint = format!(
        "projects/{}/merge_requests/{}",
        detail::encode_project(&identity.repo),
        identity.number
    );
    let response = detail::request_json_context(
        program,
        &identity.source.host,
        &endpoint,
        "GET",
        None,
        context,
    )
    .await
    .map_err(|e| e.issue);
    match response {
        Ok(response) if detail::map_core(&response.body, identity).is_some() => {
            Some(evidence(&response.body, head))
        }
        _ => None,
    }
}

fn graph_evidence(
    response: &serde_json::Value,
    identity: &crate::identity::PrIdentity,
    head: Option<&str>,
) -> Option<Evidence> {
    use serde_json::Value;
    if response
        .get("errors")
        .is_some_and(|errors| !errors.as_array().is_some_and(Vec::is_empty))
    {
        return None;
    }
    let mr = response.pointer("/data/project/mergeRequest")?;
    if head.is_none()
        || mr["diffHeadSha"].as_str() != head
        || mr["iid"].as_str() != Some(identity.number.to_string().as_str())
        || mr["webUrl"].as_str()
            != Some(
                format!(
                    "https://{}/{}/-/merge_requests/{}",
                    identity.source.host, identity.repo, identity.number
                )
                .as_str(),
            )
    {
        return None;
    }
    let mut result = evidence(
        &serde_json::json!({"sha":head,"head_pipeline":{"sha":mr["headPipeline"]["sha"],"status":mr["headPipeline"]["status"].as_str().map(str::to_ascii_lowercase)}}),
        head,
    );
    let changes = mr
        .pointer("/changeRequesters/nodes")
        .and_then(Value::as_array);
    let approvals = mr.pointer("/approvedBy/nodes").and_then(Value::as_array);
    result.review = if changes.is_some_and(|nodes| !nodes.is_empty()) {
        Some(ReviewState::ChangesRequested)
    } else if changes.is_some() {
        match (mr["approved"].as_bool(), approvals) {
            (Some(true), Some(nodes)) if !nodes.is_empty() => Some(ReviewState::Approved),
            (Some(true), Some(_)) => Some(ReviewState::None),
            (Some(false), _) => Some(ReviewState::ReviewRequired),
            _ => None,
        }
    } else {
        None
    };
    if let Some(nodes) = mr.pointer("/discussions/nodes").and_then(Value::as_array) {
        if nodes
            .iter()
            .all(|n| n["resolvable"].is_boolean() && n["resolved"].is_boolean())
        {
            let count = nodes
                .iter()
                .filter(|n| n["resolvable"] == true && n["resolved"] == false)
                .count() as u64;
            let complete = mr
                .pointer("/discussions/pageInfo/hasNextPage")
                .and_then(Value::as_bool)
                == Some(false);
            // No visible unresolved discussions on an incomplete page is not zero.
            if complete || count > 0 {
                result.unresolved = Some(count);
            }
            result.unresolved_floor = !complete;
        }
    }
    if let (Some(viewer), Some(nodes)) = (
        response
            .pointer("/data/currentUser/username")
            .and_then(Value::as_str),
        mr.pointer("/reviewers/nodes").and_then(Value::as_array),
    ) {
        if let Some(me) = nodes
            .iter()
            .find(|n| n["username"].as_str() == Some(viewer))
        {
            result.needs_my_review = match me
                .pointer("/mergeRequestInteraction/reviewState")
                .and_then(Value::as_str)
            {
                Some("UNREVIEWED" | "REVIEW_STARTED") => Some(true),
                Some("REVIEWED" | "APPROVED" | "REQUESTED_CHANGES") => Some(false),
                _ => None,
            };
        } else if mr
            .pointer("/reviewers/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            == Some(false)
        {
            result.needs_my_review = Some(false);
        }
    }
    result.in_train = mr.get("mergeTrainCar").and_then(|car| {
        if car.is_null() {
            Some(false)
        } else {
            car["index"].as_u64().map(|_| true)
        }
    });
    result.can_enqueue = mr["availableAutoMergeStrategies"]
        .as_array()
        .zip(mr["userPermissions"]["canMerge"].as_bool())
        .map(|(strategies, allowed)| {
            allowed
                && result.in_train == Some(false)
                && strategies.iter().any(|s| {
                    matches!(
                        s.as_str(),
                        Some("merge_train" | "add_to_merge_train_when_checks_pass")
                    )
                })
        });
    Some(result)
}

static REVISIONS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Mutex::default);
static NEXT_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn revision(identity: &crate::identity::PrIdentity, replace: bool) -> u64 {
    let key = serde_json::to_string(identity).expect("serializable identity");
    let mut revisions = REVISIONS.lock().unwrap_or_else(|e| e.into_inner());
    if !replace {
        if let Some(value) = revisions.get(&key) {
            return *value;
        }
    }
    if revisions.len() >= 1000 && !revisions.contains_key(&key) {
        if let Some(oldest) = revisions
            .iter()
            .min_by_key(|(_, value)| **value)
            .map(|(key, _)| key.clone())
        {
            revisions.remove(&oldest);
        }
    }
    // A retired key never returns to revision zero: late old loaders cannot
    // resurrect a receipt when its tombstone is incrementally evicted.
    let next = NEXT_REVISION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    revisions.insert(key, next);
    next
}
pub fn invalidate(identity: &crate::identity::PrIdentity) {
    revision(identity, true);
}
fn cache_key(program: &Path, row: &MergeRequest, _generation: u64) -> String {
    let identity = row.identity();
    let context = super::transport::Context::new(
        program,
        &identity.source.host,
        super::transport::Class::Advisory,
        tokio::time::Instant::now(),
    );
    serde_json::to_string(&(
        program,
        context.generation,
        identity.clone(),
        &row.viewer,
        &row.head_oid,
        &row.updated_at,
        revision(&identity, false),
    ))
    .expect("serializable MR identity")
}
fn lifetime(evidence: &Evidence) -> Duration {
    if evidence.ci.is_none()
        && evidence.review.is_none()
        && evidence.needs_my_review.is_none()
        && evidence.unresolved.is_none()
    {
        Duration::from_secs(5)
    } else {
        TTL
    }
}

pub fn fill_cached(program: &Path, rows: &mut [MergeRequest], generation: u64) {
    let keys: Vec<_> = rows
        .iter()
        .map(|row| cache_key(program, row, generation))
        .collect();
    let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let now = tokio::time::Instant::now();
    for (row, key) in rows.iter_mut().zip(keys) {
        if let Some((_, measured)) = cache
            .get(&key)
            .filter(|(at, evidence)| now.duration_since(*at) < lifetime(evidence))
        {
            measured.apply(row);
        }
    }
}

#[cfg(all(test, unix))]
pub async fn enrich(program: &Path, rows: &mut [MergeRequest], budget: Duration, generation: u64) {
    enrich_inner(None, program, rows, budget, generation).await;
}
pub async fn enrich_owned(
    context: &super::transport::Context,
    program: &Path,
    rows: &mut [MergeRequest],
    budget: Duration,
    generation: u64,
) {
    enrich_inner(Some(context), program, rows, budget, generation).await;
}
async fn enrich_inner(
    owner: Option<&super::transport::Context>,
    program: &Path,
    rows: &mut [MergeRequest],
    budget: Duration,
    generation: u64,
) {
    if owner.is_some_and(|c| !c.current()) {
        return;
    }
    if budget.is_zero() {
        return;
    }
    let now = tokio::time::Instant::now();
    let keys: Vec<_> = rows
        .iter()
        .map(|row| cache_key(program, row, generation))
        .collect();
    let mut missing = Vec::new();
    {
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, (at, _)| now.duration_since(*at) < Duration::from_secs(3600));
        for (i, row) in rows.iter_mut().enumerate() {
            if let Some((_, evidence)) = cache
                .get(&keys[i])
                .filter(|(at, evidence)| now.duration_since(*at) < lifetime(evidence))
            {
                evidence.apply(row);
            } else {
                missing.push((
                    i,
                    row.identity(),
                    row.head_oid.clone(),
                    cache.get(&keys[i]).map(|(at, _)| *at),
                    owner
                        .cloned()
                        .map(|mut context| {
                            context.class = super::transport::Class::Advisory;
                            context
                        })
                        .unwrap_or_else(|| {
                            super::transport::Context::new(
                                program,
                                &row.source.host,
                                super::transport::Class::Advisory,
                                now + budget,
                            )
                        }),
                ));
            }
        }
    }
    // Never let the first page starve later rows when refresh cadence exceeds
    // TTL. Unvisited rows go first, then the oldest measured row.
    missing.sort_by_key(|(_, _, _, at, _)| *at);
    missing.truncate(MAX_READS);
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(4));
    let attempts = &attempts;
    let reads = stream::iter(missing)
        .map(|(i, identity, head, _, mut context)| {
            context.allowance = Some(attempts.clone());
            let key = keys[i].clone();
            async move {
                let remaining = budget.saturating_sub(now.elapsed());
                if remaining.is_zero() {
                    return (i, None);
                }
                static READS: LazyLock<super::coalesce::Reads<Option<Evidence>>> =
                    LazyLock::new(super::coalesce::Reads::default);
                let measured = READS
                    .run_until(
                        key,
                        measure_context(program, &identity, head.as_deref(), &context),
                        Option::is_some,
                        now + budget,
                    )
                    .await
                    .unwrap_or(None);
                (i, measured)
            }
        })
        .buffer_unordered(2);
    tokio::pin!(reads);
    while let Some((i, measured)) = reads.next().await {
        {
            if owner.is_some_and(|c| !c.current())
                || keys[i] != cache_key(program, &rows[i], generation)
            {
                continue;
            }
            let measured = measured.unwrap_or_default();
            measured.apply(&mut rows[i]);
            let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            cache_insert(&mut cache, keys[i].clone(), measured);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test(start_paused = true)]
    async fn capacity_evicts_one_oldest_entry_and_failures_expire_early() {
        let now = tokio::time::Instant::now();
        let mut cache = HashMap::new();
        for n in 0..1000 {
            cache.insert(
                n.to_string(),
                (now + Duration::from_millis(n), Evidence::default()),
            );
        }
        cache_insert(&mut cache, "next".into(), Evidence::default());
        assert_eq!(cache.len(), 1000);
        assert!(!cache.contains_key("0"));
        assert!(cache.contains_key("999"));
        cache_insert(&mut cache, "next".into(), Evidence::default());
        assert_eq!(cache.len(), 1000);
        assert_eq!(lifetime(&Evidence::default()), Duration::from_secs(5));
        assert_eq!(
            lifetime(&Evidence {
                ci: Some(CiState::Success),
                ..Evidence::default()
            }),
            TTL
        );
    }

    #[test]
    fn schema_fallback_removes_only_the_unsupported_feature() {
        let error = json!({"errors":[{"extensions":{"code":"undefinedField","fieldName":"mergeTrainCar"}}]});
        let reduced = reduced_query(QUERY, &error).unwrap();
        assert!(!reduced.contains("mergeTrainCar"));
        for field in ["headPipeline", "reviewers", "discussions", "approvedBy"] {
            assert!(reduced.contains(field));
        }
        assert!(reduced_query(
            QUERY,
            &json!({"errors":[{"extensions":{"code":"FORBIDDEN"}}]})
        )
        .is_none());
    }

    #[test]
    fn ci_requires_the_current_head_and_known_pipeline_status() {
        let row = json!({"sha":"a","head_pipeline":{"sha":"a","status":"failed"}});
        assert_eq!(evidence(&row, Some("a")).ci, Some(CiState::Failure));
        assert_eq!(evidence(&row, Some("b")).ci, None);
        assert_eq!(
            evidence(&json!({"sha":"a","head_pipeline":null}), Some("a")).ci,
            None
        );
        assert_eq!(
            evidence(
                &json!({"sha":"a","head_pipeline":{"sha":"a","status":"new-status"}}),
                Some("a")
            )
            .ci,
            None
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn later_rows_get_four_remaining_owner_attempts_and_warm_rows_cost_zero() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, r#"#!/bin/sh
input=$(cat)
iid=$(printf '%s' "$input" | sed -n 's/.*"iid":"\([0-9]*\)".*/\1/p')
echo "$iid" >> "$0.calls"
printf 'HTTP/2 200\n\n{"data":{"project":{"mergeRequest":{"iid":"%s","webUrl":"https://owner-fixture.example/team/app/-/merge_requests/%s","diffHeadSha":"a","headPipeline":{"sha":"a","status":"SUCCESS"}}}}}' "$iid" "$iid"
"#).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let row: MergeRequest = serde_json::from_value(json!({
            "source":{"provider":"gitlab","host":"owner-fixture.example"},"id":77,"number":7,
            "title":"fixture","url":"https://owner-fixture.example/team/app/-/merge_requests/7",
            "repo":"team/app","author":"author","is_draft":false,"head_ref":"topic","head_oid":"a","base_ref":"main",
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","labels":[],"reviewers":[],"assignees":[],
            "comment_count":0,"detailed_merge_status":null,"ci":null,"review":null,"unresolved_threads":null
        })).unwrap();
        let mut rows: Vec<_> = (1..=8)
            .map(|n| {
                let mut row = row.clone();
                row.number = n;
                row.id = n;
                row.url = format!("https://owner-fixture.example/team/app/-/merge_requests/{n}");
                row
            })
            .collect();
        crate::gitlab::test_support::scripted(&program, async {
            enrich(&program, &mut rows, Duration::from_secs(30), 0).await;
            assert_eq!(
                rows.iter()
                    .filter(|r| r.ci == Some(CiState::Success))
                    .count(),
                4
            );
            // Even after the first rows age, unseen rows have scheduling priority.
            tokio::time::advance(Duration::from_secs(61)).await;
            enrich(&program, &mut rows, Duration::from_secs(30), 0).await;
            let calls = std::fs::read_to_string(program.with_extension("calls")).unwrap();
            assert_eq!(calls.lines().count(), 8);
            let mut later: Vec<_> = calls.lines().skip(4).collect();
            later.sort();
            assert_eq!(later, vec!["5", "6", "7", "8"]);
            enrich(&program, &mut rows[4..], Duration::from_secs(30), 0).await;
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                8
            );
        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn old_owner_coalesced_enrichment_cannot_publish_after_verified_rotation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, r#"#!/bin/sh
case "$5" in
user) id=1; test ! -f "$0.other" || id=2; printf 'HTTP/2 200\n\n{"id":%s,"username":"synthetic"}' "$id";;
graphql) cat >/dev/null; touch "$0.started"; while test ! -f "$0.release"; do sleep 0.01; done; printf 'HTTP/2 200\n\n{}';;
*) printf 'HTTP/2 200\n\n{}';;
esac
"#).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let row: MergeRequest = serde_json::from_value(json!({
            "source":{"provider":"gitlab","host":"owner-fixture.example"},"id":77,"number":7,
            "title":"fixture","url":"https://owner-fixture.example/team/app/-/merge_requests/7",
            "repo":"team/app","author":"author","is_draft":false,"head_ref":"topic","head_oid":"a","base_ref":"main",
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","labels":[],"reviewers":[],"assignees":[],
            "comment_count":0,"detailed_merge_status":null,"ci":null,"review":null,"unresolved_threads":null
        })).unwrap();
        crate::gitlab::test_support::scripted(&program, async {
            let host = &row.source.host;
            let ctx = super::super::transport::Context::new(
                &program,
                host,
                super::super::transport::Class::Foreground,
                tokio::time::Instant::now() + Duration::from_secs(30),
            );
            super::super::transport::api(&program, host, "user", "GET", None, &ctx)
                .await
                .unwrap();
            let old_key = cache_key(&program, &row, 0);
            let mut first = vec![row.clone()];
            let mut second = first.clone();
            let change = async {
                while !program.with_extension("started").exists() {
                    tokio::task::yield_now().await;
                }
                std::fs::write(program.with_extension("other"), "").unwrap();
                super::super::transport::api(&program, host, "user", "GET", None, &ctx)
                    .await
                    .unwrap();
                std::fs::write(program.with_extension("release"), "").unwrap();
            };
            tokio::join!(
                enrich_owned(&ctx, &program, &mut first, Duration::from_secs(30), 0),
                enrich_owned(&ctx, &program, &mut second, Duration::from_secs(30), 0),
                change
            );
            assert!(!CACHE.lock().unwrap().contains_key(&old_key));
            assert_ne!(old_key, cache_key(&program, &row, 0));
            assert_eq!(first[0].ci, None);
            assert_eq!(second[0].ci, None);
        })
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fresh_core_evidence_is_reused_but_a_changed_head_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        let source = json!({"provider":"gitlab","host":"cache-test.example"});
        let mut rows: Vec<MergeRequest> = serde_json::from_value(json!([{
            "source":source,"id":77,"number":7,"title":"fixture","url":"https://cache-test.example/team/app/-/merge_requests/7",
            "repo":"team/app","author":"author","is_draft":false,"head_ref":"topic","head_oid":"a","base_ref":"main",
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","labels":[],"reviewers":[],"assignees":[],
            "comment_count":0,"detailed_merge_status":null,"ci":null,"review":null,"unresolved_threads":null
        }])).unwrap();
        let raw = json!({"id":77,"iid":7,"title":"fixture","web_url":rows[0].url,"state":"opened","draft":false,
            "source_branch":"topic","target_branch":"main","sha":"a","head_pipeline":{"id":1,"sha":"a","status":"failed"}});
        // Consume POST stdin before replying, just as glab does. Exiting
        // early races write_all with a closed pipe on faster Linux runners.
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\ncat > /dev/null\necho read >> \"$0.calls\"\nprintf 'HTTP/2 200\\n\\n%s' '{}'\n",
                raw
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(&program, async {
            enrich(&program, &mut rows, Duration::from_secs(30), 100).await;
            assert_eq!(rows[0].ci, Some(CiState::Failure));
            rows[0].ci = None;
            enrich(&program, &mut rows, Duration::from_secs(30), 100).await;
            assert_eq!(rows[0].ci, Some(CiState::Failure));
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
            rows[0].head_oid = Some("new-head".into());
            enrich(&program, &mut rows, Duration::from_secs(30), 100).await;
            assert_eq!(rows[0].ci, None);
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                4
            );
        })
        .await;
    }
    #[test]
    fn graph_evidence_qualifies_threads_and_distinguishes_viewer_review_from_approval() {
        let identity = crate::identity::PrIdentity {
            source: crate::identity::Source {
                provider: crate::identity::Provider::Gitlab,
                host: "gitlab.com".into(),
            },
            repo: "team/app".into(),
            number: 7,
        };
        let value = json!({"data":{"currentUser":{"username":"me"},"project":{"mergeRequest":{
            "iid":"7","webUrl":"https://gitlab.com/team/app/-/merge_requests/7","diffHeadSha":"head","approved":true,
            "approvedBy":{"nodes":[{"username":"other"}]},"changeRequesters":{"nodes":[]},
            "headPipeline":{"sha":"head","status":"SUCCESS"},
            "discussions":{"nodes":[{"resolvable":true,"resolved":false}],"pageInfo":{"hasNextPage":true}},
            "reviewers":{"nodes":[{"username":"me","mergeRequestInteraction":{"reviewState":"UNREVIEWED"}}],"pageInfo":{"hasNextPage":false}},
            "mergeTrainCar":null,"availableAutoMergeStrategies":["merge_train"],"userPermissions":{"canMerge":true}
        }}}});
        let got = graph_evidence(&value, &identity, Some("head")).unwrap();
        assert_eq!(got.review, Some(ReviewState::Approved));
        assert_eq!(got.ci, Some(CiState::Success));
        assert_eq!(got.unresolved, Some(1));
        assert!(got.unresolved_floor);
        assert_eq!(got.needs_my_review, Some(true));
        assert_eq!(got.in_train, Some(false));
        assert_eq!(got.can_enqueue, Some(true));
        assert!(graph_evidence(&value, &identity, Some("other-head")).is_none());
        let mut wrong = identity;
        wrong.source.host = "other.example".into();
        assert!(graph_evidence(&value, &wrong, Some("head")).is_none());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn schema_and_legacy_rest_fallbacks_share_four_actual_api_invocations() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program,"#!/bin/sh\ncat > /dev/null\necho request >> \"$0.calls\"\nprintf 'HTTP/2 200\\n\\n{}'\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(&program, async {
            let allowance = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(4));
            let identity = crate::identity::PrIdentity {
                source: crate::identity::Source {
                    provider: crate::identity::Provider::Gitlab,
                    host: "gitlab.com".into(),
                },
                repo: "fixture/project".into(),
                number: 7,
            };
            // Serial measurements ensure the legacy REST fallback is actually reached,
            // rather than four simultaneous GraphQL calls consuming all permits first.
            for _ in 0..4 {
                assert!(measure(
                    &program,
                    &identity,
                    Some("head"),
                    Duration::from_secs(3),
                    &allowance
                )
                .await
                .is_none());
            }
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                4
            );
            assert_eq!(allowance.load(std::sync::atomic::Ordering::Acquire), 0);
        })
        .await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn successive_schema_fallbacks_cannot_add_a_fifth_api_process() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        let script = r#"#!/bin/sh
input=$(cat)
echo request >> "$0.calls"
case "$input" in
 *mergeTrainCar*) field=mergeTrainCar ;;
 *availableAutoMergeStrategies*) field=availableAutoMergeStrategies ;;
 *changeRequesters*) field=changeRequesters ;;
 *) printf 'HTTP/2 200\n\n{}'; exit 0 ;;
esac
printf 'HTTP/2 200\n\n{"errors":[{"extensions":{"code":"undefinedField","fieldName":"%s"}}]}' "$field"
"#;
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(&program, async {
            let allowance = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(4));
            let identity = crate::identity::PrIdentity {
                source: crate::identity::Source {
                    provider: crate::identity::Provider::Gitlab,
                    host: "gitlab.com".into(),
                },
                repo: "fixture/project".into(),
                number: 7,
            };
            assert!(measure(
                &program,
                &identity,
                Some("head"),
                Duration::from_secs(3),
                &allowance
            )
            .await
            .is_none());
            assert_eq!(
                std::fs::read_to_string(program.with_extension("calls"))
                    .unwrap()
                    .lines()
                    .count(),
                4
            );
        })
        .await;
    }
}
