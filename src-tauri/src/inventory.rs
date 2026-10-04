//! Qualified observations shared by provider inventories.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessField {
    Head,
    Draft,
    Ci,
    Merge,
    Review,
    Queue,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationState {
    Observed,
    Retained,
}
/// Positive source facts used only to refresh selected full details. Older
/// payloads default to no evidence; this does not grant action authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailField {
    Base,
    Comments,
    Threads,
    Reviewers,
    Reviews,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowObservation {
    pub state: ObservationState,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub unknown_fields: Vec<ReadinessField>,
    pub retained_fields: Vec<ReadinessField>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detail_fields: Vec<DetailField>,
    /// Evidence for the separate ready-for-review timestamp. Absence is
    /// unknown for old snapshots and partial provider responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at_state: Option<ObservationState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_review: Option<ConfirmedReview>,
}

pub trait InventoryRow: Clone {
    fn identity(&self) -> crate::identity::PrIdentity;
    fn observation(&self) -> &Option<RowObservation>;
    fn observation_mut(&mut self) -> &mut Option<RowObservation>;
    fn head(&self) -> Option<&str>;
    fn copy_field(&mut self, old: &Self, field: ReadinessField);
    fn retain_ready_at(&mut self, _old: &Self) -> bool {
        false
    }
    fn carry_effect(&mut self, _old: &Self) {}
}
impl InventoryRow for crate::github::model::PullRequest {
    fn identity(&self) -> crate::identity::PrIdentity {
        self.identity()
    }
    fn observation(&self) -> &Option<RowObservation> {
        &self.observation
    }
    fn observation_mut(&mut self) -> &mut Option<RowObservation> {
        &mut self.observation
    }
    fn head(&self) -> Option<&str> {
        (!self.head_oid.is_empty()).then_some(self.head_oid.as_str())
    }
    fn carry_effect(&mut self, old: &Self) {
        if let Some(effect) = old
            .observation
            .as_ref()
            .and_then(|o| o.confirmed_review.as_ref())
        {
            if let Some(next) =
                reconciled_review_effect(effect, &self.head_oid, &self.latest_reviews)
            {
                apply_confirmed_review(self, &next);
            }
        }
    }
    fn copy_field(&mut self, old: &Self, field: ReadinessField) {
        match field {
            ReadinessField::Head => {}
            ReadinessField::Draft => self.is_draft = old.is_draft,
            ReadinessField::Ci => self.ci = old.ci,
            ReadinessField::Merge => self.merge = old.merge,
            ReadinessField::Review => self.review = old.review,
            ReadinessField::Queue => self.in_merge_queue = old.in_merge_queue,
        }
    }
    fn retain_ready_at(&mut self, old: &Self) -> bool {
        let observed = |row: &Self| {
            row.observation.as_ref().is_some_and(|o| {
                !o.unknown_fields.contains(&ReadinessField::Head)
                    && !o.unknown_fields.contains(&ReadinessField::Draft)
                    && o.detail_fields.contains(&DetailField::Base)
            })
        };
        if self.ready_at.is_none()
            && old.ready_at.is_some()
            && !self.is_draft
            && !old.is_draft
            && !self.base_ref.is_empty()
            && self.base_ref == old.base_ref
            && observed(self)
            && observed(old)
            && old
                .observation
                .as_ref()
                .and_then(|o| o.ready_at_state)
                .is_some()
        {
            self.ready_at = old.ready_at;
            return true;
        }
        false
    }
}
impl InventoryRow for crate::gitlab::queues::MergeRequest {
    fn identity(&self) -> crate::identity::PrIdentity {
        self.identity()
    }
    fn observation(&self) -> &Option<RowObservation> {
        &self.observation
    }
    fn observation_mut(&mut self) -> &mut Option<RowObservation> {
        &mut self.observation
    }
    fn head(&self) -> Option<&str> {
        self.head_oid.as_deref().filter(|h| !h.is_empty())
    }
    fn carry_effect(&mut self, old: &Self) {
        if let Some(effect) = old
            .observation
            .as_ref()
            .and_then(|o| o.confirmed_review.as_ref())
        {
            if self.head_oid.as_deref() == Some(effect.head_oid.as_str()) {
                self.needs_my_review = Some(false);
                if let Some(observation) = self.observation.as_mut() {
                    let mut qualified = effect.clone();
                    qualify_review_effect(&mut qualified, Utc::now());
                    observation.confirmed_review = Some(qualified);
                }
            }
        }
    }
    fn copy_field(&mut self, old: &Self, field: ReadinessField) {
        match field {
            ReadinessField::Head => {}
            ReadinessField::Draft => self.is_draft = old.is_draft,
            ReadinessField::Ci => self.ci = old.ci,
            ReadinessField::Merge => self.detailed_merge_status = old.detailed_merge_status.clone(),
            ReadinessField::Review => self.review = old.review,
            ReadinessField::Queue => self.in_merge_queue = old.in_merge_queue,
        }
    }
}

pub fn reconcile<T: InventoryRow>(
    previous: Vec<T>,
    incoming: Vec<T>,
    complete: bool,
    now: DateTime<Utc>,
) -> Vec<T> {
    reconcile_rows(previous, incoming, complete, false, now)
}

/// A page delta says nothing about identities omitted from that page.
pub fn reconcile_delta<T: InventoryRow>(
    previous: Vec<T>,
    incoming: Vec<T>,
    now: DateTime<Utc>,
) -> Vec<T> {
    reconcile_rows(previous, incoming, false, true, now)
}

fn reconcile_rows<T: InventoryRow>(
    previous: Vec<T>,
    incoming: Vec<T>,
    complete: bool,
    delta: bool,
    now: DateTime<Utc>,
) -> Vec<T> {
    let mut prior: std::collections::HashMap<_, _> =
        previous.into_iter().map(|r| (r.identity(), r)).collect();
    let mut output = Vec::with_capacity(incoming.len() + prior.len());
    for mut row in incoming {
        let old = prior.remove(&row.identity());
        // Missing identity is not evidence of a new head. Preserve the last
        // established head and its effects, explicitly as last-known data.
        if row.head().is_none() {
            if let Some(mut old) = old.clone().filter(|old| old.head().is_some()) {
                old.observation_mut()
                    .get_or_insert(RowObservation {
                        state: ObservationState::Retained,
                        last_observed_at: None,
                        unknown_fields: vec![],
                        retained_fields: vec![],
                        detail_fields: vec![],
                        ready_at_state: None,
                        confirmed_review: None,
                    })
                    .state = ObservationState::Retained;
                output.push(old);
                continue;
            }
        }
        let mut observation = row.observation().clone().unwrap_or(RowObservation {
            state: ObservationState::Observed,
            last_observed_at: Some(now),
            unknown_fields: vec![],
            retained_fields: vec![],
            detail_fields: vec![],
            ready_at_state: None,
            confirmed_review: None,
        });
        if row.head().is_none() && !observation.unknown_fields.contains(&ReadinessField::Head) {
            observation.unknown_fields.push(ReadinessField::Head);
        }
        observation.state = ObservationState::Observed;
        observation.last_observed_at = Some(now);
        observation.retained_fields.clear();
        if let Some(old) = old
            .as_ref()
            .filter(|old| row.head().is_some() && row.head() == old.head())
        {
            let retained_ready_at =
                observation.ready_at_state.is_none() && row.retain_ready_at(old);
            observation.unknown_fields.retain(|field| {
                if old
                    .observation()
                    .as_ref()
                    .is_some_and(|o| o.unknown_fields.contains(field))
                {
                    return true;
                }
                row.copy_field(old, *field);
                observation.retained_fields.push(*field);
                false
            });
            if retained_ready_at {
                observation.ready_at_state = Some(ObservationState::Retained);
            }
        }
        *row.observation_mut() = Some(observation);
        if let Some(old) = old.as_ref() {
            row.carry_effect(old);
        }
        output.push(row);
    }
    if !complete {
        // Preserve stable display order for unobserved rows independently of map iteration.
        let mut retained: Vec<_> = prior.into_values().collect();
        retained.sort_by_key(|row| {
            let id = row.identity();
            (id.repo, id.number)
        });
        for mut row in retained {
            let observation = row.observation_mut().get_or_insert(RowObservation {
                state: ObservationState::Retained,
                last_observed_at: None,
                unknown_fields: vec![],
                retained_fields: vec![],
                detail_fields: vec![],
                ready_at_state: None,
                confirmed_review: None,
            });
            if !delta {
                observation.state = ObservationState::Retained;
            }
            if let Some(effect) = &mut observation.confirmed_review {
                qualify_review_effect(effect, now);
            }
            output.push(row);
        }
    }
    for row in &mut output {
        if let Some(effect) = row
            .observation_mut()
            .as_mut()
            .and_then(|o| o.confirmed_review.as_mut())
        {
            qualify_review_effect(effect, now);
        }
    }
    output
}

/// Receipt cardinality excludes retained inventory, which is not newly observed.
pub fn observed_count<T: InventoryRow>(rows: &[T]) -> u64 {
    rows.iter()
        .filter(|r| {
            !r.observation()
                .as_ref()
                .is_some_and(|o| o.state == ObservationState::Retained)
        })
        .count() as u64
}

pub fn github_observation(node: &serde_json::Value, partial: bool) -> RowObservation {
    use ReadinessField::*;
    let mut unknown = vec![];
    if node["headRefOid"].as_str().is_none_or(str::is_empty) {
        unknown.push(Head);
    }
    let errored = |field: &str| {
        partial
            || node["__readiness_errors"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|f| f.as_str() == Some(field)))
    };
    if node["isDraft"].as_bool().is_none() {
        unknown.push(Draft);
    }
    let commit = &node["commits"]["nodes"][0]["commit"];
    let ci = commit.get("statusCheckRollup");
    if !matches!(
        ci.and_then(|v| v["state"].as_str()),
        Some("SUCCESS" | "FAILURE" | "ERROR" | "PENDING" | "EXPECTED")
    ) && !(ci.is_some_and(serde_json::Value::is_null) && !errored("ci"))
    {
        unknown.push(Ci);
    }
    if !matches!(
        node["mergeable"].as_str(),
        Some("MERGEABLE" | "CONFLICTING")
    ) {
        unknown.push(Merge);
    }
    if !matches!(
        node["reviewDecision"].as_str(),
        Some("APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED")
    ) && !(node
        .get("reviewDecision")
        .is_some_and(serde_json::Value::is_null)
        && !errored("review"))
    {
        unknown.push(Review);
    }
    if node["isInMergeQueue"].as_bool().is_none() {
        unknown.push(Queue);
    }
    let mut detail_fields = vec![];
    if !errored("base") && node["baseRefName"].as_str().is_some_and(|s| !s.is_empty()) {
        detail_fields.push(DetailField::Base);
    }
    if !errored("comments") && node["totalCommentsCount"].as_u64().is_some() {
        detail_fields.push(DetailField::Comments);
    }
    if !errored("threads")
        && node["reviewThreads"]["nodes"]
            .as_array()
            .is_some_and(|nodes| {
                nodes
                    .iter()
                    .all(|n| n["isResolved"].is_boolean() && n["isOutdated"].is_boolean())
            })
    {
        detail_fields.push(DetailField::Threads);
    }
    for (field, name, group) in [
        ("reviewRequests", "reviewers", DetailField::Reviewers),
        ("latestReviews", "reviews", DetailField::Reviews),
    ] {
        if !errored(name)
            && node[field]["totalCount"].as_u64().is_some()
            && node[field]["nodes"].as_array().is_some_and(|nodes| {
                nodes.iter().all(|n| match group {
                    DetailField::Reviewers => n["requestedReviewer"]["login"].as_str().is_some(),
                    DetailField::Reviews => {
                        n["author"]["login"].as_str().is_some()
                            && n["state"].as_str().is_some()
                            && n["id"].as_str().is_some()
                            && n.get("submittedAt").is_some()
                            && n.get("commit")
                                .is_some_and(|c| c.is_null() || c["oid"].as_str().is_some())
                    }
                    _ => false,
                })
            })
        {
            detail_fields.push(group);
        }
    }
    let ready_at_state = (!errored("ready_at") && !node["isDraft"].as_bool().unwrap_or(true))
        .then(|| node["timelineItems"]["nodes"].as_array())
        .flatten()
        .filter(|nodes| {
            nodes.last().is_none_or(|event| {
                event["createdAt"]
                    .as_str()
                    .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                    .is_some()
            })
        })
        .map(|_| ObservationState::Observed);
    RowObservation {
        state: ObservationState::Observed,
        last_observed_at: None,
        unknown_fields: unknown,
        retained_fields: vec![],
        detail_fields,
        ready_at_state,
        confirmed_review: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmedReview {
    pub head_oid: String,
    pub review: crate::github::model::ReviewState,
    pub confirmed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<crate::github::mutate::SubmittedReview>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unresolved: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirmed_by_read: bool,
}

/// Preserve the own-review fact separately from the aggregate requirements.
fn reconciled_review_effect(
    effect: &ConfirmedReview,
    head: &str,
    reviews: &[crate::github::model::ReviewerVerdict],
) -> Option<ConfirmedReview> {
    if !head.is_empty() && head != effect.head_oid {
        return None;
    }
    let mut next = effect.clone();
    let Some(receipt) = &effect.receipt else {
        return Some(next);
    };
    if let Some(review) = reviews.iter().find(|review| {
        review.author.eq_ignore_ascii_case(&receipt.actor)
            && matches!(
                review.state.as_str(),
                "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED"
            )
            && (review.id.as_deref() == Some(&receipt.review_id)
                || (review.id.as_ref().is_some_and(|id| !id.is_empty())
                    && review
                        .submitted_at
                        .zip(receipt.submitted_at)
                        .is_some_and(|(read, written)| read > written)))
    }) {
        let verdict = match review.state.as_str() {
            "APPROVED" => crate::github::model::ReviewState::Approved,
            "CHANGES_REQUESTED" => crate::github::model::ReviewState::ChangesRequested,
            _ => return None,
        };
        // A later review on another/unknown commit cannot establish current-head approval.
        if review.id.as_deref() != Some(&receipt.review_id)
            && review.commit_oid.as_deref() != Some(effect.head_oid.as_str())
        {
            return None;
        }
        next.review = verdict;
        next.confirmed_by_read = true;
        next.unresolved = false;
        next.receipt = Some(crate::github::mutate::SubmittedReview {
            review_id: review
                .id
                .clone()
                .unwrap_or_else(|| receipt.review_id.clone()),
            state: review.state.clone(),
            actor: review.author.clone(),
            commit_oid: effect.head_oid.clone(),
            submitted_at: review.submitted_at.or(receipt.submitted_at),
            ..receipt.clone()
        });
    }
    Some(next)
}
#[cfg(test)]
fn review_effect_resolved(
    effect: &ConfirmedReview,
    head: &str,
    reviews: &[crate::github::model::ReviewerVerdict],
) -> bool {
    reconciled_review_effect(effect, head, reviews).is_none()
}
pub fn qualify_review_effect(effect: &mut ConfirmedReview, now: DateTime<Utc>) {
    if !effect.confirmed_by_read
        && now.signed_duration_since(effect.confirmed_at) >= chrono::Duration::minutes(15)
    {
        effect.unresolved = true;
    }
}
pub fn reconcile_detail_review(
    row: &mut crate::github::model::PullRequest,
    detail: &crate::github::model::PrDetail,
) -> bool {
    let Some(observation) = &mut row.observation else {
        return false;
    };
    let Some(effect) = &mut observation.confirmed_review else {
        return false;
    };
    let previous = effect.clone();
    let mut next = reconciled_review_effect(effect, &detail.head_oid, &detail.latest_reviews);
    if let Some(effect) = &mut next {
        qualify_review_effect(effect, Utc::now());
    }
    let changed = next.as_ref() != Some(&previous);
    observation.confirmed_review = next;
    changed
}

/// A verified mutation is stronger than a lagging or omitted ordinary read.
/// Old-head effects never cross a positively changed head.
pub fn apply_confirmed_review(
    row: &mut crate::github::model::PullRequest,
    effect: &ConfirmedReview,
) -> bool {
    if row.head_oid.is_empty() || row.head_oid != effect.head_oid {
        return false;
    }
    let observation = row.observation.get_or_insert(RowObservation {
        state: ObservationState::Observed,
        last_observed_at: None,
        unknown_fields: vec![],
        retained_fields: vec![],
        detail_fields: vec![],
        ready_at_state: None,
        confirmed_review: None,
    });
    observation.confirmed_review = Some(effect.clone());
    true
}

/// Preserve only a closed readiness classification from GraphQL error paths.
/// Provider text and arbitrary path strings never cross the model boundary.
pub fn mark_readiness_errors(data: &mut serde_json::Value, errors: &[serde_json::Value]) {
    if !data.is_object() {
        return;
    }
    for error in errors.iter().take(1024) {
        let Some(path) = error["path"].as_array() else {
            data["__readiness_unknown"] = true.into();
            continue;
        };
        if path.first().and_then(|v| v.as_str()) != Some("authored") {
            continue;
        }
        let Some(index) = path.get(2).and_then(|v| v.as_u64()).filter(|n| *n < 250) else {
            data["__readiness_unknown"] = true.into();
            continue;
        };
        let field = match path.get(3).and_then(|v| v.as_str()) {
            Some("baseRefName") => "base",
            Some("totalCommentsCount") => "comments",
            Some("reviewThreads") => "threads",
            Some("reviewRequests") => "reviewers",
            Some("latestReviews") => "reviews",
            Some("timelineItems") => "ready_at",
            Some("isDraft") => "draft",
            Some("commits") => "ci",
            Some("mergeable") => "merge",
            Some("reviewDecision") => "review",
            Some("isInMergeQueue" | "mergeQueueEntry") => "queue",
            _ => continue,
        };
        if let Some(node) = data
            .get_mut("authored")
            .and_then(|v| v.get_mut("nodes"))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|nodes| nodes.get_mut(index as usize))
            .and_then(|node| node.as_object_mut())
        {
            let errors = node
                .entry("__readiness_errors")
                .or_insert_with(|| serde_json::json!([]));
            if let Some(fields) = errors.as_array_mut() {
                let field = serde_json::Value::String(field.into());
                if !fields.contains(&field) {
                    fields.push(field);
                }
            }
        }
    }
    if errors.len() > 1024 {
        data["__readiness_unknown"] = true.into();
    }
}

pub fn gitlab_observation(row: &crate::gitlab::queues::MergeRequest) -> RowObservation {
    let mut unknown_fields = vec![];
    if row.head().is_none() {
        unknown_fields.push(ReadinessField::Head);
    }
    if row.ci.is_none() {
        unknown_fields.push(ReadinessField::Ci);
    }
    if row.review.is_none() {
        unknown_fields.push(ReadinessField::Review);
    }
    if row.detailed_merge_status.is_none() {
        unknown_fields.push(ReadinessField::Merge);
    }
    if row.in_merge_queue.is_none() {
        unknown_fields.push(ReadinessField::Queue);
    }
    RowObservation {
        state: ObservationState::Observed,
        last_observed_at: None,
        unknown_fields,
        retained_fields: vec![],
        detail_fields: vec![],
        ready_at_state: None,
        confirmed_review: None,
    }
}

/// Only execute's semantically verified, account/head-bound action can patch
/// retained GitLab inventory. An explicit later reopened row remains observable.
pub fn apply_gitlab_action(
    rows: &mut Vec<crate::gitlab::queues::MergeRequest>,
    request: &crate::gitlab::actions::ActionRequest,
    receipt: &crate::gitlab::actions::Receipt,
) -> bool {
    use crate::gitlab::actions::{Action, Outcome};
    if receipt.outcome != Outcome::Verified
        || receipt.identity != request.identity
        || receipt.action != request.action
    {
        return false;
    }
    let (Some(viewer), Some(head)) = (
        request.expected_viewer.as_deref(),
        request.expected_head.as_deref(),
    ) else {
        return false;
    };
    if viewer.is_empty() || head.is_empty() {
        return false;
    }
    let matches = |row: &crate::gitlab::queues::MergeRequest| {
        row.identity() == request.identity
            && row.viewer.as_deref() == Some(viewer)
            && row.head_oid.as_deref() == Some(head)
    };
    if matches!(request.action, Action::Close | Action::Merge) {
        let before = rows.len();
        rows.retain(|row| !matches(row));
        return rows.len() != before;
    }
    let review = match request.action {
        Action::Approve => crate::github::model::ReviewState::Approved,
        Action::RequestChanges => crate::github::model::ReviewState::ChangesRequested,
        _ => return false,
    };
    let mut changed = false;
    for row in rows.iter_mut().filter(|row| matches(row)) {
        row.needs_my_review = Some(false);
        let observation = row.observation.get_or_insert(RowObservation {
            state: ObservationState::Observed,
            last_observed_at: None,
            unknown_fields: vec![],
            retained_fields: vec![],
            detail_fields: vec![],
            ready_at_state: None,
            confirmed_review: None,
        });
        observation.confirmed_review = Some(ConfirmedReview {
            head_oid: head.into(),
            review,
            confirmed_at: Utc::now(),
            receipt: None,
            unresolved: false,
            confirmed_by_read: false,
        });
        changed = true;
    }
    changed
}

pub fn apply_github_removal(
    rows: &mut Vec<crate::github::model::PullRequest>,
    repo: &str,
    number: u64,
    effect: &crate::github::mutate::ConfirmedRemoval,
) -> bool {
    let before = rows.len();
    rows.retain(|row| {
        !(row.source == crate::identity::Source::default()
            && row.repo == repo
            && row.number == number
            && row.id == effect.id
            && row.head_oid == effect.head_oid)
    });
    rows.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rows() -> Vec<crate::github::model::PullRequest> {
        crate::github::map::map_list(
            &serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap(),
            "authored",
        )
    }
    #[test]
    fn unread_head_keeps_effect_until_positive_head_evidence() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/inventory-head-transitions.json"
        ))
        .unwrap();
        let now = contract["time"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap();
        let assert_contract = |name: &str, row: &crate::github::model::PullRequest| {
            let actual = serde_json::to_value(row).unwrap();
            for (key, expected) in contract["expected"][name].as_object().unwrap() {
                assert_eq!(&actual[key], expected, "{name}.{key}");
            }
        };
        let mut response: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
        let node = &mut response["authored"]["nodes"][0];
        node["headRefOid"] = serde_json::json!("head-a");
        node["isDraft"] = serde_json::json!(false);
        node["isInMergeQueue"] = serde_json::json!(false);
        node["mergeable"] = serde_json::json!("MERGEABLE");
        node["reviewDecision"] = serde_json::json!("REVIEW_REQUIRED");
        node["commits"]["nodes"][0]["commit"]["statusCheckRollup"] =
            serde_json::json!({"state":"SUCCESS"});
        let mapped = |v: &serde_json::Value| crate::github::map::map_list(v, "authored").remove(0);
        let mut old = mapped(&response);
        assert!(apply_confirmed_review(
            &mut old,
            &ConfirmedReview {
                head_oid: "head-a".into(),
                review: crate::github::model::ReviewState::Approved,
                confirmed_at: now,
                receipt: None,
                unresolved: false,
                confirmed_by_read: false,
            }
        ));
        for head in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!("")),
        ] {
            let mut unread = response.clone();
            if let Some(head) = head {
                unread["authored"]["nodes"][0]["headRefOid"] = head;
            } else {
                unread["authored"]["nodes"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("headRefOid");
            }
            let fresh = mapped(&unread);
            let new = reconcile(vec![], vec![fresh.clone()], false, now);
            assert!(!new[0]
                .observation
                .as_ref()
                .unwrap()
                .unknown_fields
                .is_empty());
            assert_contract("new_unread", &new[0]);
            let mut unreviewed = old.clone();
            unreviewed.observation.as_mut().unwrap().confirmed_review = None;
            let unreviewed = reconcile(vec![unreviewed], vec![fresh.clone()], false, now);
            assert_contract("retained_unread_without_effect", &unreviewed[0]);
            assert_eq!(observed_count(&unreviewed), 0);
            let retained = reconcile(vec![old.clone()], vec![fresh], false, now);
            assert_eq!(retained[0].head_oid, "head-a");
            assert_eq!(
                retained[0].observation.as_ref().unwrap().state,
                ObservationState::Retained
            );
            assert!(retained[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .is_some());
            assert_contract("retained_unread", &retained[0]);
            let lagging = reconcile(retained, vec![mapped(&response)], true, now);
            assert!(lagging[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .is_some());
            assert_contract("same_head", &lagging[0]);
            let mut changed = response.clone();
            changed["authored"]["nodes"][0]["headRefOid"] = serde_json::json!("head-b");
            let changed = reconcile(lagging, vec![mapped(&changed)], true, now);
            assert_contract("changed_head", &changed[0]);
            assert!(changed[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .is_none());
            assert!(changed[0]
                .observation
                .as_ref()
                .unwrap()
                .unknown_fields
                .is_empty());
        }
    }
    #[test]
    fn verified_removal_cannot_return_through_partial_omission() {
        let mut row = rows().remove(0);
        row.id = "PR-a".into();
        row.head_oid = "head-a".into();
        let mut inventory = vec![row.clone()];
        let mut proof = crate::github::mutate::ConfirmedRemoval {
            viewer: "fixture".into(),
            id: row.id.clone(),
            head_oid: "head-b".into(),
        };
        assert!(!apply_github_removal(
            &mut inventory,
            &row.repo,
            row.number,
            &proof
        ));
        proof.head_oid = row.head_oid.clone();
        assert!(apply_github_removal(
            &mut inventory,
            &row.repo,
            row.number,
            &proof
        ));
        assert!(reconcile(inventory.clone(), vec![], false, Utc::now()).is_empty());
        assert_eq!(reconcile(inventory, vec![row], true, Utc::now()).len(), 1);
    }

    #[test]
    fn identity_merge_keeps_unseen_repository_and_updates_only_matching_row() {
        let a = rows().remove(0);
        let mut b = a.clone();
        b.repo = "fixture/other".into();
        let mut update = a.clone();
        update.title = "updated".into();
        let merged = reconcile(vec![a, b.clone()], vec![update], false, Utc::now());
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].title, "updated");
        assert_eq!(merged[1].repo, b.repo);
        assert_eq!(merged[1].title, b.title);
        assert_eq!(
            merged[1].observation.as_ref().unwrap().state,
            ObservationState::Retained
        );
    }

    #[test]
    fn same_head_reuses_known_fields_but_new_head_never_does() {
        let mut old = rows().remove(0);
        old.head_oid = "head-a".into();
        old.ci = crate::github::model::CiState::Success;
        let mut incoming = old.clone();
        incoming.ci = crate::github::model::CiState::None;
        incoming.observation.as_mut().unwrap().unknown_fields = vec![ReadinessField::Ci];
        let same = reconcile(vec![old.clone()], vec![incoming.clone()], false, Utc::now());
        assert_eq!(same[0].ci, crate::github::model::CiState::Success);
        assert_eq!(
            same[0].observation.as_ref().unwrap().retained_fields,
            vec![ReadinessField::Ci]
        );
        incoming.head_oid = "head-b".into();
        let changed = reconcile(vec![old], vec![incoming], false, Utc::now());
        assert_eq!(
            changed[0].observation.as_ref().unwrap().unknown_fields,
            vec![ReadinessField::Ci]
        );
        assert_eq!(changed[0].ci, crate::github::model::CiState::None);
    }
    #[test]
    fn ready_date_retains_only_across_positive_compatible_facts() {
        let date = "2026-08-19T15:30:00Z".parse::<DateTime<Utc>>().unwrap();
        let qualified = |row: &mut crate::github::model::PullRequest| {
            row.head_oid = "head-a".into();
            row.base_ref = "main".into();
            row.is_draft = false;
            let observation = row.observation.as_mut().unwrap();
            observation.unknown_fields.clear();
            if !observation.detail_fields.contains(&DetailField::Base) {
                observation.detail_fields.push(DetailField::Base);
            }
        };
        let mut old = rows().remove(0);
        qualified(&mut old);
        old.ready_at = Some(date);
        old.observation.as_mut().unwrap().ready_at_state = Some(ObservationState::Observed);

        let mut refused = old.clone();
        refused.ready_at = None;
        refused.observation.as_mut().unwrap().ready_at_state = None;
        let retained = reconcile(vec![old.clone()], vec![refused.clone()], true, Utc::now());
        assert_eq!(retained[0].ready_at, Some(date));
        assert_eq!(
            retained[0].observation.as_ref().unwrap().ready_at_state,
            Some(ObservationState::Retained)
        );

        for mutate in [
            |row: &mut crate::github::model::PullRequest| row.head_oid = "head-b".into(),
            |row: &mut crate::github::model::PullRequest| row.base_ref = "release".into(),
            |row: &mut crate::github::model::PullRequest| row.is_draft = true,
        ] as [fn(&mut crate::github::model::PullRequest); 3]
        {
            let mut changed = refused.clone();
            mutate(&mut changed);
            let result = reconcile(vec![old.clone()], vec![changed], true, Utc::now());
            assert_eq!(result[0].ready_at, None);
            assert_eq!(result[0].observation.as_ref().unwrap().ready_at_state, None);
        }

        let mut first_seen = refused.clone();
        first_seen.observation.as_mut().unwrap().ready_at_state = None;
        assert_eq!(
            reconcile(vec![], vec![first_seen], true, Utc::now())[0].ready_at,
            None
        );

        let mut prior_draft = old.clone();
        prior_draft.is_draft = true;
        let mut newly_ready_without_timeline = refused.clone();
        newly_ready_without_timeline.is_draft = false;
        let after_draft = reconcile(
            vec![prior_draft],
            vec![newly_ready_without_timeline],
            true,
            Utc::now(),
        );
        assert_eq!(after_draft[0].ready_at, None);
        assert_eq!(
            after_draft[0].observation.as_ref().unwrap().ready_at_state,
            None
        );

        let mut unknown_draft = refused.clone();
        unknown_draft
            .observation
            .as_mut()
            .unwrap()
            .unknown_fields
            .push(ReadinessField::Draft);
        let unknown_draft = reconcile(vec![old.clone()], vec![unknown_draft], true, Utc::now());
        assert_eq!(unknown_draft[0].ready_at, None);

        let newer = "2026-09-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut observed = refused;
        observed.ready_at = Some(newer);
        observed.observation.as_mut().unwrap().ready_at_state = Some(ObservationState::Observed);
        let observed = reconcile(retained, vec![observed], true, Utc::now());
        assert_eq!(observed[0].ready_at, Some(newer));
        assert_eq!(
            observed[0].observation.as_ref().unwrap().ready_at_state,
            Some(ObservationState::Observed)
        );
    }
    #[test]
    fn ready_date_provider_reconcile_fixture_matches_frontend_contract() {
        let now = "2026-10-03T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut response: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
        let template = response["authored"]["nodes"][0].clone();
        let node = |number: u64, title: &str, ready_at: &str| {
            let mut node = template.clone();
            node["number"] = number.into();
            node["id"] = format!("PR-{number}").into();
            node["title"] = title.into();
            node["headRefOid"] = format!("head-{number}").into();
            node["baseRefName"] = "main".into();
            node["isDraft"] = false.into();
            node["mergeable"] = "MERGEABLE".into();
            node["reviewDecision"] = "REVIEW_REQUIRED".into();
            node["isInMergeQueue"] = false.into();
            node["commits"]["nodes"][0]["commit"]["statusCheckRollup"] =
                serde_json::json!({"state": "SUCCESS"});
            node["timelineItems"] = serde_json::json!({"nodes": [{"createdAt": ready_at}]});
            node
        };
        response["authored"]["nodes"] = serde_json::json!([
            node(1, "Older ready", "2026-09-01T00:00:00Z"),
            node(2, "Newer ready", "2026-09-02T00:00:00Z")
        ]);
        let initial = crate::github::map::map_list(&response, "authored");

        let mut refused = response.clone();
        refused["authored"]["nodes"][0]["timelineItems"] = serde_json::Value::Null;
        mark_readiness_errors(
            &mut refused,
            &[serde_json::json!({"path": ["authored", "nodes", 0, "timelineItems"]})],
        );
        let retained = reconcile(
            initial,
            crate::github::map::map_list(&refused, "authored"),
            true,
            now,
        );

        let mut recovered = response;
        recovered["authored"]["nodes"][0]["timelineItems"] =
            serde_json::json!({"nodes": [{"createdAt": "2026-09-03T00:00:00Z"}]});
        let recovered = reconcile(
            retained.clone(),
            crate::github::map::map_list(&recovered, "authored"),
            true,
            now,
        );
        let actual = serde_json::json!({"retained": retained, "recovered": recovered});
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ready-date-reconcile.json"))
                .unwrap();
        assert_eq!(
            actual, expected,
            "fixture must be regenerated from this provider path"
        );
    }
    #[test]
    fn confirmed_review_survives_omission_restart_and_lag_but_not_new_head() {
        let mut old = rows().remove(0);
        old.head_oid = "head-a".into();
        old.review = crate::github::model::ReviewState::ReviewRequired;
        let effect = ConfirmedReview {
            head_oid: old.head_oid.clone(),
            review: crate::github::model::ReviewState::Approved,
            confirmed_at: Utc::now(),
            receipt: None,
            unresolved: false,
            confirmed_by_read: false,
        };
        assert!(apply_confirmed_review(&mut old, &effect));
        let persisted = serde_json::to_vec(&old).unwrap();
        let old: crate::github::model::PullRequest = serde_json::from_slice(&persisted).unwrap();
        let retained = reconcile(vec![old.clone()], vec![], false, Utc::now());
        assert_eq!(
            retained[0].review,
            crate::github::model::ReviewState::ReviewRequired
        );
        assert_eq!(
            retained[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .as_ref()
                .unwrap()
                .review,
            effect.review
        );
        assert_eq!(observed_count(&retained), 0);
        let mut lagging = old.clone();
        lagging.observation = None;
        lagging.review = crate::github::model::ReviewState::ReviewRequired;
        let lag = reconcile(vec![old.clone()], vec![lagging.clone()], true, Utc::now());
        assert_eq!(
            lag[0].review,
            crate::github::model::ReviewState::ReviewRequired
        );
        assert!(lag[0]
            .observation
            .as_ref()
            .unwrap()
            .confirmed_review
            .is_some());
        lagging.head_oid = "head-b".into();
        let changed = reconcile(vec![old], vec![lagging], true, Utc::now());
        assert_eq!(
            changed[0].review,
            crate::github::model::ReviewState::ReviewRequired
        );
        assert!(changed[0]
            .observation
            .as_ref()
            .unwrap()
            .confirmed_review
            .is_none());
    }
    #[test]
    fn detail_fields_require_positive_non_refused_values_and_old_payloads_are_unknown() {
        let node = serde_json::json!({
            "baseRefName": "main", "totalCommentsCount": 0,
            "reviewThreads": {"nodes": [], "totalCount": 0},
            "reviewRequests": {"nodes": [], "totalCount": 0},
            "latestReviews": {"nodes": [], "totalCount": 0}
        });
        let observed = serde_json::to_value(github_observation(&node, false)).unwrap();
        assert_eq!(
            observed["detail_fields"],
            serde_json::json!(["base", "comments", "threads", "reviewers", "reviews"])
        );
        for (path, group) in [
            ("baseRefName", "base"),
            ("totalCommentsCount", "comments"),
            ("reviewThreads", "threads"),
            ("reviewRequests", "reviewers"),
            ("latestReviews", "reviews"),
        ] {
            let mut response = serde_json::json!({"authored": {"nodes": [node.clone()]}});
            mark_readiness_errors(
                &mut response,
                &[serde_json::json!({"path": ["authored", "nodes", 0, path]})],
            );
            let refused =
                serde_json::to_value(github_observation(&response["authored"]["nodes"][0], false))
                    .unwrap();
            assert!(!refused["detail_fields"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(group)));
        }
        for partial in [false, true] {
            let missing =
                serde_json::to_value(github_observation(&serde_json::json!({}), partial)).unwrap();
            assert!(missing["detail_fields"]
                .as_array()
                .is_none_or(Vec::is_empty));
        }
        let mut timeline = serde_json::json!({"authored": {"nodes": [{
            "isDraft": false, "timelineItems": {"nodes": []}
        }]}});
        mark_readiness_errors(
            &mut timeline,
            &[serde_json::json!({"path": ["authored", "nodes", 0, "timelineItems"]})],
        );
        let refused = github_observation(&timeline["authored"]["nodes"][0], false);
        assert_eq!(refused.ready_at_state, None);
        for (field, group) in [
            ("reviewRequests", "reviewers"),
            ("latestReviews", "reviews"),
        ] {
            let mut incomplete = node.clone();
            incomplete[field]["nodes"] = serde_json::json!([{}]);
            let observation = serde_json::to_value(github_observation(&incomplete, false)).unwrap();
            assert!(!observation["detail_fields"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(group)));
        }
        let mut old = observed;
        old.as_object_mut().unwrap().remove("detail_fields");
        let old: RowObservation = serde_json::from_value(old).unwrap();
        let old = serde_json::to_value(old).unwrap();
        assert!(old["detail_fields"].as_array().is_none_or(Vec::is_empty));
        assert!(old.get("ready_at_state").is_none());

        let states = [
            Some(ObservationState::Observed),
            Some(ObservationState::Retained),
            None,
        ];
        for state in states {
            let mut value = github_observation(&node, false);
            value.ready_at_state = state;
            let round_trip: RowObservation =
                serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
            assert_eq!(round_trip.ready_at_state, state);
        }
    }

    #[test]
    fn readiness_errors_are_field_specific_and_no_checks_remains_valid() {
        let mut response: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/search.json")).unwrap();
        response["authored"]["nodes"][0]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] =
            serde_json::Value::Null;
        mark_readiness_errors(
            &mut response,
            &[serde_json::json!({"path": ["authored", "nodes", 0, "labels"]})],
        );
        let good = crate::github::map::map_list(&response, "authored");
        assert!(!good[0]
            .observation
            .as_ref()
            .unwrap()
            .unknown_fields
            .contains(&ReadinessField::Ci));
        mark_readiness_errors(
            &mut response,
            &[
                serde_json::json!({"path": ["authored", "nodes", 0, "commits", "nodes", 0, "commit", "statusCheckRollup"]}),
            ],
        );
        let unknown = crate::github::map::map_list(&response, "authored");
        assert!(unknown[0]
            .observation
            .as_ref()
            .unwrap()
            .unknown_fields
            .contains(&ReadinessField::Ci));
        for (key, field) in [
            ("isDraft", ReadinessField::Draft),
            ("reviewDecision", ReadinessField::Review),
            ("mergeable", ReadinessField::Merge),
            ("isInMergeQueue", ReadinessField::Queue),
        ] {
            let mut failed = response.clone();
            failed["authored"]["nodes"][0][key] = serde_json::Value::Null;
            mark_readiness_errors(
                &mut failed,
                &[serde_json::json!({"path": ["authored", "nodes", 0, key]})],
            );
            let rows = crate::github::map::map_list(&failed, "authored");
            assert!(rows[0]
                .observation
                .as_ref()
                .unwrap()
                .unknown_fields
                .contains(&field));
        }
    }
    #[test]
    fn partial_omission_retains_qualified_rows_but_complete_removes_them() {
        let previous = rows();
        assert!(!previous.is_empty());
        let next = reconcile(previous.clone(), vec![], false, Utc::now());
        assert_eq!(next.len(), previous.len());
        assert!(next
            .iter()
            .all(|r| r.observation.as_ref().unwrap().state == ObservationState::Retained));
        assert!(reconcile(next, vec![], true, Utc::now()).is_empty());
    }
    #[test]
    fn review_receipt_authority_survives_restart_lag_and_expires_to_qualification() {
        use crate::github::{
            model::{ReviewState, ReviewerVerdict},
            mutate::SubmittedReview,
        };
        let now = Utc::now();
        let mut effect = ConfirmedReview {
            head_oid: "head".into(),
            review: ReviewState::Approved,
            confirmed_at: now,
            unresolved: false,
            confirmed_by_read: false,
            receipt: Some(SubmittedReview {
                review_id: "review".into(),
                state: "APPROVED".into(),
                actor: "fixture".into(),
                commit_oid: "head".into(),
                submitted_at: Some(now),
                pr_id: "pr".into(),
                repo: "fixture/project".into(),
                number: 1,
            }),
        };
        effect = serde_json::from_str(&serde_json::to_string(&effect).unwrap()).unwrap();
        let mut read = ReviewerVerdict {
            author: "fixture".into(),
            state: "DISMISSED".into(),
            ..Default::default()
        };
        assert!(!review_effect_resolved(&effect, "head", &[read.clone()]));
        read.id = Some("older-review".into());
        read.submitted_at = Some(now - chrono::Duration::seconds(1));
        assert!(!review_effect_resolved(&effect, "head", &[read.clone()]));
        let mut newer_effect = effect.clone();
        newer_effect.receipt.as_mut().unwrap().review_id = "newer-ack".into();
        newer_effect.receipt.as_mut().unwrap().submitted_at =
            Some(now + chrono::Duration::seconds(2));
        assert!(
            !review_effect_resolved(&newer_effect, "head", &[read.clone()]),
            "old readback cannot clear a newer acknowledgement"
        );
        read.id = Some("review".into());
        assert!(review_effect_resolved(&effect, "head", &[read.clone()]));
        read.id = Some("newer-review".into());
        read.submitted_at = Some(now + chrono::Duration::seconds(1));
        assert!(review_effect_resolved(&effect, "head", &[read]));
        assert!(!review_effect_resolved(&effect, "", &[]));
        assert!(review_effect_resolved(&effect, "new-head", &[]));
        qualify_review_effect(&mut effect, now + chrono::Duration::minutes(14));
        assert!(!effect.unresolved);
        qualify_review_effect(&mut effect, now + chrono::Duration::minutes(15));
        assert!(effect.unresolved);
        assert!(!review_effect_resolved(&effect, "head", &[]));
    }
    #[test]
    fn converged_own_review_survives_partial_omission_without_claiming_aggregate_approval() {
        use crate::github::{
            model::{ReviewState, ReviewerVerdict},
            mutate::SubmittedReview,
        };
        let now = Utc::now();
        let mut row = rows().remove(0);
        row.head_oid = "head".into();
        row.review = ReviewState::ReviewRequired;
        let effect = ConfirmedReview {
            head_oid: "head".into(),
            review: ReviewState::Approved,
            confirmed_at: now,
            unresolved: false,
            confirmed_by_read: false,
            receipt: Some(SubmittedReview {
                review_id: "review".into(),
                state: "APPROVED".into(),
                actor: "fixture".into(),
                commit_oid: "head".into(),
                submitted_at: Some(now),
                pr_id: row.id.clone(),
                repo: row.repo.clone(),
                number: row.number,
            }),
        };
        apply_confirmed_review(&mut row, &effect);
        let mut fresh = row.clone();
        fresh.observation.as_mut().unwrap().confirmed_review = None;
        fresh.latest_reviews = vec![ReviewerVerdict {
            author: "fixture".into(),
            state: "APPROVED".into(),
            id: Some("review".into()),
            commit_oid: Some("head".into()),
            submitted_at: Some(now),
        }];
        let converged = reconcile(vec![row], vec![fresh], false, now);
        assert!(
            converged[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .as_ref()
                .unwrap()
                .confirmed_by_read
        );
        assert_eq!(converged[0].review, ReviewState::ReviewRequired);
        let retained = reconcile(converged, vec![], false, now + chrono::Duration::hours(1));
        let own = retained[0]
            .observation
            .as_ref()
            .unwrap()
            .confirmed_review
            .as_ref()
            .unwrap();
        assert!(own.confirmed_by_read);
        assert!(!own.unresolved);
        for new_head in [false, true] {
            let mut fresh = retained[0].clone();
            fresh.observation.as_mut().unwrap().confirmed_review = None;
            if new_head {
                fresh.head_oid = "new-head".into();
            } else {
                fresh.latest_reviews[0].state = "DISMISSED".into();
            }
            let retired = reconcile(retained.clone(), vec![fresh], false, now);
            assert!(retired[0]
                .observation
                .as_ref()
                .unwrap()
                .confirmed_review
                .is_none());
        }
    }
}
