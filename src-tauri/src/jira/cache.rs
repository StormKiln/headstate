//! Jira issues as last read, kept in `jira_issue` (#2032).
//!
//! Two clocks per issue. `read_at` is when the status was last read, by a
//! batched search; a search is cheap, so it goes stale in a minute.
//! `parent_read_at` is when the parent was last read, which `acli` only
//! answers one `workitem view` at a time; parents rarely move, so that
//! answer is kept for hours.

use super::model::JiraIssue;
use crate::store::{settings, StoreError};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One issue as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Cached {
    pub issue: JiraIssue,
    pub read_at: DateTime<Utc>,
    /// `None`: the parent has never been read. Not "has no parent".
    pub parent_read_at: Option<DateTime<Utc>>,
}

impl Cached {
    /// The issue with the stored parent and its age applied, as the UI
    /// sees it.
    pub fn resolved(&self) -> JiraIssue {
        let mut issue = self.issue.clone();
        issue.parent_checked = self.parent_read_at.is_some();
        issue.read_at = Some(self.read_at.to_rfc3339());
        issue
    }
}

/// The last assigned list that was read in full (#2036): which keys, and
/// when. A failed search is answered from this, marked stale, rather
/// than with the error alone. The issues themselves are in `jira_issue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssignedList {
    pub keys: Vec<String>,
    pub truncated: bool,
    pub read_at: DateTime<Utc>,
}

pub fn save_assigned(conn: &Connection, list: &AssignedList) -> Result<(), StoreError> {
    settings::set(conn, settings::keys::JIRA_ASSIGNED, list)
}

/// `None`: no assigned list has ever been read. Not "nothing assigned".
pub fn load_assigned(conn: &Connection) -> Result<Option<AssignedList>, StoreError> {
    settings::get(conn, settings::keys::JIRA_ASSIGNED)
}

fn when(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

pub fn load(conn: &Connection, keys: &[String]) -> Result<HashMap<String, Cached>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT payload, read_at, parent_key, parent_read_at FROM jira_issue WHERE key = ?1",
    )?;
    let mut out = HashMap::new();
    for key in keys {
        let row = stmt.query_row([key], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        });
        let (payload, read_at, parent_key, parent_read_at) = match row {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(e) => return Err(e.into()),
        };
        // A row this build cannot read is skipped, not fatal: it is
        // re-read from Jira and overwritten.
        let (Ok(mut issue), Some(read_at)) =
            (serde_json::from_str::<JiraIssue>(&payload), when(&read_at))
        else {
            continue;
        };
        let parent_read_at = parent_read_at.as_deref().and_then(when);
        if parent_read_at.is_some() {
            issue.parent_key = parent_key;
        }
        out.insert(
            key.clone(),
            Cached {
                issue,
                read_at,
                parent_read_at,
            },
        );
    }
    Ok(out)
}

/// Store issues just read by a search. The parent columns are left alone:
/// a search cannot see parents, so it has nothing to say about them.
pub fn save_read(
    conn: &Connection,
    issues: &[JiraIssue],
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO jira_issue (key, payload, read_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET payload = excluded.payload, read_at = excluded.read_at",
    )?;
    for issue in issues {
        let payload = serde_json::to_string(issue)?;
        stmt.execute(params![issue.key, payload, now.to_rfc3339()])?;
    }
    Ok(())
}

/// Store a parent read by `workitem view`. `parent` `None` is an answer:
/// the issue has no parent.
pub fn save_parent(
    conn: &Connection,
    key: &str,
    parent: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE jira_issue SET parent_key = ?2, parent_read_at = ?3 WHERE key = ?1",
        params![key, parent, now.to_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jira::model::{JiraIssueType, JiraStatus, StatusCategory};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();
        conn
    }

    fn issue(key: &str) -> JiraIssue {
        JiraIssue {
            key: key.into(),
            id: None,
            summary: "s".into(),
            issue_type: JiraIssueType {
                name: "Task".into(),
                hierarchy_level: Some(0),
                icon_url: None,
            },
            status: JiraStatus {
                name: "To Do".into(),
                category: StatusCategory::New,
            },
            assignee: None,
            parent_key: None,
            parent_checked: false,
            project_key: "ABC".into(),
            url: None,
            updated: None,
            read_at: None,
            stale: false,
        }
    }

    #[test]
    fn a_parent_never_read_is_not_no_parent() {
        let conn = db();
        let now = Utc::now();
        save_read(&conn, &[issue("ABC-1")], now).unwrap();
        let got = &load(&conn, &["ABC-1".into()]).unwrap()["ABC-1"];
        assert_eq!(got.parent_read_at, None);
        assert!(!got.resolved().parent_checked);

        save_parent(&conn, "ABC-1", None, now).unwrap();
        let got = &load(&conn, &["ABC-1".into()]).unwrap()["ABC-1"];
        assert!(got.resolved().parent_checked, "read, and it has none");
        assert_eq!(got.resolved().parent_key, None);
    }

    #[test]
    fn a_later_search_keeps_the_parent_it_cannot_see() {
        let conn = db();
        let now = Utc::now();
        save_read(&conn, &[issue("ABC-1")], now).unwrap();
        save_parent(&conn, "ABC-1", Some("ABC-2"), now).unwrap();
        let mut moved = issue("ABC-1");
        moved.status.category = StatusCategory::Done;
        save_read(&conn, &[moved], now).unwrap();
        let got = load(&conn, &["ABC-1".into()]).unwrap()["ABC-1"].resolved();
        assert_eq!(got.parent_key.as_deref(), Some("ABC-2"));
        assert_eq!(got.status.category, StatusCategory::Done);
    }

    #[test]
    fn a_served_row_says_when_it_was_read() {
        let conn = db();
        let now = Utc::now();
        save_read(&conn, &[issue("ABC-1")], now).unwrap();
        let got = load(&conn, &["ABC-1".into()]).unwrap()["ABC-1"].resolved();
        assert_eq!(got.read_at, Some(now.to_rfc3339()));
        assert!(!got.stale);
    }

    #[test]
    fn the_assigned_list_is_absent_until_read() {
        let conn = db();
        assert_eq!(load_assigned(&conn).unwrap(), None);
        let list = AssignedList {
            keys: vec!["ABC-1".into()],
            truncated: false,
            read_at: Utc::now(),
        };
        save_assigned(&conn, &list).unwrap();
        assert_eq!(load_assigned(&conn).unwrap(), Some(list));
    }

    #[test]
    fn unknown_keys_are_absent_not_empty_issues() {
        assert!(load(&db(), &["ABC-9".into()]).unwrap().is_empty());
    }
}
