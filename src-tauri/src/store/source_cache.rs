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
    save_payload(conn, source, list, prs, coverage)
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
    save_payload(conn, source, list, mrs, coverage)
}

fn save_payload<T: Serialize>(
    conn: &Connection,
    source: &Source,
    list: CachedList,
    rows: &[T],
    coverage: &Coverage,
) -> Result<(), StoreError> {
    let provider = serde_json::to_value(source.provider)?;
    conn.execute(
        "INSERT INTO snapshot (provider, host, id, payload, fetched_at, coverage)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5)
         ON CONFLICT(provider, host, id) DO UPDATE SET
            payload = excluded.payload, fetched_at = excluded.fetched_at,
            coverage = excluded.coverage",
        params![
            provider.as_str(),
            source.host,
            list.id(),
            serde_json::to_string(rows)?,
            serde_json::to_string(coverage)?
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
                    Ok(prs) if prs.iter().all(|pr| &pr.source == source) => {
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
