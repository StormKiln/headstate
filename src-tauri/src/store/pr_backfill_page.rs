//! Durable, bounded search continuations (#1626). A dense day previously
//! repeated its first page forever. Attempts also rotate dates within a scope.
use super::{
    pr_history, pr_slice,
    stats_owner::{self, StatsOwner},
    StoreError,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const VERSION: u32 = 1;
const CAP: usize = crate::github::stats::slice::SEARCH_CAP as usize;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    version: u32,
    pub day: String,
    pub after: Option<String>,
    total: Option<u64>,
    ids: BTreeSet<(String, u64)>,
    cursors: BTreeSet<String>,
}
impl Page {
    fn new(day: String) -> Self {
        Self {
            version: VERSION,
            day,
            after: None,
            total: None,
            ids: BTreeSet::new(),
            cursors: BTreeSet::new(),
        }
    }
}

/// Reservations commit before network work: cancellation/failure still yields
/// to other dates. This selects work without claiming history coverage.
pub fn select_in(
    tx: &rusqlite::Transaction<'_>,
    owner: &StatsOwner,
    key: &str,
    dates: &[String],
    limit: usize,
) -> Result<Vec<Page>, String> {
    stats_owner::require_current(tx, owner).map_err(|e| e.to_string())?;
    let viewer = owner.viewer();
    // Retain only this scope's still-uncovered dates. Old horizons and
    // days completed by the foreground do not accumulate orphan cursors.
    let old_days: Vec<String> = {
        let mut stmt = tx
            .prepare("SELECT day FROM pr_backfill_page WHERE viewer=?1 AND scope_key=?2")
            .map_err(|e| e.to_string())?;
        let days = stmt
            .query_map(params![viewer, key], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        days
    };
    for day in old_days {
        if !dates.contains(&day) {
            tx.execute(
                "DELETE FROM pr_backfill_page WHERE viewer=?1 AND scope_key=?2 AND day=?3",
                params![viewer, key, day],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    let mut candidates = Vec::new();
    for day in dates {
        let row: Option<(String, i64)> = tx.query_row(
            "SELECT payload, attempted FROM pr_backfill_page WHERE viewer=?1 AND scope_key=?2 AND day=?3",
            params![viewer,key,day], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e| e.to_string())?;
        let (page, order) = match row {
            Some((payload, order)) => {
                let page = (payload.len() <= 8 * 1024 * 1024)
                    .then(|| serde_json::from_str::<Page>(&payload).ok())
                    .flatten()
                    .filter(|p| {
                        p.version == VERSION
                            && p.day == *day
                            && p.ids.len() <= CAP
                            && p.cursors.len() <= CAP
                            && p.after.as_ref().is_none_or(|s| s.len() <= 4096)
                            && p.cursors.iter().all(|s| !s.is_empty() && s.len() <= 4096)
                            && p.after.is_some() == p.total.is_some()
                            && p.after
                                .as_ref()
                                .is_none_or(|s| !p.ids.is_empty() && p.cursors.contains(s))
                            && p.ids.len() as u64 <= p.total.unwrap_or(0)
                    });
                (page.unwrap_or_else(|| Page::new(day.clone())), order)
            }
            None => (Page::new(day.clone()), -1),
        };
        candidates.push((order, page));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.day.cmp(&b.1.day)));
    let order: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(attempted),0)+1 FROM pr_backfill_page",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let selected: Vec<Page> = candidates.into_iter().take(limit).map(|(_, p)| p).collect();
    for page in &selected {
        let payload = serde_json::to_string(page).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO pr_backfill_page(viewer,scope_key,day,payload,attempted) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(viewer,scope_key,day) DO UPDATE SET payload=excluded.payload,attempted=excluded.attempted",params![viewer,key,page.day,payload,order]).map_err(|e| e.to_string())?;
    }
    Ok(selected)
}

/// Persist usable rows even when the page cannot support a cursor/coverage
/// claim. Each returned bool says whether this page's contract was sound.
pub fn commit_in(
    tx: &rusqlite::Transaction<'_>,
    owner: &StatsOwner,
    key: &str,
    pages: &[Page],
    map: &serde_json::Value,
    now: DateTime<Utc>,
) -> Result<(usize, bool), String> {
    stats_owner::require_current(tx, owner).map_err(|e| e.to_string())?;
    let viewer = owner.viewer();
    let mut written = 0;
    let mut valid = true;
    for (i, previous) in pages.iter().enumerate() {
        // A foreground load may have completed this day while we fetched it.
        let coverage =
            pr_slice::coverage(tx, key, &previous.day, &previous.day).map_err(|e| e.to_string())?;
        if coverage.days_covered() == 1 && !coverage.partial {
            continue;
        }
        let alias = &map[crate::github::stats::query::slice_alias(i)];
        let nodes = alias["nodes"].as_array();
        let raw: Vec<_> = nodes
            .into_iter()
            .flatten()
            .filter(|n| valid_node(n, &previous.day))
            .cloned()
            .collect();
        let one = serde_json::json!({"s0":{"nodes":raw}});
        let slices =
            crate::github::stats::backfill::day_slices(std::slice::from_ref(&previous.day));
        let rows = crate::github::stats::Board::retrieved_prs(&one, &slices);
        written += pr_history::put_many_in(tx, key, &previous.day, &previous.day, &rows, now)
            .map_err(|e| e.to_string())?;
        let mut page = previous.clone();
        let count = alias["issueCount"].as_u64();
        let more = alias["pageInfo"]["hasNextPage"].as_bool();
        let cursor = alias["pageInfo"]["endCursor"]
            .as_str()
            .filter(|c| !c.is_empty() && c.len() <= 4096);
        let mut sound = map["__refused"].as_u64().unwrap_or(0) == 0
            && map["__partial_errors"].as_u64().unwrap_or(0) == 0
            && nodes.is_some_and(|n| n.len() == rows.len() && n.len() <= 50)
            && count.is_some_and(|n| n <= i64::MAX as u64)
            && more.is_some()
            && (alias["pageInfo"]["endCursor"].is_null() || cursor.is_some())
            && alias["pageInfo"].get("endCursor").is_some();
        if let Some(count) = count {
            sound &= page.total.is_none_or(|old| old == count);
        }
        for row in &rows {
            sound &= page.ids.insert((row.repo.clone(), row.number));
        }
        sound &= page.ids.len() <= CAP && page.ids.len() as u64 <= count.unwrap_or(0);
        // A terminal page is still a page in this pass: its new rows
        // need a fresh cursor before they can certify complete coverage.
        // Null remains valid for a genuinely empty result.
        if !rows.is_empty() {
            sound &= cursor.is_some_and(|c| page.cursors.insert(c.to_string()));
        }
        if more == Some(true) {
            sound &= !rows.is_empty();
        }
        if more == Some(false) && count.is_some_and(|n| n <= CAP as u64) {
            sound &= page.ids.len() as u64 == count.unwrap();
        }
        let capped = count.is_some_and(|n| n > CAP as u64) && page.ids.len() == CAP;
        if more == Some(false) && count.is_some_and(|n| n > CAP as u64) {
            sound &= capped;
        }
        if sound {
            page.total = count;
            page.after = cursor.map(str::to_string);
            let terminal = more == Some(false) || capped;
            let state = if capped {
                pr_slice::SliceState::Irreducible
            } else if terminal {
                pr_slice::SliceState::Complete
            } else {
                pr_slice::SliceState::Refused
            };
            pr_slice::put(
                tx,
                key,
                &pr_slice::SliceRow {
                    from: page.day.clone(),
                    to: page.day.clone(),
                    state,
                    issue_count: count.unwrap(),
                    retrieved: page.ids.len() as u64,
                    refused_fields: 0,
                },
                now,
            )
            .map_err(|e| e.to_string())?;
            if terminal {
                // Remove obsolete observations from a discarded pass only
                // once a full current pass proves the day's membership.
                if !capped {
                    for old in pr_history::load(tx, key, &page.day, &page.day)
                        .map_err(|e| e.to_string())?
                    {
                        if !page.ids.contains(&(old.repo.clone(), old.number)) {
                            tx.execute("DELETE FROM pr_history WHERE scope_key=?1 AND repo=?2 AND number=?3",params![key,old.repo,old.number as i64]).map_err(|e| e.to_string())?;
                        }
                    }
                }
                tx.execute(
                    "DELETE FROM pr_backfill_page WHERE viewer=?1 AND scope_key=?2 AND day=?3",
                    params![viewer, key, page.day],
                )
                .map_err(|e| e.to_string())?;
                continue;
            }
        } else {
            valid = false;
            // Inconsistent/reordered pages start a fresh pass next turn;
            // history survives, but cannot be used to claim completeness.
            page = Page::new(previous.day.clone());
        }
        tx.execute(
            "UPDATE pr_backfill_page SET payload=?4 WHERE viewer=?1 AND scope_key=?2 AND day=?3",
            params![
                viewer,
                key,
                page.day,
                serde_json::to_string(&page).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok((written, valid))
}

#[cfg(test)]
fn select(
    conn: &mut Connection,
    viewer: &str,
    key: &str,
    dates: &[String],
    limit: usize,
) -> Result<Vec<Page>, String> {
    let owner = stats_owner::current_for_verified(conn, viewer)
        .map_err(|e| e.to_string())?
        .ok_or("stats account changed")?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let pages = select_in(&tx, &owner, key, dates, limit)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(pages)
}
#[cfg(test)]
fn commit(
    conn: &mut Connection,
    viewer: &str,
    key: &str,
    pages: &[Page],
    map: &serde_json::Value,
    now: DateTime<Utc>,
) -> Result<(usize, bool), String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let owner = stats_owner::current_for_verified(&tx, viewer)
        .map_err(|e| e.to_string())?
        .ok_or("stats account changed")?;
    let result = commit_in(&tx, &owner, key, pages, map, now)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

fn valid_node(n: &serde_json::Value, day: &str) -> bool {
    n["number"]
        .as_u64()
        .is_some_and(|v| v > 0 && v <= i64::MAX as u64)
        && n["repository"]["nameWithOwner"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        && n["title"].is_string()
        && n["url"].is_string()
        && n.get("author")
            .is_some_and(|a| a.is_null() || a["login"].is_string())
        && n["createdAt"]
            .as_str()
            .is_some_and(|s| DateTime::parse_from_rfc3339(s).is_ok())
        && n["mergedAt"].as_str().is_some_and(|s| {
            DateTime::parse_from_rfc3339(s).is_ok_and(|d| d.date_naive().to_string() == day)
        })
        && ["additions", "deletions", "changedFiles"]
            .iter()
            .all(|k| n[k].as_u64().is_some())
        && n["reviews"]["totalCount"].as_u64().is_some()
}

pub fn clear(conn: &Connection) -> Result<usize, StoreError> {
    Ok(conn.execute("DELETE FROM pr_backfill_page", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        stats_owner::capture_verified(&conn, "a").unwrap();
        conn
    }
    fn page() -> serde_json::Value {
        serde_json::json!({"s0":{"issueCount":2,"pageInfo":{"hasNextPage":true,"endCursor":"next"},"nodes":[{
            "number":1,"title":"fixture","url":"https://example.com/pr","repository":{"nameWithOwner":"fixture/repo"},"author":null,
            "createdAt":"2026-09-01T00:00:00Z","mergedAt":"2026-09-01T01:00:00Z","additions":1,"deletions":0,"changedFiles":1,"reviews":{"totalCount":0}
        }]}})
    }
    #[test]
    fn dense_pages_roll_back_history_cursor_and_ledger_together() {
        let mut conn = db();
        let dates = vec!["2026-09-01".into()];
        let pages = select(&mut conn, "a", "merged|*|org:fixture", &dates, 5).unwrap();
        conn.execute_batch("CREATE TRIGGER refuse_page BEFORE UPDATE ON pr_backfill_page BEGIN SELECT RAISE(ABORT,'fixture refusal'); END;").unwrap();
        assert!(commit(
            &mut conn,
            "a",
            "merged|*|org:fixture",
            &pages,
            &page(),
            Utc::now()
        )
        .is_err());
        assert_eq!(
            pr_history::count(&conn, "merged|*|org:fixture", &dates[0], &dates[0]).unwrap(),
            0
        );
        assert_eq!(pr_slice::total_rows(&conn).unwrap(), 0);
        let payload: String = conn
            .query_row("SELECT payload FROM pr_backfill_page", [], |r| r.get(0))
            .unwrap();
        assert!(serde_json::from_str::<Page>(&payload)
            .unwrap()
            .after
            .is_none());
    }
    #[test]
    fn dense_pages_partition_scope_measure_and_date_and_reset_corrupt_state() {
        let mut conn = db();
        let dates = vec!["2026-09-01".into()];
        let pages = select(&mut conn, "a", "merged|*|org:fixture", &dates, 5).unwrap();
        commit(
            &mut conn,
            "a",
            "merged|*|org:fixture",
            &pages,
            &page(),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(
            select(&mut conn, "a", "merged|*|org:fixture", &dates, 5).unwrap()[0]
                .after
                .as_deref(),
            Some("next")
        );
        for key in ["opened|*|org:fixture", "merged|*|org:other"] {
            assert!(select(&mut conn, "a", key, &dates, 5).unwrap()[0]
                .after
                .is_none());
        }
        assert!(select(
            &mut conn,
            "a",
            "merged|*|org:fixture",
            &["2026-09-02".into()],
            5
        )
        .unwrap()[0]
            .after
            .is_none());
        assert!(select(&mut conn, "b", "merged|*|org:fixture", &dates, 5).is_err());
        conn.execute("UPDATE pr_backfill_page SET payload='invalid'", [])
            .unwrap();
        assert!(
            select(&mut conn, "a", "merged|*|org:fixture", &dates, 5).unwrap()[0]
                .after
                .is_none()
        );
    }
}
