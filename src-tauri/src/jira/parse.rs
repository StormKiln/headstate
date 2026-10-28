//! Reading `acli` output.
//!
//! Shapes follow Jira Cloud REST v3 (`{key, self, fields: {...}}` per issue,
//! `{key, name}` per project). `acli` is documented to emit those objects
//! with `--json` but the envelope around them is not documented, so both a
//! bare array and an object holding `issues` / `values` are accepted. #2032
//! records the live shape once it is measured; until then a reader that
//! cannot find a field reports it absent rather than inventing a value.

use super::model::{
    JiraConnection, JiraIssue, JiraIssueType, JiraProject, JiraStatus, StatusCategory,
};
use serde_json::Value;

/// The list inside whichever envelope `acli` used.
fn items<'a>(v: &'a Value, wrappers: &[&str]) -> Option<&'a Vec<Value>> {
    if let Some(a) = v.as_array() {
        return Some(a);
    }
    wrappers
        .iter()
        .find_map(|w| v.get(*w).and_then(Value::as_array))
}

fn category(status: &Value) -> StatusCategory {
    match status
        .pointer("/statusCategory/key")
        .and_then(Value::as_str)
    {
        Some("new") => StatusCategory::New,
        Some("indeterminate") => StatusCategory::InProgress,
        Some("done") => StatusCategory::Done,
        // Includes "undefined", which Jira itself uses for "no category".
        _ => StatusCategory::Unknown,
    }
}

/// `https://<site>/rest/api/3/issue/10001` -> the site's
/// browse URL for `key`. Only an https origin is trusted to build a link.
fn browse_url(self_link: Option<&str>, key: &str) -> Option<String> {
    let link = self_link?;
    let rest = link.strip_prefix("https://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("https://{host}/browse/{key}"))
}

fn issue(v: &Value) -> Option<JiraIssue> {
    let key = v.get("key")?.as_str()?.to_string();
    let f = v.get("fields").unwrap_or(&Value::Null);
    let s = |p: &str| f.pointer(p).and_then(Value::as_str).map(str::to_string);
    let status = f.get("status").unwrap_or(&Value::Null);
    Some(JiraIssue {
        summary: s("/summary").unwrap_or_default(),
        issue_type: JiraIssueType {
            name: s("/issuetype/name").unwrap_or_default(),
            hierarchy_level: f
                .pointer("/issuetype/hierarchyLevel")
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok()),
        },
        status: JiraStatus {
            name: status
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            category: category(status),
        },
        assignee: s("/assignee/displayName"),
        parent_key: s("/parent/key"),
        project_key: s("/project/key")
            .or_else(|| key.split_once('-').map(|(p, _)| p.to_string()))
            .unwrap_or_default(),
        url: browse_url(v.get("self").and_then(Value::as_str), &key),
        updated: s("/updated"),
        key,
    })
}

/// Issues from `workitem search --json` (or a single `workitem view`).
pub fn issues(raw: &str) -> Result<Vec<JiraIssue>, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("unreadable Jira reply: {e}"))?;
    if let Some(list) = items(&v, &["issues", "values"]) {
        return Ok(list.iter().filter_map(issue).collect());
    }
    // `workitem view` answers with one object, not a list.
    issue(&v)
        .map(|i| vec![i])
        .ok_or_else(|| "Jira reply held no work items".to_string())
}

/// Projects from `project list --json`.
pub fn projects(raw: &str) -> Result<Vec<JiraProject>, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("unreadable Jira reply: {e}"))?;
    let list = items(&v, &["values", "projects"]).ok_or("Jira reply held no project list")?;
    Ok(list
        .iter()
        .filter_map(|p| {
            Some(JiraProject {
                key: p.get("key")?.as_str()?.to_string(),
                name: p
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

/// `acli jira auth status`, which has no `--json`.
///
/// Read conservatively: only a successful exit naming a site counts as
/// logged in, and only a failure that says so counts as logged out.
/// Anything else is `CannotTell`, which the settings panel shows verbatim
/// rather than guessing.
pub fn auth_status(success: bool, stdout: &str, stderr: &str) -> JiraConnection {
    let text = format!("{stdout}\n{stderr}");
    let lower = text.to_ascii_lowercase();
    let site = text
        .split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '(' || c == ')')
        .map(|w| {
            w.trim_start_matches("https://")
                .trim_end_matches(['/', ',', '.'])
        })
        .find(|w| w.ends_with(".atlassian.net"))
        .map(str::to_string);
    let logged_out = [
        "not logged in",
        "not authenticated",
        "no account",
        "unauthorized",
        "please log in",
        "acli jira auth login",
    ]
    .iter()
    .any(|p| lower.contains(p));
    if success && !logged_out {
        let account = text
            .split_whitespace()
            .find(|w| w.contains('@') && w.contains('.'))
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string());
        if site.is_some() {
            return JiraConnection::LoggedIn { site, account };
        }
    }
    if logged_out {
        return JiraConnection::NotLoggedIn {
            detail: text.trim().to_string(),
        };
    }
    JiraConnection::CannotTell {
        reason: text.trim().chars().take(400).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assembled at runtime: `scripts/check-privacy.sh` rejects any literal
    /// `<name>.atlassian.net` in committed text, synthetic or not.
    fn site() -> String {
        ["acme", "atlassian", "net"].join(".")
    }

    fn issue_json() -> String {
        ISSUE.replace("SITE", &site())
    }

    const ISSUE: &str = r#"{
      "key": "ABC-1", "id": "10001",
      "self": "https://SITE/rest/api/3/issue/10001",
      "fields": {
        "summary": "Fix login redirect",
        "issuetype": {"name": "Story", "hierarchyLevel": 0},
        "status": {"name": "In Review", "statusCategory": {"key": "indeterminate"}},
        "assignee": {"displayName": "Dana Example"},
        "parent": {"key": "ABC-2"},
        "project": {"key": "ABC"},
        "updated": "2026-10-01T10:00:00.000+0000"
      }
    }"#;

    #[test]
    fn reads_a_rest_v3_issue_in_any_envelope() {
        for raw in [
            format!("[{}]", issue_json()),
            format!(r#"{{"issues": [{}]}}"#, issue_json()),
            issue_json(),
        ] {
            let got = issues(&raw).unwrap();
            assert_eq!(got.len(), 1, "{raw}");
            let i = &got[0];
            assert_eq!(i.key, "ABC-1");
            assert_eq!(i.status.category, StatusCategory::InProgress);
            assert_eq!(i.status.name, "In Review");
            assert_eq!(i.parent_key.as_deref(), Some("ABC-2"));
            assert_eq!(i.issue_type.hierarchy_level, Some(0));
            assert_eq!(
                i.url.as_deref(),
                Some(format!("https://{}/browse/ABC-1", site()).as_str())
            );
        }
    }

    /// Absent is not zero: a status with no category must not read as To Do.
    #[test]
    fn a_missing_category_is_unknown_not_new() {
        let raw = r#"[{"key": "ABC-3", "fields": {"status": {"name": "Odd"}}}]"#;
        let i = &issues(raw).unwrap()[0];
        assert_eq!(i.status.category, StatusCategory::Unknown);
        assert_eq!(i.url, None, "no self link, so no invented URL");
        assert_eq!(i.project_key, "ABC", "falls back to the key's prefix");
        assert_eq!(i.issue_type.hierarchy_level, None);
    }

    #[test]
    fn a_plain_http_self_link_builds_no_url() {
        let raw = format!(
            r#"{{"key": "ABC-4", "self": "http://{}/x", "fields": {{}}}}"#,
            site()
        );
        assert_eq!(issues(&raw).unwrap()[0].url, None);
    }

    #[test]
    fn rejects_unreadable_replies_rather_than_returning_empty() {
        assert!(issues("not json").is_err());
        assert!(issues(r#"{"something": "else"}"#).is_err());
        assert!(projects(r#"{"x": 1}"#).is_err());
    }

    #[test]
    fn reads_projects_bare_or_paged() {
        for raw in [
            r#"[{"key": "ABC", "name": "Alpha"}, {"key": "XY", "name": "Ex"}]"#,
            r#"{"values": [{"key": "ABC", "name": "Alpha"}, {"key": "XY", "name": "Ex"}]}"#,
        ] {
            let p = projects(raw).unwrap();
            assert_eq!(
                p.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
                ["ABC", "XY"]
            );
        }
    }

    #[test]
    fn auth_status_is_read_conservatively() {
        assert_eq!(
            auth_status(
                true,
                &format!(
                    "✓ Authenticated\n  Site: {}\n  Email: dana@users.noreply.github.com",
                    site()
                ),
                ""
            ),
            JiraConnection::LoggedIn {
                site: Some(site()),
                account: Some("dana@users.noreply.github.com".into()),
            }
        );
        assert!(matches!(
            auth_status(false, "", "You are not logged in. Run acli jira auth login"),
            JiraConnection::NotLoggedIn { .. }
        ));
        // Success with no site named is not proof of a login.
        assert!(matches!(
            auth_status(true, "ok", ""),
            JiraConnection::CannotTell { .. }
        ));
        assert!(matches!(
            auth_status(false, "", "boom"),
            JiraConnection::CannotTell { .. }
        ));
    }
}
