//! Semantic graph deltas. Caller owns the fenced, atomic publication transaction.
use crate::claude::subagent::PrLink;
use rusqlite::{params, Connection};
use std::collections::BTreeMap;

type LinkKey = (String, String, i64);
type LinkValue = (String, Option<String>);
type ParentValue = (String, Option<String>, Option<String>);

pub(super) struct Attribution {
    pub session_id: String,
    pub agent_id: String,
    pub parent: Option<String>,
    pub why: Option<String>,
}

/// None attribution means partial discovery: add/update observed links but
/// retain missing links and all prior attribution. Some means a complete graph.
pub(super) fn publish(
    conn: &Connection,
    links: &[PrLink],
    parents: Option<&[Attribution]>,
    now: &str,
) -> rusqlite::Result<()> {
    // Collect keeps the last incoming value for duplicate keys, matching the
    // former ordered replacement loop without transient duplicate writes.
    let desired: BTreeMap<LinkKey, LinkValue> = links
        .iter()
        .map(|l| {
            (
                (l.session_id.clone(), l.repo.clone(), l.number as i64),
                (l.url.clone(), l.first_seen_at.clone()),
            )
        })
        .collect();
    let existing: BTreeMap<LinkKey, LinkValue> = conn
        .prepare("SELECT session_id, repo, number, url, first_seen_at FROM claude_session_pr")?
        .query_map([], |r| {
            Ok(((r.get(0)?, r.get(1)?, r.get(2)?), (r.get(3)?, r.get(4)?)))
        })?
        .collect::<Result<_, _>>()?;
    if parents.is_some() {
        for (session, repo, number) in existing.keys().filter(|key| !desired.contains_key(*key)) {
            conn.execute(
                "DELETE FROM claude_session_pr WHERE session_id=?1 AND repo=?2 AND number=?3",
                params![session, repo, number],
            )?;
        }
    }
    for (key, value) in &desired {
        if existing.get(key) == Some(value) {
            continue;
        }
        let (session, repo, number) = key;
        let (url, first_seen) = value;
        conn.execute("INSERT INTO claude_session_pr(session_id, repo, number, url, first_seen_at)
            VALUES(?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(session_id, repo, number) DO UPDATE SET url=excluded.url, first_seen_at=excluded.first_seen_at",
            params![session, repo, number, url, first_seen])?;
    }
    let Some(parents) = parents else {
        return Ok(());
    };
    let desired: BTreeMap<String, ParentValue> = parents
        .iter()
        .map(|p| {
            (
                p.session_id.clone(),
                (p.agent_id.clone(), p.parent.clone(), p.why.clone()),
            )
        })
        .collect();
    let existing: BTreeMap<String, ParentValue> = conn
        .prepare("SELECT session_id, agent_id, parent_session_id, why FROM claude_subagent")?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
        .collect::<Result<_, _>>()?;
    for session in existing.keys().filter(|key| !desired.contains_key(*key)) {
        conn.execute("DELETE FROM claude_subagent WHERE session_id=?1", [session])?;
    }
    for (session, value) in &desired {
        if existing.get(session) == Some(value) {
            continue;
        }
        let (agent, parent, why) = value;
        // resolved_at is when attribution last changed, not when an identical
        // corpus was visited. NULL fields are compared as Options above.
        conn.execute("INSERT INTO claude_subagent(session_id, agent_id, parent_session_id, why, resolved_at)
            VALUES(?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(session_id) DO UPDATE SET agent_id=excluded.agent_id,
                parent_session_id=excluded.parent_session_id, why=excluded.why, resolved_at=excluded.resolved_at",
            params![session, agent, parent, why, now])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "synthetic bundled SQLite graph/WAL measurement; run explicitly"]
    fn graph_writer_measurements() {
        eprintln!("rows,delta,changed_percent,mutations,writer_us,wal_bytes");
        for count in [1000, 10000] {
            for delta in [false, true] {
                for changed in [false, true] {
                    let dir = tempfile::tempdir().unwrap();
                    let mut conn = Connection::open(dir.path().join("synthetic.sqlite")).unwrap();
                    crate::store::migrate(&conn).unwrap();
                    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
                    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
                    let mut links: Vec<_> = (0..count)
                        .map(|i| PrLink {
                            session_id: format!("s{i}"),
                            repo: "octocat/hello-world".into(),
                            number: i as u64,
                            url: format!("https://github.com/octocat/hello-world/pull/{i}"),
                            first_seen_at: None,
                        })
                        .collect();
                    let mut parents: Vec<_> = (0..count)
                        .map(|i| Attribution {
                            session_id: format!("s{i}"),
                            agent_id: format!("a{i}"),
                            parent: Some("synthetic-parent".into()),
                            why: None,
                        })
                        .collect();
                    let tx = conn.transaction().unwrap();
                    publish(&tx, &links, Some(&parents), "initial").unwrap();
                    tx.commit().unwrap();
                    if changed {
                        for i in 0..count / 100 {
                            links[i].first_seen_at = Some("new-evidence".into());
                            parents[i].parent = Some("changed-parent".into());
                        }
                    }
                    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                        .unwrap();
                    let before = conn.total_changes();
                    let started = std::time::Instant::now();
                    let tx = conn
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                        .unwrap();
                    if delta {
                        publish(&tx, &links, Some(&parents), "new").unwrap();
                    } else {
                        tx.execute("DELETE FROM claude_session_pr", []).unwrap();
                        tx.execute("DELETE FROM claude_subagent", []).unwrap();
                        for l in &links {
                            tx.execute("INSERT INTO claude_session_pr(session_id,repo,number,url,first_seen_at) VALUES(?1,?2,?3,?4,?5)", params![l.session_id,l.repo,l.number as i64,l.url,l.first_seen_at]).unwrap();
                        }
                        for p in &parents {
                            tx.execute("INSERT INTO claude_subagent(session_id,agent_id,parent_session_id,why,resolved_at) VALUES(?1,?2,?3,?4,'new')",params![p.session_id,p.agent_id,p.parent,p.why]).unwrap();
                        }
                    }
                    tx.commit().unwrap();
                    let elapsed = started.elapsed().as_micros();
                    let mutations = conn.total_changes() - before;
                    if delta {
                        assert_eq!(
                            mutations,
                            if changed { (count / 100 * 2) as u64 } else { 0 }
                        );
                    }
                    let wal = std::fs::metadata(dir.path().join("synthetic.sqlite-wal"))
                        .unwrap()
                        .len();
                    eprintln!(
                        "{count},{delta},{},{mutations},{elapsed},{wal}",
                        usize::from(changed)
                    );
                }
            }
        }
    }
}
