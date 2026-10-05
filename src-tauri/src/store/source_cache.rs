//! Source-scoped snapshots. A cache miss, unreadable cache and measured empty
//! queue are different answers. Coverage survives relaunch and partial results
//! replace only their own source/list, never another provider's usable data.
use super::{cache, CachedList, StoreError};
use crate::{
    github::model::PullRequest,
    gitlab::queues::MergeRequest,
    identity::{Provider, Source},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Partial {
        total: Option<u64>,
    },
    /// Old caches did not record coverage. Do not invent completeness.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SnapshotData {
    Missing,
    Withheld {
        reason: String,
    },
    Unreadable,
    Available {
        prs: Vec<PullRequest>,
        fetched_at: String,
        stale_secs: Option<i64>,
        coverage: Coverage,
    },
    GitLabAvailable {
        mrs: Vec<MergeRequest>,
        fetched_at: String,
        stale_secs: Option<i64>,
        coverage: Coverage,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub source: Source,
    pub list: CachedList,
    pub data: SnapshotData,
}

/// Only qualified snapshots cross command boundaries. Binding bytes never do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnedSourceSnapshot {
    pub session: Option<String>,
    #[serde(flatten)]
    pub snapshot: SourceSnapshot,
    pub ownership: Ownership,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Ownership {
    LiveVerified { owner: String },
    CredentialBound { owner: String },
    Unverified,
    DifferentAccount,
}

pub(crate) fn save_verified_binding(
    conn: &Connection,
    binding: &str,
    owner: &str,
) -> Result<(), StoreError> {
    if owner.is_empty() {
        return Ok(());
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT owner FROM snapshot_credentials WHERE binding=?1",
            [binding],
            |r| r.get(0),
        )
        .optional()?;
    if existing.as_deref() == Some(owner) {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO snapshot_credentials(binding,owner) VALUES (?1,?2)
        ON CONFLICT(binding) DO UPDATE SET owner=excluded.owner",
        params![binding, owner],
    )?;
    Ok(())
}

pub(crate) fn load_qualified_snapshot(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    viewer: Option<&str>,
    binding: Option<&str>,
) -> Result<OwnedSourceSnapshot, StoreError> {
    // A read transaction keeps the binding, owner and decoded payload on the
    // same SQLite snapshot even when another connection publishes a receipt.
    let tx = conn.unchecked_transaction()?;
    let owner: Option<(Option<String>, Option<String>)> = tx.query_row(
        "SELECT s.owner,c.owner FROM snapshot s LEFT JOIN snapshot_credentials c ON c.binding=?4
         WHERE s.provider=?1 AND s.host=?2 AND s.id=?3",
        params![serde_json::to_value(source.provider)?.as_str(), source.host, list.id(), binding],
        |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let mut snapshot = load_source_snapshot(&tx, source, list)?;
    let ownership = match owner {
        Some((Some(owner), bound)) if !owner.is_empty() => {
            if let Some(viewer) = viewer {
                if owner.eq_ignore_ascii_case(viewer) {
                    Ownership::LiveVerified { owner }
                } else {
                    Ownership::DifferentAccount
                }
            } else if bound
                .as_deref()
                .is_some_and(|v| owner.eq_ignore_ascii_case(v))
            {
                Ownership::CredentialBound { owner }
            } else {
                Ownership::Unverified
            }
        }
        _ => Ownership::Unverified,
    };
    if !matches!(
        ownership,
        Ownership::LiveVerified { .. } | Ownership::CredentialBound { .. }
    ) && !matches!(
        snapshot.data,
        SnapshotData::Missing | SnapshotData::Unreadable
    ) {
        snapshot.data = SnapshotData::Withheld { reason: match ownership {
            Ownership::DifferentAccount => "The saved snapshot belongs to a different account. Refresh to load this account.".into(),
            _ => "The saved snapshot's account could not be verified. Connect once to verify this credential.".into(),
        } };
    }
    tx.commit()?;
    Ok(OwnedSourceSnapshot {
        session: None,
        snapshot,
        ownership,
    })
}

pub fn save_source_snapshot(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    prs: &[PullRequest],
    coverage: &Coverage,
) -> Result<(), StoreError> {
    // Reject a mixed/wrong source instead of silently relabelling records.
    if source.provider != Provider::Github || prs.iter().any(|pr| &pr.source != source) {
        return Err(StoreError::SnapshotSourceMismatch);
    }
    save_owned_source_snapshot(conn, source, list, prs, coverage, None)
}

pub fn save_gitlab_snapshot(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    mrs: &[MergeRequest],
    coverage: &Coverage,
) -> Result<(), StoreError> {
    if source.provider != Provider::Gitlab || mrs.iter().any(|mr| &mr.source != source) {
        return Err(StoreError::SnapshotSourceMismatch);
    }
    save_owned_gitlab_snapshot(conn, source, list, mrs, coverage, None)
}

pub fn save_owned_source_snapshot(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    prs: &[PullRequest],
    coverage: &Coverage,
    owner: Option<&str>,
) -> Result<(), StoreError> {
    if source.provider != Provider::Github || prs.iter().any(|pr| &pr.source != source) {
        return Err(StoreError::SnapshotSourceMismatch);
    }
    save_payload(conn, source, list, prs, coverage, owner)
}
/// Requalifying cached rows after a failed read must not make their receipt younger.
/// The caller's checkpoint transaction covers this write as well.
pub fn save_owned_source_failure(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    prs: &[PullRequest],
    coverage: &Coverage,
    owner: &str,
) -> Result<(), StoreError> {
    let provider = serde_json::to_value(source.provider)?;
    let old_time: Option<String> = conn
        .query_row(
            "SELECT fetched_at FROM snapshot WHERE provider=?1 AND host=?2 AND id=?3 AND owner=?4",
            params![provider.as_str(), source.host, list.id(), owner],
            |r| r.get(0),
        )
        .optional()?;
    let Some(old_time) = old_time else {
        return Ok(());
    };
    save_owned_source_snapshot(conn, source, list, prs, coverage, Some(owner))?;
    conn.execute(
        "UPDATE snapshot SET fetched_at=?1 WHERE provider=?2 AND host=?3 AND id=?4 AND owner=?5",
        params![old_time, provider.as_str(), source.host, list.id(), owner],
    )?;
    Ok(())
}

pub fn save_owned_gitlab_snapshot(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    mrs: &[MergeRequest],
    coverage: &Coverage,
    owner: Option<&str>,
) -> Result<(), StoreError> {
    if source.provider != Provider::Gitlab
        || mrs
            .iter()
            .any(|mr| &mr.source != source || owner.is_some() && mr.viewer.as_deref() != owner)
    {
        return Err(StoreError::SnapshotSourceMismatch);
    }
    save_payload(conn, source, list, mrs, coverage, owner)
}
/// Internal ownership; deliberately excluded from remote snapshot payloads.
pub fn snapshot_owner(
    conn: &Connection,
    source: &Source,
    list: CachedList,
) -> Result<Option<String>, StoreError> {
    let provider = serde_json::to_value(source.provider)?;
    Ok(conn
        .query_row(
            "SELECT owner FROM snapshot WHERE provider = ?1 AND host = ?2 AND id = ?3",
            params![provider.as_str(), source.host, list.id()],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

fn save_payload<T: Serialize>(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    rows: &[T],
    coverage: &Coverage,
    owner: Option<&str>,
) -> Result<(), StoreError> {
    let provider = serde_json::to_value(source.provider)?;
    conn.execute(
        "INSERT INTO snapshot (provider, host, id, payload, fetched_at, coverage, owner)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, ?6)
         ON CONFLICT(provider, host, id) DO UPDATE SET
            payload = excluded.payload, fetched_at = excluded.fetched_at,
            coverage = excluded.coverage, owner = excluded.owner",
        params![
            provider.as_str(),
            source.host,
            list.id(),
            serde_json::to_string(rows)?,
            serde_json::to_string(coverage)?,
            owner
        ],
    )?;
    Ok(())
}

pub fn load_source_snapshot(
    conn: &Connection,
    source: &Source,
    list: CachedList,
) -> Result<SourceSnapshot, StoreError> {
    let provider = serde_json::to_value(source.provider)?;
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT payload, fetched_at, coverage FROM snapshot
         WHERE provider = ?1 AND host = ?2 AND id = ?3",
            params![provider.as_str(), source.host, list.id()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let data = match row {
        None => SnapshotData::Missing,
        Some((payload, at, coverage)) => {
            let Ok(coverage) = serde_json::from_str::<Coverage>(&coverage) else {
                return Ok(SourceSnapshot {
                    source: source.clone(),
                    list,
                    data: SnapshotData::Unreadable,
                });
            };
            let now = chrono::Utc::now();
            let stale_secs = (!cache::is_fresh(&at, now)).then(|| {
                cache::age_secs(&at, now)
                    .unwrap_or(cache::MAX_SNAPSHOT_AGE_SECS)
                    .max(1)
            });
            match source.provider {
                Provider::Github => match serde_json::from_str::<Vec<PullRequest>>(&payload) {
                    Ok(mut prs) if prs.iter().all(|pr| &pr.source == source) => {
                        for pr in &mut prs {
                            if let Some(effect) = pr
                                .observation
                                .as_mut()
                                .and_then(|o| o.confirmed_review.as_mut())
                            {
                                crate::inventory::qualify_review_effect(effect, now);
                            }
                        }
                        SnapshotData::Available {
                            prs,
                            fetched_at: at,
                            stale_secs,
                            coverage,
                        }
                    }
                    _ => SnapshotData::Unreadable,
                },
                Provider::Gitlab => match serde_json::from_str::<Vec<MergeRequest>>(&payload) {
                    Ok(mrs) if mrs.iter().all(|mr| &mr.source == source) => {
                        SnapshotData::GitLabAvailable {
                            mrs,
                            fetched_at: at,
                            stale_secs,
                            coverage,
                        }
                    }
                    _ => SnapshotData::Unreadable,
                },
            }
        }
    };
    Ok(SourceSnapshot {
        source: source.clone(),
        list,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Provider;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }
    fn gitlab(host: &str) -> Source {
        Source {
            provider: Provider::Gitlab,
            host: host.into(),
        }
    }
    fn sample(source: Source) -> PullRequest {
        let mut pr: PullRequest = serde_json::from_value(serde_json::json!({
            "number": 7, "title": "fixture", "url": "https://example.com/mr/7",
            "repo": "group/subgroup/project", "author": "fixture", "is_draft": true,
            "head_ref": "topic", "base_ref": "main", "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z", "ci": "pending", "merge": "checking",
            "review": "review_required", "in_merge_queue": false, "labels": [], "comment_count": 0
        }))
        .unwrap();
        pr.source = source;
        pr
    }
    fn sample_mr(source: Source) -> MergeRequest {
        serde_json::from_value(serde_json::json!({
            "source": source, "id": 17, "number": 7, "title": "fixture",
            "url": "https://gitlab.com/group/subgroup/project/-/merge_requests/7",
            "repo": "group/subgroup/project", "author": "fixture", "is_draft": true,
            "head_ref": "topic", "base_ref": "main",
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "labels": [], "reviewers": [], "assignees": [], "comment_count": 0,
            "detailed_merge_status": "draft_status", "ci": null,
            "review": null, "unresolved_threads": null
        }))
        .unwrap()
    }
    #[test]
    fn qualified_warm_read_does_not_reserve_a_wal_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let reader = crate::store::open_db(&path).unwrap();
        save_owned_source_snapshot(
            &reader,
            &Source::default(),
            CachedList::Authored,
            &[],
            &Coverage::Complete,
            Some("alice"),
        )
        .unwrap();
        save_verified_binding(&reader, "fixture-binding", "alice").unwrap();
        let writer = crate::store::open_db(&path).unwrap();
        writer
            .execute_batch("BEGIN IMMEDIATE; UPDATE snapshot SET owner='bob'")
            .unwrap();
        reader
            .busy_timeout(std::time::Duration::from_millis(1))
            .unwrap();
        save_verified_binding(&reader, "fixture-binding", "alice").unwrap();
        let snapshot = load_qualified_snapshot(
            &reader,
            &Source::default(),
            CachedList::Authored,
            None,
            Some("fixture-binding"),
        )
        .unwrap();
        assert!(
            matches!(snapshot.ownership, Ownership::CredentialBound { owner } if owner == "alice")
        );
        assert!(matches!(
            snapshot.snapshot.data,
            SnapshotData::Available { .. }
        ));
        writer.execute_batch("ROLLBACK").unwrap();
    }
    #[test]
    fn qualified_receipts_keep_hour_boundary_corruption_and_missing_distinct() {
        let conn = db();
        let read = || {
            load_qualified_snapshot(
                &conn,
                &Source::default(),
                CachedList::Authored,
                Some("alice"),
                None,
            )
            .unwrap()
        };
        assert!(matches!(read().snapshot.data, SnapshotData::Missing));
        save_owned_source_snapshot(
            &conn,
            &Source::default(),
            CachedList::Authored,
            &[],
            &Coverage::Complete,
            Some("alice"),
        )
        .unwrap();
        for age in [3599, 3600, 3601, 86400] {
            let at = (chrono::Utc::now() - chrono::Duration::seconds(age))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            conn.execute("UPDATE snapshot SET fetched_at=?1", [&at])
                .unwrap();
            let SnapshotData::Available {
                prs,
                fetched_at,
                stale_secs,
                ..
            } = read().snapshot.data
            else {
                panic!("retained")
            };
            assert!(prs.is_empty());
            assert_eq!(fetched_at, at);
            assert_eq!(stale_secs.is_some(), age > 3600);
        }
        conn.execute("UPDATE snapshot SET payload='broken'", [])
            .unwrap();
        assert!(matches!(read().snapshot.data, SnapshotData::Unreadable));
        conn.execute("UPDATE snapshot SET payload='[]',coverage='broken'", [])
            .unwrap();
        assert!(matches!(read().snapshot.data, SnapshotData::Unreadable));
    }

    #[test]
    fn owned_restart_receipts_distinguish_accounts_empty_and_retained_age() {
        let conn = db();
        let source = Source::default();
        for rows in [vec![], vec![sample(source.clone()); 150]] {
            save_owned_source_snapshot(
                &conn,
                &source,
                CachedList::Authored,
                &rows,
                &Coverage::Complete,
                Some("alice"),
            )
            .unwrap();
            conn.execute(
                "UPDATE snapshot SET fetched_at=datetime('now','-1 day')",
                [],
            )
            .unwrap();
            let read = |viewer, binding| {
                load_qualified_snapshot(&conn, &source, CachedList::Authored, viewer, binding)
                    .unwrap()
            };
            assert!(matches!(
                read(None, None).snapshot.data,
                SnapshotData::Withheld { .. }
            ));
            assert!(matches!(
                read(Some("bob"), None).snapshot.data,
                SnapshotData::Withheld { .. }
            ));
            let SnapshotData::Available {
                prs, stale_secs, ..
            } = read(Some("alice"), None).snapshot.data
            else {
                panic!("same owner retained")
            };
            assert_eq!(prs.len(), rows.len());
            assert!(stale_secs.unwrap() >= 86400);
            save_verified_binding(&conn, "binding-a", "alice").unwrap();
            assert!(matches!(
                read(None, Some("binding-a")).snapshot.data,
                SnapshotData::Available { .. }
            ));
            assert!(matches!(
                read(None, Some("binding-b")).snapshot.data,
                SnapshotData::Withheld { .. }
            ));
            assert!(matches!(
                read(Some("bob"), Some("binding-a")).snapshot.data,
                SnapshotData::Withheld { .. }
            ));
            assert!(!serde_json::to_string(&read(None, Some("binding-a")))
                .unwrap()
                .contains("binding-a"));
        }
    }

    #[test]
    fn verified_gitlab_actions_survive_partial_omission_but_unverified_does_not_patch() {
        use crate::gitlab::actions::{Action, ActionRequest, Outcome, Receipt};
        let conn = db();
        let source = gitlab("gitlab.com");
        let mut row = sample_mr(source.clone());
        row.viewer = Some("fixture".into());
        row.head_oid = Some("head-a".into());
        row.needs_my_review = Some(true);
        let mut request = ActionRequest {
            expected_viewer: row.viewer.clone(),
            identity: row.identity(),
            action: Action::Approve,
            expected_head: row.head_oid.clone(),
            body: None,
            discussion_id: None,
        };
        let mut receipt = Receipt {
            session: None,
            identity: row.identity(),
            action: Action::Approve,
            outcome: Outcome::Unverified,
            message: String::new(),
        };
        let mut rows = vec![row.clone()];
        assert!(!crate::inventory::apply_gitlab_action(
            &mut rows, &request, &receipt
        ));
        assert_eq!(rows[0].needs_my_review, Some(true));
        receipt.outcome = Outcome::Verified;
        assert!(crate::inventory::apply_gitlab_action(
            &mut rows, &request, &receipt
        ));
        assert_eq!(rows[0].needs_my_review, Some(false));
        assert_eq!(rows[0].review, row.review);
        save_owned_gitlab_snapshot(
            &conn,
            &source,
            CachedList::Reviewing,
            &rows,
            &Coverage::Complete,
            Some("fixture"),
        )
        .unwrap();
        let SnapshotData::GitLabAvailable { mrs, .. } =
            load_source_snapshot(&conn, &source, CachedList::Reviewing)
                .unwrap()
                .data
        else {
            panic!("snapshot")
        };
        let retained = crate::inventory::reconcile(mrs, vec![], false, chrono::Utc::now());
        assert_eq!(retained[0].needs_my_review, Some(false));
        request.action = Action::Close;
        receipt.action = Action::Close;
        let mut removed = retained;
        assert!(crate::inventory::apply_gitlab_action(
            &mut removed,
            &request,
            &receipt
        ));
        assert!(
            crate::inventory::reconcile(removed.clone(), vec![], false, chrono::Utc::now())
                .is_empty()
        );
        assert_eq!(
            crate::inventory::reconcile(removed, vec![row.clone()], true, chrono::Utc::now()).len(),
            1
        );
        request.expected_head = Some("head-b".into());
        assert!(!crate::inventory::apply_gitlab_action(
            &mut vec![row.clone()],
            &request,
            &receipt
        ));
        request.expected_head = row.head_oid.clone();
        request.expected_viewer = Some("different-account".into());
        assert!(!crate::inventory::apply_gitlab_action(
            &mut vec![row],
            &request,
            &receipt
        ));
    }

    #[test]
    fn ownership_survives_empty_receipts_and_is_not_public_row_evidence() {
        let conn = db();
        let source = Source::default();
        save_owned_source_snapshot(
            &conn,
            &source,
            CachedList::Reviewing,
            &[],
            &Coverage::Complete,
            Some("first-account"),
        )
        .unwrap();
        assert_eq!(
            snapshot_owner(&conn, &source, CachedList::Reviewing)
                .unwrap()
                .as_deref(),
            Some("first-account")
        );
        let wire = serde_json::to_string(
            &load_source_snapshot(&conn, &source, CachedList::Reviewing).unwrap(),
        )
        .unwrap();
        assert!(!wire.contains("first-account"));
        save_owned_source_snapshot(
            &conn,
            &source,
            CachedList::Reviewing,
            &[],
            &Coverage::Partial { total: None },
            Some("second-account"),
        )
        .unwrap();
        assert_eq!(
            snapshot_owner(&conn, &source, CachedList::Reviewing)
                .unwrap()
                .as_deref(),
            Some("second-account")
        );
        save_source_snapshot(
            &conn,
            &source,
            CachedList::Reviewing,
            &[],
            &Coverage::Unknown,
        )
        .unwrap();
        assert_eq!(
            snapshot_owner(&conn, &source, CachedList::Reviewing).unwrap(),
            None
        );
    }

    #[test]
    fn reconciled_partial_inventory_and_effect_survive_actual_cache_reload() {
        let conn = db();
        let source = Source::default();
        let mut a = sample(source.clone());
        a.head_oid = "head-a".into();
        let effect = crate::inventory::ConfirmedReview {
            head_oid: a.head_oid.clone(),
            review: crate::github::model::ReviewState::Approved,
            confirmed_at: chrono::Utc::now(),
            receipt: None,
            unresolved: false,
            confirmed_by_read: false,
        };
        crate::inventory::apply_confirmed_review(&mut a, &effect);
        let mut b = a.clone();
        b.number += 1;
        save_source_snapshot(
            &conn,
            &source,
            CachedList::Reviewing,
            &[a.clone(), b],
            &Coverage::Complete,
        )
        .unwrap();
        let SnapshotData::Available { prs: previous, .. } =
            load_source_snapshot(&conn, &source, CachedList::Reviewing)
                .unwrap()
                .data
        else {
            panic!("snapshot")
        };
        let rows = crate::inventory::reconcile(previous, vec![a], false, chrono::Utc::now());
        save_source_snapshot(
            &conn,
            &source,
            CachedList::Reviewing,
            &rows,
            &Coverage::Partial { total: Some(2) },
        )
        .unwrap();
        let SnapshotData::Available { prs, coverage, .. } =
            load_source_snapshot(&conn, &source, CachedList::Reviewing)
                .unwrap()
                .data
        else {
            panic!("snapshot")
        };
        assert_eq!(prs, rows);
        assert_eq!(coverage, Coverage::Partial { total: Some(2) });
        assert_eq!(prs.len(), 2);
        assert_eq!(crate::inventory::observed_count(&prs), 1);
        assert_eq!(
            prs[1].observation.as_ref().unwrap().state,
            crate::inventory::ObservationState::Retained
        );
        assert!(prs[1]
            .observation
            .as_ref()
            .unwrap()
            .confirmed_review
            .is_some());
    }

    #[test]
    fn gitlab_snapshot_wire_keeps_absence_and_coverage_distinct() {
        let conn = db();
        let source = gitlab("gitlab.com");
        save_gitlab_snapshot(
            &conn,
            &source,
            CachedList::Reviewing,
            &[sample_mr(source.clone())],
            &Coverage::Partial { total: Some(3) },
        )
        .unwrap();
        let wire = serde_json::to_value(
            load_source_snapshot(&conn, &source, CachedList::Reviewing).unwrap(),
        )
        .unwrap();
        assert_eq!(wire["data"]["state"], "git_lab_available");
        assert_eq!(wire["data"]["coverage"]["partial"]["total"], 3);
        assert!(wire["data"]["mrs"][0]["ci"].is_null());
        assert!(wire["data"]["mrs"][0]["review"].is_null());
    }
    #[test]
    fn overlapping_ids_roundtrip_and_a_mixed_write_cannot_replace_good_data() {
        let conn = db();
        let gh = Source::default();
        let gl = gitlab("gitlab.com");
        save_source_snapshot(
            &conn,
            &gh,
            CachedList::Authored,
            &[sample(gh.clone())],
            &Coverage::Complete,
        )
        .unwrap();
        save_gitlab_snapshot(
            &conn,
            &gl,
            CachedList::Authored,
            &[sample_mr(gl.clone())],
            &Coverage::Complete,
        )
        .unwrap();
        assert!(save_source_snapshot(
            &conn,
            &gh,
            CachedList::Authored,
            &[sample(gl.clone())],
            &Coverage::Complete
        )
        .is_err());
        assert!(
            matches!(load_source_snapshot(&conn, &gh, CachedList::Authored).unwrap().data,
            SnapshotData::Available { prs, .. } if prs == vec![sample(gh.clone())])
        );
        assert!(
            matches!(load_source_snapshot(&conn, &gl, CachedList::Authored).unwrap().data,
            SnapshotData::GitLabAvailable { mrs, .. } if mrs == vec![sample_mr(gl.clone())])
        );
        conn.execute("UPDATE snapshot SET provider = 'gitlab', host = 'wrong.example' WHERE provider = 'github'", []).unwrap();
        assert_eq!(
            load_source_snapshot(&conn, &gitlab("wrong.example"), CachedList::Authored)
                .unwrap()
                .data,
            SnapshotData::Unreadable
        );
    }

    #[test]
    fn sources_hosts_lists_and_freshness_are_independent() {
        let conn = db();
        let gh = Source::default();
        let gl = gitlab("gitlab.com");
        let other = gitlab("gitlab.example");
        save_source_snapshot(&conn, &gh, CachedList::Authored, &[], &Coverage::Complete).unwrap();
        save_gitlab_snapshot(
            &conn,
            &gl,
            CachedList::Authored,
            &[],
            &Coverage::Partial { total: Some(4) },
        )
        .unwrap();
        save_gitlab_snapshot(&conn, &other, CachedList::Authored, &[], &Coverage::Unknown).unwrap();
        conn.execute("UPDATE snapshot SET fetched_at = datetime('now', '-2 hours') WHERE provider = 'github'", []).unwrap();
        assert!(matches!(
            load_source_snapshot(&conn, &gh, CachedList::Authored)
                .unwrap()
                .data,
            SnapshotData::Available {
                stale_secs: Some(_),
                coverage: Coverage::Complete,
                ..
            }
        ));
        assert!(matches!(
            load_source_snapshot(&conn, &gl, CachedList::Authored)
                .unwrap()
                .data,
            SnapshotData::GitLabAvailable {
                stale_secs: None,
                coverage: Coverage::Partial { total: Some(4) },
                ..
            }
        ));
        assert!(matches!(
            load_source_snapshot(&conn, &other, CachedList::Authored)
                .unwrap()
                .data,
            SnapshotData::GitLabAvailable {
                coverage: Coverage::Unknown,
                ..
            }
        ));
        assert_eq!(
            load_source_snapshot(&conn, &gl, CachedList::Reviewing)
                .unwrap()
                .data,
            SnapshotData::Missing
        );
    }
    #[test]
    fn corrupt_and_absent_are_not_measured_empty() {
        let conn = db();
        let source = Source::default();
        assert_eq!(
            load_source_snapshot(&conn, &source, CachedList::Authored)
                .unwrap()
                .data,
            SnapshotData::Missing
        );
        save_source_snapshot(
            &conn,
            &source,
            CachedList::Authored,
            &[],
            &Coverage::Complete,
        )
        .unwrap();
        assert!(
            matches!(load_source_snapshot(&conn, &source, CachedList::Authored).unwrap().data,
            SnapshotData::Available { prs, .. } if prs.is_empty())
        );
        conn.execute("UPDATE snapshot SET payload = 'bad'", [])
            .unwrap();
        assert_eq!(
            load_source_snapshot(&conn, &source, CachedList::Authored)
                .unwrap()
                .data,
            SnapshotData::Unreadable
        );
    }
}
