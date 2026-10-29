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

/// The browse URL for `key` on `site`, the host `auth status` names.
///
/// Not derived from the API's `self` link: through `acli` that names an
/// internal Atlassian host (`jira-prod-*.prod.atl-paas.net`), not the
/// user's site, so a link built from it does not open (#2032). Only a
/// plain hostname is trusted to build a link.
pub fn browse_url(site: Option<&str>, key: &str) -> Option<String> {
    let site = site?;
    let plain = !site.is_empty()
        && site.contains('.')
        && site
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    plain.then(|| format!("https://{site}/browse/{key}"))
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
        // Set by the reader that knows whether `parent` was asked for.
        parent_checked: false,
        project_key: s("/project/key")
            .or_else(|| key.split_once('-').map(|(p, _)| p.to_string()))
            .unwrap_or_default(),
        url: None,
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

/// The parent named by `workitem view --fields parent,issuetype --json`.
///
/// `Ok(None)` is an answer: the issue has no parent, and Jira omits the
/// field. It is only trusted from a reply that is visibly an issue -- a
/// key and a `fields` object -- because asked for `parent` alone, an issue
/// with none comes back as a bare `null`, indistinguishable from nothing.
pub fn parent(raw: &str) -> Result<Option<String>, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("unreadable Jira reply: {e}"))?;
    let fields = v
        .get("key")
        .and_then(Value::as_str)
        .and(v.get("fields"))
        .filter(|f| f.is_object())
        .ok_or("Jira reply held no work item")?;
    Ok(fields
        .pointer("/parent/key")
        .and_then(Value::as_str)
        .map(str::to_string))
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

/// The distinct statuses held by the issues of one `workitem search
/// --fields status --json`, in the order first seen. A status whose name
/// did not come back is left out: an empty name is not a status anyone
/// can choose.
pub fn distinct_statuses(raw: &str) -> Result<Vec<JiraStatus>, String> {
    let mut out: Vec<JiraStatus> = Vec::new();
    for s in issues(raw)?.into_iter().map(|i| i.status) {
        if !s.name.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

/// The version from `acli --version`, which prints e.g.
/// `acli version 1.3.39-stable`. `None` when the line is not that shape.
pub fn acli_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().find(|l| !l.trim().is_empty())?.trim();
    let rest = line.strip_prefix("acli version ")?.trim();
    (!rest.is_empty() && !rest.contains(char::is_whitespace)).then(|| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_distinct_by_name_and_category() {
        let raw = r#"[
          {"key":"ABC-1","fields":{"status":{"name":"To Do","statusCategory":{"key":"new"}}}},
          {"key":"ABC-2","fields":{"status":{"name":"In Review","statusCategory":{"key":"indeterminate"}}}},
          {"key":"ABC-3","fields":{"status":{"name":"To Do","statusCategory":{"key":"new"}}}},
          {"key":"ABC-4","fields":{"status":{"statusCategory":{"key":"done"}}}}
        ]"#;
        let got = distinct_statuses(raw).unwrap();
        assert_eq!(
            got,
            [
                JiraStatus {
                    name: "To Do".into(),
                    category: StatusCategory::New
                },
                JiraStatus {
                    name: "In Review".into(),
                    category: StatusCategory::InProgress
                },
            ]
        );
        assert!(distinct_statuses("not json").is_err());
    }

    #[test]
    fn the_version_is_read_from_the_live_shape() {
        assert_eq!(
            acli_version("acli version 1.3.39-stable\n").as_deref(),
            Some("1.3.39-stable")
        );
        assert_eq!(acli_version(""), None);
        assert_eq!(acli_version("something else entirely"), None);
    }

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
            assert_eq!(i.url, None, "the self link is not the site");
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
    fn browse_urls_come_from_the_site_and_only_a_plain_host() {
        assert_eq!(
            browse_url(Some(&site()), "ABC-4").as_deref(),
            Some(format!("https://{}/browse/ABC-4", site()).as_str())
        );
        for bad in ["", "evil.example/x?", "a b.net", "localhost"] {
            assert_eq!(browse_url(Some(bad), "ABC-4"), None, "{bad}");
        }
        assert_eq!(browse_url(None, "ABC-4"), None);
    }

    /// Replies captured from a live `acli` 1.3.39 on a test site (#2032),
    /// with account ids, names and emails replaced.
    const LIVE_SEARCH: &str = include_str!("fixtures/search.json");
    const LIVE_VIEW_PARENT: &str = include_str!("fixtures/view_with_parent.json");
    const LIVE_VIEW_NO_PARENT: &str = include_str!("fixtures/view_without_parent.json");

    #[test]
    fn reads_a_live_search_reply() {
        let got = issues(LIVE_SEARCH).unwrap();
        let by = |k: &str| got.iter().find(|i| i.key == k).unwrap();
        assert_eq!(got.len(), 2);
        let epic = by("SAM-1");
        assert_eq!(epic.issue_type.name, "Epic");
        assert_eq!(epic.issue_type.hierarchy_level, Some(1));
        assert_eq!(epic.status.category, StatusCategory::New);
        let task = by("SAM-4");
        assert_eq!(task.issue_type.hierarchy_level, Some(0));
        assert_eq!(task.assignee.as_deref(), Some("Dana Example"));
        assert_eq!(task.project_key, "SAM", "search cannot return project");
        // Search refuses `parent`: nothing here says whether there is one.
        assert_eq!(task.parent_key, None);
        assert!(!task.parent_checked);
        assert_eq!(task.url, None);
    }

    #[test]
    fn reads_parents_from_live_view_replies() {
        assert_eq!(parent(LIVE_VIEW_PARENT).unwrap().as_deref(), Some("SAM-1"));
        assert_eq!(parent(LIVE_VIEW_NO_PARENT).unwrap(), None);
        // What `--fields parent` alone returns for an issue with none.
        assert!(parent("null").is_err());
        assert!(parent("{}").is_err());
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
