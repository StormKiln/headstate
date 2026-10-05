//! Durable per-field evidence from targeted reads and verified writes. List
//! publication time is not provider ordering. Equal-version list rows cannot
//! erase a targeted fact; a later targeted read may settle that ambiguity.
use super::{CachedList, StoreError};
use crate::{
    github::model::{PullRequest, ReviewState},
    inventory::ReadinessField,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

// Only candidate identities are loaded, at most four field facts per row.
// Exact terminal barriers have no lifetime expiry: their compact disk records
// grow with completed identities, while hot memory follows the current page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    State(String),
    Draft(bool),
    Queue(bool),
    Review(ReviewState),
}
impl Value {
    fn field(&self) -> Option<ReadinessField> {
        match self {
            Self::State(_) => None,
            Self::Draft(_) => Some(ReadinessField::Draft),
            Self::Queue(_) => Some(ReadinessField::Queue),
            Self::Review(_) => Some(ReadinessField::Review),
        }
    }
    fn same_field(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadAtStart {
    pub id: String,
    pub head_oid: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub session: String,
    pub sequence: u64,
    #[serde(default)]
    pub heads_at_start: [Option<HeadAtStart>; 2],
    #[serde(default)]
    pub receipts_at_start: [Option<u64>; 2],
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    #[serde(default)]
    pub operation: Option<Operation>,
    pub head_oid: String,
    pub updated_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub value: Value,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub facts: Vec<Fact>,
}
impl Observation {
    pub fn from_node(node: &serde_json::Value, observed_at: DateTime<Utc>) -> Option<Self> {
        let id = node["id"].as_str().filter(|s| !s.is_empty())?.to_owned();
        let head = node["headRefOid"].as_str().filter(|s| !s.is_empty())?;
        let updated_at = node["updatedAt"].as_str().and_then(|s| s.parse().ok());
        let mut values = Vec::new();
        if let Some(state @ ("OPEN" | "CLOSED" | "MERGED")) = node["state"].as_str() {
            values.push(Value::State(state.to_lowercase()));
        }
        if let Some(draft) = node["isDraft"].as_bool() {
            values.push(Value::Draft(draft));
        }
        if let Some(queue) = node["isInMergeQueue"].as_bool() {
            // True alone can describe a rejected queue entry; require its state.
            if !queue || node["mergeQueueEntry"]["state"].is_string() {
                values.push(Value::Queue(
                    queue
                        && !matches!(
                            node["mergeQueueEntry"]["state"].as_str(),
                            Some("UNMERGEABLE" | "LOCKED")
                        ),
                ));
            }
        }
        let review = match node.get("reviewDecision") {
            Some(serde_json::Value::Null) => Some(ReviewState::None),
            Some(v) => match v.as_str() {
                Some("APPROVED") => Some(ReviewState::Approved),
                Some("CHANGES_REQUESTED") => Some(ReviewState::ChangesRequested),
                Some("REVIEW_REQUIRED") => Some(ReviewState::ReviewRequired),
                _ => None,
            },
            None => None,
        };
        if let Some(review) = review {
            values.push(Value::Review(review));
        }
        Some(Self {
            id,
            facts: values
                .into_iter()
                .map(|value| Fact {
                    operation: None,
                    head_oid: head.into(),
                    updated_at,
                    observed_at,
                    value,
                })
                .collect(),
        })
    }
}

pub(crate) fn load(
    conn: &Connection,
    list: CachedList,
    owner: &str,
    repo: &str,
    number: u64,
    id: &str,
) -> Result<Option<Observation>, StoreError> {
    let payload: Option<String> = conn.query_row("SELECT payload FROM github_pr_facts WHERE list=?1 AND owner=?2 AND repo=?3 AND number=?4 AND node_id=?5", params![list.id(),owner.to_lowercase(),repo.to_lowercase(),number.to_string(),id], |r| r.get(0)).optional()?;
    payload
        .map(|p| serde_json::from_str(&p).map_err(StoreError::from))
        .transpose()
}
pub fn contains(
    conn: &Connection,
    list: CachedList,
    owner: &str,
    repo: &str,
    number: u64,
    id: &str,
) -> Result<bool, StoreError> {
    Ok(load(conn, list, owner, repo, number, id)?.is_some())
}
/// Caller owns the list publication gate and transaction. Generation validation
/// happens before this call, so a delayed detail cannot replace a later action.
pub fn accept(
    conn: &Connection,
    list: CachedList,
    owner: &str,
    repo: &str,
    number: u64,
    incoming: &Observation,
) -> Result<bool, StoreError> {
    // Ownership retirement is a single indexed check, not a journal scan on
    // every fact. Old credentials cannot reach this function past the gate.
    let previous_owner: Option<String> = conn
        .query_row(
            "SELECT owner FROM github_pr_fact_owner WHERE list=?1",
            [list.id()],
            |r| r.get(0),
        )
        .optional()?;
    let owner_key = owner.to_lowercase();
    if previous_owner.as_deref() != Some(owner_key.as_str()) {
        conn.execute(
            "DELETE FROM github_pr_facts WHERE list=?1 AND owner<>?2",
            params![list.id(), owner_key],
        )?;
        conn.execute("INSERT INTO github_pr_fact_owner(list,owner) VALUES(?1,?2) ON CONFLICT(list) DO UPDATE SET owner=excluded.owner",params![list.id(),owner_key])?;
    }
    let old = load(conn, list, owner, repo, number, &incoming.id)?;
    let mut next = old.clone().unwrap_or(Observation {
        id: incoming.id.clone(),
        facts: vec![],
    });
    for fact in &incoming.facts {
        // A partial newer-head acknowledgment also fences older requests whose
        // fields were never journaled. Per-field ordering alone cannot do that.
        if next.facts.iter().any(|previous| previous.head_oid != fact.head_oid &&
            (matches!((&fact.operation, &previous.operation), (Some(a), Some(b)) if a.session == b.session && a.sequence < b.sequence)
             || fact.observed_at < previous.observed_at)) {
            continue;
        }
        if let Some(previous) = next
            .facts
            .iter_mut()
            .find(|p| p.value.same_field(&fact.value))
        {
            if matches!((&fact.operation,&previous.operation), (Some(a),Some(b)) if a.session == b.session && a.sequence < b.sequence)
            {
                continue;
            }
            if fact.observed_at < previous.observed_at
                || matches!((fact.updated_at,previous.updated_at), (Some(a),Some(b)) if a < b)
                || (fact.updated_at.is_none() && previous.updated_at.is_some())
            {
                continue;
            }
            *previous = fact.clone();
        } else {
            next.facts.push(fact.clone());
        }
    }
    // A terminal fact replaces the active fields with one compact barrier.
    // Reopening requires positive newer evidence, never a time-based eviction.
    if next
        .facts
        .iter()
        .any(|f| matches!(&f.value, Value::State(s) if s == "closed" || s == "merged"))
    {
        next.facts.retain(|f| matches!(f.value, Value::State(_)));
    }
    if old.as_ref() == Some(&next) || next.facts.is_empty() {
        return Ok(false);
    }
    conn.execute("INSERT INTO github_pr_facts(list,owner,repo,number,node_id,payload) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(list,owner,repo,number,node_id) DO UPDATE SET payload=excluded.payload", params![list.id(),owner.to_lowercase(),repo.to_lowercase(),number.to_string(),next.id,serde_json::to_string(&next)?])?;
    Ok(true)
}
/// Applies evidence without stamping the row as freshly fetched. Fields have
/// their own original times in the journal; whole-row freshness stays unchanged.
pub fn apply(
    conn: &Connection,
    list: CachedList,
    owner: &str,
    rows: &mut Vec<PullRequest>,
) -> Result<(), StoreError> {
    apply_inner(conn, list, owner, rows, None)
}

/// A just-accepted targeted field resolves its own qualifier, not list age or
/// unrelated fields. Replayed conflicting evidence remains explicitly retained.
pub fn apply_targeted(
    conn: &Connection,
    list: CachedList,
    owner: &str,
    rows: &mut Vec<PullRequest>,
    targeted: &Observation,
) -> Result<(), StoreError> {
    apply_inner(conn, list, owner, rows, Some(targeted))
}
fn apply_inner(
    conn: &Connection,
    list: CachedList,
    owner: &str,
    rows: &mut Vec<PullRequest>,
    targeted: Option<&Observation>,
) -> Result<(), StoreError> {
    let mut keep = Vec::with_capacity(rows.len());
    for mut row in rows.drain(..) {
        let Some(mut evidence) = load(conn, list, owner, &row.repo, row.number, &row.id)? else {
            keep.push(row);
            continue;
        };
        // A targeted read can positively advance the head before the next
        // inventory page. Keep other head-dependent facts explicitly unknown.
        if let Some(head) = evidence
            .facts
            .iter()
            .filter(|f| {
                f.updated_at.is_some_and(|t| {
                    t > row.updated_at || (t == row.updated_at && f.operation.is_some())
                })
            })
            .max_by_key(|f| {
                (
                    f.updated_at,
                    f.observed_at,
                    f.operation.as_ref().map(|o| o.sequence),
                )
            })
            .map(|f| &f.head_oid)
        {
            if row.head_oid != *head {
                row.head_oid = head.clone();
                let observation = row
                    .observation
                    .get_or_insert(crate::inventory::RowObservation {
                        state: crate::inventory::ObservationState::Retained,
                        last_observed_at: None,
                        unknown_fields: vec![],
                        retained_fields: vec![],
                        detail_fields: vec![],
                        ready_at_state: None,
                        confirmed_review: None,
                    });
                observation.confirmed_review = None;
                for field in [
                    ReadinessField::Ci,
                    ReadinessField::Merge,
                    ReadinessField::Review,
                    ReadinessField::Queue,
                ] {
                    if !observation.unknown_fields.contains(&field) {
                        observation.unknown_fields.push(field);
                    }
                }
                observation
                    .unknown_fields
                    .retain(|f| *f != ReadinessField::Head);
            }
        }
        let mut terminal = false;
        evidence.facts.retain_mut(|fact| {
            let observed = row.observation.as_ref().is_some_and(|o| {
                o.state == crate::inventory::ObservationState::Observed
                    && fact.value.field().is_none_or(|field| {
                        !o.unknown_fields.contains(&field) && !o.retained_fields.contains(&field)
                    })
            });
            // Retain the new version floor as well: deleting it would allow the
            // next lagging page to resurrect the fact it just superseded.
            if observed
                && fact
                    .updated_at
                    .is_some_and(|version| row.updated_at > version)
            {
                fact.value = match fact.value {
                    Value::State(_) => Value::State("open".into()),
                    Value::Draft(_) => Value::Draft(row.is_draft),
                    Value::Queue(_) => Value::Queue(row.in_merge_queue),
                    Value::Review(_) => Value::Review(row.review),
                };
                fact.head_oid = row.head_oid.clone();
                fact.updated_at = Some(row.updated_at);
                fact.operation = None;
                if let Some(time) = row.observation.as_ref().and_then(|o| o.last_observed_at) {
                    fact.observed_at = time;
                }
                return true;
            }
            if row.head_oid != fact.head_oid {
                if let Value::State(state) = &fact.value {
                    terminal = state == "closed" || state == "merged";
                } else if let (Some(field), Some(observation)) =
                    (fact.value.field(), row.observation.as_mut())
                {
                    if !observation.unknown_fields.contains(&field) {
                        observation.unknown_fields.push(field);
                    }
                }
                return true;
            }
            let targeted_now = targeted
                .is_some_and(|incoming| incoming.id == row.id && incoming.facts.contains(fact));
            let agrees_with_observed = observed
                && match fact.value {
                    Value::State(ref state) => state == "open",
                    Value::Draft(value) => row.is_draft == value,
                    Value::Queue(value) => row.in_merge_queue == value,
                    Value::Review(value) => row.review == value,
                };
            match &fact.value {
                Value::State(state) => terminal = state == "closed" || state == "merged",
                Value::Draft(value) => row.is_draft = *value,
                Value::Queue(value) => row.in_merge_queue = *value,
                Value::Review(value) => row.review = *value,
            }
            if let (Some(field), Some(observation)) = (fact.value.field(), row.observation.as_mut())
            {
                observation.unknown_fields.retain(|f| *f != field);
                if targeted_now || agrees_with_observed {
                    observation.retained_fields.retain(|f| *f != field);
                } else if !observation.retained_fields.contains(&field) {
                    observation.retained_fields.push(field);
                }
            }
            true
        });
        if evidence.facts.is_empty() {
            conn.execute("DELETE FROM github_pr_facts WHERE list=?1 AND owner=?2 AND repo=?3 AND number=?4 AND node_id=?5", params![list.id(),owner.to_lowercase(),row.repo.to_lowercase(),row.number.to_string(),row.id])?;
        } else {
            conn.execute("UPDATE github_pr_facts SET payload=?1 WHERE list=?2 AND owner=?3 AND repo=?4 AND number=?5 AND node_id=?6", params![serde_json::to_string(&evidence)?,list.id(),owner.to_lowercase(),row.repo.to_lowercase(),row.number.to_string(),row.id])?;
        }
        if !terminal {
            keep.push(row);
        }
    }
    *rows = keep;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }
    fn raw() -> serde_json::Value {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/search.json")).unwrap();
        let mut row = data["authored"]["nodes"][0].clone();
        row["id"] = json!("PR_synthetic");
        row["state"] = json!("OPEN");
        row["headRefOid"] = json!("head-a");
        row["updatedAt"] = json!("2026-10-01T10:00:00Z");
        row["isInMergeQueue"] = json!(false);
        row["isDraft"] = json!(false);
        row
    }
    fn rows(node: serde_json::Value) -> Vec<PullRequest> {
        crate::github::map::map_list(&json!({"authored":{"nodes":[node]}}), "authored")
    }
    fn save(conn: &Connection, row: &PullRequest, node: &serde_json::Value) {
        accept(
            conn,
            CachedList::Reviewing,
            "viewer",
            &row.repo,
            row.number,
            &Observation::from_node(node, Utc::now()).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn held_and_equal_version_lists_cannot_undo_queue_but_newer_can() {
        let conn = db();
        let original = rows(raw());
        let mut node = raw();
        node["isInMergeQueue"] = json!(true);
        node["mergeQueueEntry"] = json!({"state":"QUEUED"});
        save(&conn, &original[0], &node);
        for _ in 0..2 {
            let mut stale = original.clone();
            apply(&conn, CachedList::Reviewing, "viewer", &mut stale).unwrap();
            assert!(stale[0].in_merge_queue);
        }
        let mut newer = raw();
        newer["updatedAt"] = json!("2026-10-01T11:00:00Z");
        let mut incoming = rows(newer);
        apply(&conn, CachedList::Reviewing, "viewer", &mut incoming).unwrap();
        assert!(!incoming[0].in_merge_queue);
        let mut lagging = original;
        lagging[0].in_merge_queue = true;
        apply(&conn, CachedList::Reviewing, "viewer", &mut lagging).unwrap();
        assert!(
            !lagging[0].in_merge_queue,
            "newer provider floor also fences later lagging pages"
        );
    }
    #[test]
    fn partial_detail_preserves_other_field_evidence_and_original_time() {
        let conn = db();
        let original = rows(raw());
        let mut queued = raw();
        queued["isInMergeQueue"] = json!(true);
        queued["mergeQueueEntry"] = json!({"state":"QUEUED"});
        save(&conn, &original[0], &queued);
        let before = load(
            &conn,
            CachedList::Reviewing,
            "viewer",
            &original[0].repo,
            original[0].number,
            &original[0].id,
        )
        .unwrap()
        .unwrap();
        let partial = json!({"id":"PR_synthetic","headRefOid":"head-a","updatedAt":"2026-10-01T10:00:00Z","isDraft":true});
        save(&conn, &original[0], &partial);
        let after = load(
            &conn,
            CachedList::Reviewing,
            "viewer",
            &original[0].repo,
            original[0].number,
            &original[0].id,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            before
                .facts
                .iter()
                .find(|f| matches!(f.value, Value::Queue(_))),
            after
                .facts
                .iter()
                .find(|f| matches!(f.value, Value::Queue(_)))
        );
        let mut incoming = original;
        apply(&conn, CachedList::Reviewing, "viewer", &mut incoming).unwrap();
        assert!(incoming[0].is_draft && incoming[0].in_merge_queue);
    }
    #[test]
    fn terminal_barrier_survives_lagging_other_head_and_only_newer_reopen_wins() {
        let conn = db();
        let original = rows(raw());
        let mut closed = raw();
        closed["state"] = json!("CLOSED");
        save(&conn, &original[0], &closed);
        let mut oldhead = raw();
        oldhead["headRefOid"] = json!("head-before-a");
        let mut stale = rows(oldhead);
        apply(&conn, CachedList::Reviewing, "viewer", &mut stale).unwrap();
        assert!(stale.is_empty());
        let mut reopened = raw();
        reopened["updatedAt"] = json!("2026-10-01T11:00:00Z");
        let mut current = rows(reopened);
        apply(&conn, CachedList::Reviewing, "viewer", &mut current).unwrap();
        assert_eq!(current.len(), 1);
    }
    #[test]
    fn older_or_unversioned_detail_does_not_replace_versioned_fact() {
        let conn = db();
        let original = rows(raw());
        let mut queued = raw();
        queued["isInMergeQueue"] = json!(true);
        queued["mergeQueueEntry"] = json!({"state":"QUEUED"});
        save(&conn, &original[0], &queued);
        for timestamp in [serde_json::Value::Null, json!("2026-09-01T00:00:00Z")] {
            let mut old = raw();
            old["updatedAt"] = timestamp;
            save(&conn, &original[0], &old);
        }
        let mut stale = original;
        apply(&conn, CachedList::Reviewing, "viewer", &mut stale).unwrap();
        assert!(stale[0].in_merge_queue);
    }
    #[test]
    fn later_equal_version_targeted_detail_and_new_head_can_supersede() {
        let conn = db();
        let original = rows(raw());
        let mut queued = raw();
        queued["isInMergeQueue"] = json!(true);
        queued["mergeQueueEntry"] = json!({"state":"QUEUED"});
        save(&conn, &original[0], &queued);
        save(&conn, &original[0], &raw());
        let mut current = original;
        apply(&conn, CachedList::Reviewing, "viewer", &mut current).unwrap();
        assert!(!current[0].in_merge_queue);
        let mut newhead = raw();
        newhead["headRefOid"] = json!("head-b");
        newhead["updatedAt"] = json!("2026-10-01T12:00:00Z");
        let mut current = rows(newhead);
        apply(&conn, CachedList::Reviewing, "viewer", &mut current).unwrap();
        assert_eq!(current[0].head_oid, "head-b");
        assert!(!current[0].in_merge_queue);
    }
    #[test]
    fn facts_never_cross_owner_or_node_identity() {
        let conn = db();
        let original = rows(raw());
        let mut closed = raw();
        closed["state"] = json!("MERGED");
        save(&conn, &original[0], &closed);
        let mut otherowner = original;
        apply(&conn, CachedList::Reviewing, "other", &mut otherowner).unwrap();
        assert_eq!(otherowner.len(), 1);
        let mut othernode = raw();
        othernode["id"] = json!("PR_other");
        let mut other = rows(othernode);
        apply(&conn, CachedList::Reviewing, "viewer", &mut other).unwrap();
        assert_eq!(other.len(), 1);
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;
    use serde_json::json;
    fn observation(sequence: u64, queued: bool) -> Observation {
        let mut o=Observation::from_node(&json!({"id":"PR_order","headRefOid":"head-a","updatedAt":"2026-10-01T00:00:00Z","isInMergeQueue":queued,"mergeQueueEntry":{"state":"QUEUED"}}),Utc::now()).unwrap();
        for fact in &mut o.facts {
            fact.operation = Some(Operation {
                session: "process-a".into(),
                sequence,
                heads_at_start: [None, None],
                receipts_at_start: [None, None],
            });
        }
        o
    }
    #[test]
    fn earlier_started_equal_version_completion_cannot_undo_later_operation() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        accept(
            &conn,
            CachedList::Reviewing,
            "viewer",
            "synthetic/repo",
            1,
            &observation(2, true),
        )
        .unwrap();
        assert!(!accept(
            &conn,
            CachedList::Reviewing,
            "viewer",
            "synthetic/repo",
            1,
            &observation(1, false)
        )
        .unwrap());
        assert!(matches!(
            load(
                &conn,
                CachedList::Reviewing,
                "viewer",
                "synthetic/repo",
                1,
                "PR_order"
            )
            .unwrap()
            .unwrap()
            .facts[0]
                .value,
            Value::Queue(true)
        ));
        assert!(accept(
            &conn,
            CachedList::Reviewing,
            "viewer",
            "synthetic/repo",
            1,
            &observation(3, false)
        )
        .unwrap());
    }
    #[test]
    fn lifetime_terminal_ledger_does_not_exhaust_hot_fact_admission() {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        for number in 1..=4100 {
            let mut proof = observation(number, true);
            proof.id = format!("PR_{number}");
            proof.facts[0].value = Value::State("merged".into());
            accept(
                &conn,
                CachedList::Reviewing,
                "viewer",
                "synthetic/repo",
                number,
                &proof,
            )
            .unwrap();
        }
        let proof = observation(5000, true);
        assert!(accept(
            &conn,
            CachedList::Reviewing,
            "viewer",
            "synthetic/repo",
            5000,
            &proof
        )
        .unwrap());
        let first = load(
            &conn,
            CachedList::Reviewing,
            "viewer",
            "synthetic/repo",
            1,
            "PR_1",
        )
        .unwrap()
        .unwrap();
        assert_eq!(first.facts.len(), 1);
        assert!(matches!(&first.facts[0].value,Value::State(s) if s=="merged"));
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/search.json")).unwrap();
        let mut stale = crate::github::map::map_list(&fixture, "authored");
        stale.truncate(1);
        stale[0].id = "PR_1".into();
        stale[0].repo = "synthetic/repo".into();
        stale[0].number = 1;
        apply(&conn, CachedList::Reviewing, "viewer", &mut stale).unwrap();
        assert!(
            stale.is_empty(),
            "old terminal identity remains fenced after 4100 completions"
        );
        assert!(load(
            &conn,
            CachedList::Reviewing,
            "viewer",
            "synthetic/repo",
            5000,
            "PR_order"
        )
        .unwrap()
        .is_some());
        // Positive owner retirement bounds retained accounts, not lifetime PRs.
        accept(
            &conn,
            CachedList::Reviewing,
            "replacement",
            "synthetic/repo",
            1,
            &proof,
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM github_pr_facts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
