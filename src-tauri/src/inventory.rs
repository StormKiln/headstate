//! Qualified observations shared by provider inventories.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessField {
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowObservation {
    pub state: ObservationState,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub unknown_fields: Vec<ReadinessField>,
    pub retained_fields: Vec<ReadinessField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_review: Option<ConfirmedReview>,
}

pub trait InventoryRow: Clone {
    fn identity(&self) -> crate::identity::PrIdentity;
    fn observation(&self) -> &Option<RowObservation>;
    fn observation_mut(&mut self) -> &mut Option<RowObservation>;
    fn head(&self) -> Option<&str>;
    fn copy_field(&mut self, old: &Self, field: ReadinessField);
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
            apply_confirmed_review(self, effect);
        }
    }
    fn copy_field(&mut self, old: &Self, field: ReadinessField) {
        match field {
            ReadinessField::Draft => self.is_draft = old.is_draft,
            ReadinessField::Ci => self.ci = old.ci,
            ReadinessField::Merge => self.merge = old.merge,
            ReadinessField::Review => self.review = old.review,
            ReadinessField::Queue => self.in_merge_queue = old.in_merge_queue,
        }
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
                    observation.confirmed_review = Some(effect.clone());
                }
            }
        }
    }
    fn copy_field(&mut self, old: &Self, field: ReadinessField) {
        match field {
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
    let mut prior: std::collections::HashMap<_, _> =
        previous.into_iter().map(|r| (r.identity(), r)).collect();
    let mut output = Vec::with_capacity(incoming.len() + prior.len());
    for mut row in incoming {
        let old = prior.remove(&row.identity());
        let mut observation = row.observation().clone().unwrap_or(RowObservation {
            state: ObservationState::Observed,
            last_observed_at: Some(now),
            unknown_fields: vec![],
            retained_fields: vec![],
            confirmed_review: None,
        });
        observation.state = ObservationState::Observed;
        observation.last_observed_at = Some(now);
        observation.retained_fields.clear();
        if let Some(old) = old
            .as_ref()
            .filter(|old| row.head().is_some() && row.head() == old.head())
        {
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
                confirmed_review: None,
            });
            observation.state = ObservationState::Retained;
            output.push(row);
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
    RowObservation {
        state: ObservationState::Observed,
        last_observed_at: None,
        unknown_fields: unknown,
        retained_fields: vec![],
        confirmed_review: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmedReview {
    pub head_oid: String,
    pub review: crate::github::model::ReviewState,
    pub confirmed_at: DateTime<Utc>,
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
            confirmed_review: None,
        });
        observation.confirmed_review = Some(ConfirmedReview {
            head_oid: head.into(),
            review,
            confirmed_at: Utc::now(),
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
    fn confirmed_review_survives_omission_restart_and_lag_but_not_new_head() {
        let mut old = rows().remove(0);
        old.head_oid = "head-a".into();
        old.review = crate::github::model::ReviewState::ReviewRequired;
        let effect = ConfirmedReview {
            head_oid: old.head_oid.clone(),
            review: crate::github::model::ReviewState::Approved,
            confirmed_at: Utc::now(),
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
}
