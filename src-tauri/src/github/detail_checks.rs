//! Same-head check observations collected under the primary operation deadline.
use super::{
    client::GitHubClient,
    model::{ChecksCoverage, ChecksState},
    query::PR_CHECKS_PAGE_QUERY,
};
use serde_json::{json, Value};
use std::collections::HashSet;

const PR: &str = "/repository/pullRequest";
const COMMIT: &str = "/repository/pullRequest/commits/nodes/0/commit";
const CONTEXTS: &str = "/repository/pullRequest/commits/nodes/0/commit/statusCheckRollup/contexts";

// Error paths are consumed here, never persisted or sent to the frontend.
pub(super) fn mark_errors(data: &mut Value, errors: &[Value]) {
    let affected = errors.iter().any(|error| {
        let Some(path) = error["path"].as_array() else {
            return true;
        };
        let prefix = [
            json!("repository"),
            json!("pullRequest"),
            json!("commits"),
            json!("nodes"),
            json!(0),
            json!("commit"),
        ];
        if !path
            .iter()
            .zip(&prefix)
            .all(|(part, expected)| part == expected)
        {
            return false;
        }
        if path.len() <= prefix.len() {
            return true;
        }
        // Commit date and optional workflow-run linkage do not determine check coverage.
        (path[6] == "oid" || path[6] == "statusCheckRollup")
            && !path.iter().any(|part| part == "checkSuite")
    });
    if affected {
        if let Some(data) = data.as_object_mut() {
            data.insert("__checks_unavailable".into(), true.into());
        }
    }
}
fn coverage(state: ChecksState, total: Option<u64>) -> ChecksCoverage {
    ChecksCoverage { state, total }
}
fn head_matches(v: &Value, number: u64, head: &str) -> bool {
    let pr = &v.pointer(PR).unwrap_or(&Value::Null);
    pr["number"].as_u64() == Some(number)
        && pr["headRefOid"].as_str() == Some(head)
        && v.pointer(COMMIT).and_then(|c| c["oid"].as_str()) == Some(head)
}
fn usable(node: &Value) -> bool {
    node["id"].as_str().is_some_and(|s| !s.is_empty())
        && ((node["name"].as_str().is_some_and(|s| !s.is_empty())
            && node
                .get("conclusion")
                .is_some_and(|v| v.is_null() || v.is_string()))
            || (node["context"].as_str().is_some_and(|s| !s.is_empty())
                && node["state"].is_string()))
}
fn read_nodes(v: &Value, seen: &mut HashSet<String>) -> (Vec<Value>, bool) {
    let Some(nodes) = v["nodes"].as_array() else {
        return (vec![], false);
    };
    let mut valid = nodes.len() <= 100;
    let mut rows = vec![];
    for node in nodes.iter().take(100) {
        if !usable(node) {
            valid = false;
            continue;
        }
        if !seen.insert(node["id"].as_str().unwrap().into()) {
            valid = false;
            continue;
        }
        rows.push(node.clone());
    }
    (rows, valid)
}
impl GitHubClient {
    pub(super) async fn collect_detail_checks(
        &self,
        v: &mut Value,
        owner: &str,
        name: &str,
        number: u64,
    ) -> ChecksCoverage {
        let head = v.pointer(PR).unwrap()["headRefOid"]
            .as_str()
            .unwrap()
            .to_string();
        let commit_matches = head_matches(v, number, &head);
        let unavailable = v["__checks_unavailable"] == true;
        let rollup = v.pointer(COMMIT).and_then(|c| c.get("statusCheckRollup"));
        if commit_matches && !unavailable && rollup == Some(&Value::Null) {
            return coverage(ChecksState::Complete, Some(0));
        }
        // A different/missing commit cannot contribute check facts for the viewed head.
        if !commit_matches {
            if let Some(contexts) = v.pointer_mut(CONTEXTS) {
                contexts["nodes"] = json!([]);
            }
            return coverage(ChecksState::Unknown, None);
        }
        let contexts = v.pointer(CONTEXTS).cloned().unwrap_or(Value::Null);
        let mut total = contexts["totalCount"].as_u64();
        let mut seen = HashSet::new();
        let (nodes, valid) = read_nodes(&contexts, &mut seen);
        if let Some(target) = v.pointer_mut(CONTEXTS) {
            target["nodes"] = json!(nodes);
        }
        let mut count = nodes.len() as u64;
        if unavailable || !valid || total.is_none() || total.is_some_and(|t| t < count) {
            return coverage(ChecksState::Unknown, None);
        }
        let mut info = contexts["pageInfo"].clone();
        let mut cursors = HashSet::new();
        let client = self.with_attempt_limit(3);
        for page_index in 0..=3 {
            match info["hasNextPage"].as_bool() {
                Some(false) if total == Some(count) => {
                    return coverage(ChecksState::Complete, total)
                }
                Some(false) | None => return coverage(ChecksState::Unknown, None),
                Some(true) => {}
            }
            if page_index == 3 {
                return coverage(ChecksState::Partial, total);
            }
            let Some(cursor) = info["endCursor"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 4096)
            else {
                return coverage(ChecksState::Unknown, None);
            };
            if !cursors.insert(cursor.to_string()) {
                return coverage(ChecksState::Unknown, None);
            }
            let page = match client.graphql_partial_ok(&json!({"query":PR_CHECKS_PAGE_QUERY,"variables":{"owner":owner,"repo":name,"number":number,"after":cursor}})).await {
                Ok(page) => page,
                Err(_) => return coverage(ChecksState::Partial, total),
            };
            if !head_matches(&page, number, &head) {
                return coverage(ChecksState::Unknown, None);
            }
            let fetched = page.pointer(CONTEXTS).unwrap_or(&Value::Null);
            let measured_total = fetched["totalCount"].as_u64();
            if measured_total.is_some() && measured_total != total {
                total = None;
            }
            let (more, valid) = read_nodes(fetched, &mut seen);
            count += more.len() as u64;
            if let Some(existing) = v
                .pointer_mut(CONTEXTS)
                .and_then(|c| c["nodes"].as_array_mut())
            {
                existing.extend(more);
            }
            if page["__checks_unavailable"] == true || !valid || measured_total.is_none() {
                return coverage(ChecksState::Partial, total);
            }
            if total.is_none() || total.is_some_and(|t| t < count) {
                return coverage(ChecksState::Unknown, None);
            }
            info = fetched["pageInfo"].clone();
        }
        unreachable!()
    }
}
