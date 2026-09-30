//! Bounded GitLab MR detail reads. Every optional endpoint has its own
//! receipt: a failed read cannot become zero approvals, zero discussions, or
//! green CI. No merge-readiness verdict is derived from these fields.
use crate::{
    identity::{PrIdentity, Provider},
    store::source_cache::Coverage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DETAIL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PAGES: usize = 5;
const PAGE_SIZE: usize = 100;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailIssue {
    MissingCli,
    UnsupportedHost,
    InvalidIdentity,
    Unauthorized,
    Forbidden,
    NotFound,
    RateLimited,
    /// The operation's budget expired before this request was issued.
    BudgetExhausted,
    Timeout,
    Request,
    InvalidResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReadState<T> {
    Available { value: T },
    Unavailable { issue: DetailIssue },
}

impl<T> From<Result<T, DetailIssue>> for ReadState<T> {
    fn from(result: Result<T, DetailIssue>) -> Self {
        match result {
            Ok(value) => Self::Available { value },
            Err(issue) => Self::Unavailable { issue },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub total: Option<u64>,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MrCore {
    pub identity: PrIdentity,
    pub id: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
    pub body: Option<String>,
    pub author: Option<String>,
    pub head_ref: String,
    pub head_oid: Option<String>,
    pub base_ref: String,
    /// GitLab's raw detailed status. Never translate it to GitHub Clean.
    pub detailed_merge_status: Option<String>,
    pub blocking_discussions_resolved: Option<bool>,
    /// A null or missing pipeline is unknown, not a successful check.
    pub head_pipeline: Option<Pipeline>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: u64,
    pub project_id: Option<u64>,
    pub sha: String,
    pub status: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub allow_failure: Option<bool>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineRead {
    /// MR pipeline list only. GitLab may have other pipeline sources; this
    /// is not an assertion that all checks on the commit were fetched.
    pub pipelines: ReadState<Paged<Pipeline>>,
    /// Jobs are fetched only when a pipeline with the current head SHA is
    /// identified. None means the request was not issued.
    pub current_head_jobs: Option<ReadState<Paged<Job>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approvals {
    pub required: Option<u64>,
    pub left: Option<u64>,
    /// An empty measured list means nobody approved; `approved: true` from
    /// GitLab can also mean zero approvals were required.
    pub approved_by: Vec<String>,
    pub user_has_approved: Option<bool>,
    pub user_can_approve: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRule {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub approved: Option<bool>,
    pub approvals_required: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub author: Option<String>,
    pub body: Option<String>,
    pub created_at: Option<String>,
    pub system: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscussionNote {
    pub comment: Comment,
    pub kind: Option<String>,
    pub resolvable: Option<bool>,
    pub resolved: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discussion {
    pub id: String,
    pub individual_note: Option<bool>,
    pub notes: Vec<DiscussionNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscussionRead {
    pub discussions: Paged<Discussion>,
    /// None when pagination or a note's resolution data is inconclusive.
    pub unresolved_resolvable: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRequestDetail {
    #[serde(default)]
    pub viewer: Option<String>,
    pub core: MrCore,
    pub pipelines: PipelineRead,
    pub approvals: ReadState<Approvals>,
    pub approval_rules: ReadState<Vec<ApprovalRule>>,
    pub comments: ReadState<Paged<Comment>>,
    pub discussions: ReadState<DiscussionRead>,
}

pub async fn fetch(identity: &PrIdentity) -> Result<MergeRequestDetail, DetailIssue> {
    validate_identity(identity)?;
    let program = super::auth::find_glab().ok_or(DetailIssue::MissingCli)?;
    let viewer = super::auth::viewer(&program, &identity.source.host).await?;
    static READS: std::sync::LazyLock<
        super::coalesce::Reads<Result<MergeRequestDetail, DetailIssue>>,
    > = std::sync::LazyLock::new(super::coalesce::Reads::default);
    let key = serde_json::to_string(&(identity, &viewer, super::queues::generation()))
        .expect("serializable identity");
    let mut detail = READS
        .run(key, fetch_with_program(&program, identity, DETAIL_TIMEOUT))
        .await?;
    if super::auth::viewer(&program, &identity.source.host).await? != viewer {
        return Err(DetailIssue::Unauthorized);
    }
    detail.viewer = Some(viewer);
    Ok(detail)
}

pub(super) fn validate_identity(identity: &PrIdentity) -> Result<(), DetailIssue> {
    if identity.source.provider != Provider::Gitlab
        || super::host::validate(&identity.source.host).is_err()
    {
        return Err(DetailIssue::UnsupportedHost);
    }
    if identity.number == 0
        || identity.repo.is_empty()
        || identity
            .repo
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(DetailIssue::InvalidIdentity);
    }
    Ok(())
}

async fn fetch_with_program(
    program: &Path,
    identity: &PrIdentity,
    budget: Duration,
) -> Result<MergeRequestDetail, DetailIssue> {
    validate_identity(identity)?;
    let started = tokio::time::Instant::now();
    let project = encode_project(&identity.repo);
    let base = format!("projects/{project}/merge_requests/{}", identity.number);
    let core_page = request(
        program,
        &identity.source.host,
        &base,
        remaining(started, budget)?,
    )
    .await?;
    let core = map_core(&core_page.body, identity).ok_or(DetailIssue::InvalidResponse)?;

    let pipelines = read_pages(
        program,
        &identity.source.host,
        &format!("{base}/pipelines"),
        started,
        budget,
        map_pipeline,
        |p: &Pipeline| p.id.to_string(),
    )
    .await;
    let current = match &pipelines {
        ReadState::Available { value } if value.coverage == Coverage::Complete => {
            core.head_oid.as_deref().and_then(|sha| {
                value
                    .items
                    .iter()
                    .filter(|p| p.sha == sha)
                    .max_by_key(|p| p.id)
                    .and_then(|p| Some((p.project_id?, p.id)))
            })
        }
        _ => None,
    };
    let current_head_jobs = if let Some((pipeline_project, id)) = current {
        Some(
            read_pages(
                program,
                &identity.source.host,
                &format!("projects/{pipeline_project}/pipelines/{id}/jobs"),
                started,
                budget,
                map_job,
                |j: &Job| j.id.to_string(),
            )
            .await,
        )
    } else {
        None
    };
    let approvals = read_one(
        program,
        &identity.source.host,
        &format!("{base}/approvals"),
        started,
        budget,
        map_approvals,
    )
    .await;
    let approval_rules = read_one(
        program,
        &identity.source.host,
        &format!("{base}/approval_state"),
        started,
        budget,
        map_approval_rules,
    )
    .await;
    let comments = read_pages(
        program,
        &identity.source.host,
        &format!("{base}/notes"),
        started,
        budget,
        map_comment,
        |n: &Comment| n.id.to_string(),
    )
    .await;
    let discussions = match read_pages(
        program,
        &identity.source.host,
        &format!("{base}/discussions"),
        started,
        budget,
        map_discussion,
        |d: &Discussion| d.id.clone(),
    )
    .await
    {
        ReadState::Available { value } => {
            let unresolved_resolvable = count_unresolved(&value);
            ReadState::Available {
                value: DiscussionRead {
                    discussions: value,
                    unresolved_resolvable,
                },
            }
        }
        ReadState::Unavailable { issue } => ReadState::Unavailable { issue },
    };
    Ok(MergeRequestDetail {
        viewer: None,
        core,
        pipelines: PipelineRead {
            pipelines,
            current_head_jobs,
        },
        approvals,
        approval_rules,
        comments,
        discussions,
    })
}

fn remaining(started: tokio::time::Instant, budget: Duration) -> Result<Duration, DetailIssue> {
    let left = budget.saturating_sub(started.elapsed());
    if left.is_zero() {
        Err(DetailIssue::BudgetExhausted)
    } else {
        Ok(left.min(REQUEST_TIMEOUT))
    }
}

async fn read_one<T>(
    program: &Path,
    host: &str,
    endpoint: &str,
    started: tokio::time::Instant,
    budget: Duration,
    mapper: fn(&Value) -> Option<T>,
) -> ReadState<T> {
    let result = async {
        let response = request(program, host, endpoint, remaining(started, budget)?).await?;
        mapper(&response.body).ok_or(DetailIssue::InvalidResponse)
    }
    .await;
    result.into()
}

async fn read_pages<T, K>(
    program: &Path,
    host: &str,
    endpoint: &str,
    started: tokio::time::Instant,
    budget: Duration,
    mapper: fn(&Value) -> Option<T>,
    key: K,
) -> ReadState<Paged<T>>
where
    K: Fn(&T) -> String,
{
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut total = None;
    let mut page_number = 1;
    let mut incomplete = false;
    let mut complete = false;
    for page_index in 0..MAX_PAGES {
        let timeout = match remaining(started, budget) {
            Ok(timeout) => timeout,
            Err(issue) if items.is_empty() && page_number == 1 => {
                return ReadState::Unavailable { issue }
            }
            Err(_) => {
                incomplete = true;
                break;
            }
        };
        let url = format!("{endpoint}?per_page={PAGE_SIZE}&page={page_number}");
        let response = match request(program, host, &url, timeout).await {
            Ok(response) => response,
            Err(issue) if page_number == 1 => return ReadState::Unavailable { issue },
            Err(_) => {
                incomplete = true;
                break;
            }
        };
        let Some(rows) = response.body.as_array() else {
            if page_number == 1 {
                return ReadState::Unavailable {
                    issue: DetailIssue::InvalidResponse,
                };
            }
            incomplete = true;
            break;
        };
        if let Some(n) = response.total {
            if total.is_some_and(|old| old != n) {
                total = None;
                incomplete = true;
            } else if !incomplete {
                total = Some(n);
            }
        }
        for row in rows {
            match mapper(row) {
                Some(item) if seen.insert(key(&item)) => items.push(item),
                _ => incomplete = true,
            }
        }
        if response.terminal_known && response.next.is_none() {
            complete = true;
            break;
        }
        if page_index + 1 == MAX_PAGES {
            incomplete = true;
            break;
        }
        match response.next {
            Some(next) if next > page_number => page_number = next,
            Some(_) => {
                incomplete = true;
                break;
            }
            None if rows.len() < PAGE_SIZE => break,
            None => page_number += 1,
        }
    }
    if total.is_some_and(|n| n < items.len() as u64) {
        total = None;
        incomplete = true;
    }
    let coverage = if incomplete || total.is_some_and(|n| n > items.len() as u64) {
        Coverage::Partial { total }
    } else if complete {
        Coverage::Complete
    } else {
        Coverage::Unknown
    };
    ReadState::Available {
        value: Paged {
            items,
            total,
            coverage,
        },
    }
}

pub(super) struct Response {
    pub(super) body: Value,
    total: Option<u64>,
    next: Option<usize>,
    terminal_known: bool,
}

async fn request(
    program: &Path,
    host: &str,
    endpoint: &str,
    timeout: Duration,
) -> Result<Response, DetailIssue> {
    request_json(program, host, endpoint, "GET", None, timeout).await
}

/// Shared bounded transport. Request bodies go through stdin, never process
/// arguments, and glab retains ownership of credentials and OAuth refresh.
pub(super) async fn request_json(
    program: &Path,
    host: &str,
    endpoint: &str,
    method: &str,
    body: Option<Value>,
    timeout: Duration,
) -> Result<Response, DetailIssue> {
    let mut command = super::host::constrained_command(program, host)
        .map_err(|_| DetailIssue::UnsupportedHost)?;
    command.args(["api", "--hostname", host, "-i", endpoint]);
    if method != "GET" {
        command.args(["--method", method]);
    }
    if body.is_some() {
        command.args(["--input", "-", "--header", "Content-Type: application/json"]);
    }
    command
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let operation = async {
        let _permit = super::transport::acquire(program).await;
        let mut child = command.spawn().map_err(|_| DetailIssue::Request)?;
        let stdout = child.stdout.take().ok_or(DetailIssue::Request)?;
        if let Some(body) = body {
            let bytes = serde_json::to_vec(&body).map_err(|_| DetailIssue::InvalidResponse)?;
            let mut stdin = child.stdin.take().ok_or(DetailIssue::Request)?;
            stdin
                .write_all(&bytes)
                .await
                .map_err(|_| DetailIssue::Request)?;
            stdin.shutdown().await.map_err(|_| DetailIssue::Request)?;
            drop(stdin);
        }
        let mut raw = Vec::new();
        stdout
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut raw)
            .await
            .map_err(|_| DetailIssue::Request)?;
        if raw.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(DetailIssue::InvalidResponse);
        }
        let status = child.wait().await.map_err(|_| DetailIssue::Request)?;
        parse_response(&raw, status.success(), endpoint == "graphql")
    };
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| DetailIssue::Timeout)?
}

fn parse_response(raw: &[u8], successful: bool, graphql: bool) -> Result<Response, DetailIssue> {
    let text = std::str::from_utf8(raw).map_err(|_| DetailIssue::InvalidResponse)?;
    let normalized = text.replace("\r\n", "\n");
    let status = normalized
        .lines()
        .filter(|line| line.starts_with("HTTP/"))
        .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u16>().ok())
        .next_back();
    match status {
        Some(401) => return Err(DetailIssue::Unauthorized),
        Some(403) => return Err(DetailIssue::Forbidden),
        Some(404) => return Err(DetailIssue::NotFound),
        Some(429) => return Err(DetailIssue::RateLimited),
        None if !successful => return Err(DetailIssue::Request),
        Some(code) if !(200..300).contains(&code) => return Err(DetailIssue::Request),
        None => return Err(DetailIssue::InvalidResponse),
        _ => {}
    }
    let (headers, body) = normalized
        .rsplit_once("\n\n")
        .ok_or(DetailIssue::InvalidResponse)?;
    let headers = headers
        .rsplit_once("\n\n")
        .map_or(headers, |(_, last)| last);
    let mut total = None;
    let mut next = None;
    let mut terminal_known = false;
    for line in headers.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            if key.eq_ignore_ascii_case("x-total") {
                total = value.parse().ok();
            }
            if key.eq_ignore_ascii_case("x-next-page") {
                terminal_known = true;
                if !value.is_empty() {
                    next = Some(value.parse().map_err(|_| DetailIssue::InvalidResponse)?);
                }
            }
        }
    }
    let body: Value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body.trim()).map_err(|_| DetailIssue::InvalidResponse)?
    };
    // glab returns exit 1 for HTTP status 200 GraphQL errors. Preserve the typed
    // error document so callers can distinguish unsupported schema from
    // authorization failures; a nonzero REST response is still a failure.
    if !successful
        && !(graphql
            && body
                .get("errors")
                .and_then(Value::as_array)
                .is_some_and(|errors| !errors.is_empty()))
    {
        return Err(DetailIssue::Request);
    }
    Ok(Response {
        body,
        total,
        next,
        terminal_known,
    })
}

pub(super) fn encode_project(repo: &str) -> String {
    let mut out = String::new();
    for byte in repo.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(super) fn map_core(v: &Value, identity: &PrIdentity) -> Option<MrCore> {
    let number = v.get("iid")?.as_u64()?;
    if number != identity.number {
        return None;
    }
    let url = v.get("web_url")?.as_str()?;
    let expected = format!(
        "https://{}/{}/-/merge_requests/{number}",
        identity.source.host, identity.repo
    );
    if url != expected {
        return None;
    }
    let head_oid = v.get("sha").and_then(Value::as_str).map(str::to_owned);
    let head_pipeline = v
        .get("head_pipeline")
        .filter(|p| !p.is_null())
        .and_then(map_pipeline)
        .filter(|pipeline| Some(pipeline.sha.as_str()) == head_oid.as_deref());
    Some(MrCore {
        identity: identity.clone(),
        id: v.get("id")?.as_u64()?,
        title: v.get("title")?.as_str()?.to_owned(),
        url: url.to_owned(),
        state: v.get("state")?.as_str()?.to_owned(),
        is_draft: v.get("draft")?.as_bool()?,
        body: v
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        author: v
            .get("author")
            .and_then(|a| a.get("username"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        head_ref: v.get("source_branch")?.as_str()?.to_owned(),
        head_oid,
        base_ref: v.get("target_branch")?.as_str()?.to_owned(),
        detailed_merge_status: v
            .get("detailed_merge_status")
            .and_then(Value::as_str)
            .map(str::to_owned),
        blocking_discussions_resolved: v
            .get("blocking_discussions_resolved")
            .and_then(Value::as_bool),
        head_pipeline,
    })
}

fn map_pipeline(v: &Value) -> Option<Pipeline> {
    Some(Pipeline {
        id: v.get("id")?.as_u64()?,
        project_id: v.get("project_id").and_then(Value::as_u64),
        sha: v.get("sha")?.as_str()?.to_owned(),
        status: v.get("status")?.as_str()?.to_owned(),
        url: v.get("web_url").and_then(Value::as_str).map(str::to_owned),
    })
}

fn map_job(v: &Value) -> Option<Job> {
    Some(Job {
        id: v.get("id")?.as_u64()?,
        name: v.get("name")?.as_str()?.to_owned(),
        status: v.get("status")?.as_str()?.to_owned(),
        allow_failure: v.get("allow_failure").and_then(Value::as_bool),
        url: v.get("web_url").and_then(Value::as_str).map(str::to_owned),
    })
}

fn map_approvals(v: &Value) -> Option<Approvals> {
    let approved_by = v
        .get("approved_by")?
        .as_array()?
        .iter()
        .map(|entry| Some(entry.get("user")?.get("username")?.as_str()?.to_owned()))
        .collect::<Option<Vec<_>>>()?;
    Some(Approvals {
        required: v.get("approvals_required").and_then(Value::as_u64),
        left: v.get("approvals_left").and_then(Value::as_u64),
        approved_by,
        user_has_approved: v.get("user_has_approved").and_then(Value::as_bool),
        user_can_approve: v.get("user_can_approve").and_then(Value::as_bool),
    })
}

fn map_approval_rules(v: &Value) -> Option<Vec<ApprovalRule>> {
    v.get("rules")?
        .as_array()?
        .iter()
        .map(|rule| {
            Some(ApprovalRule {
                id: rule.get("id").and_then(Value::as_u64),
                name: rule.get("name").and_then(Value::as_str).map(str::to_owned),
                approved: rule.get("approved").and_then(Value::as_bool),
                approvals_required: rule.get("approvals_required").and_then(Value::as_u64),
            })
        })
        .collect()
}

fn map_comment(v: &Value) -> Option<Comment> {
    Some(Comment {
        id: v.get("id")?.as_u64()?,
        author: v
            .get("author")
            .and_then(|a| a.get("username"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        body: v.get("body").and_then(Value::as_str).map(str::to_owned),
        created_at: v
            .get("created_at")
            .and_then(Value::as_str)
            .map(str::to_owned),
        system: v.get("system").and_then(Value::as_bool),
    })
}

fn map_discussion(v: &Value) -> Option<Discussion> {
    let notes = v
        .get("notes")?
        .as_array()?
        .iter()
        .map(|note| {
            Some(DiscussionNote {
                comment: map_comment(note)?,
                kind: note.get("type").and_then(Value::as_str).map(str::to_owned),
                resolvable: note.get("resolvable").and_then(Value::as_bool),
                resolved: note.get("resolved").and_then(Value::as_bool),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Discussion {
        id: v.get("id")?.as_str()?.to_owned(),
        individual_note: v.get("individual_note").and_then(Value::as_bool),
        notes,
    })
}

fn count_unresolved(page: &Paged<Discussion>) -> Option<u64> {
    if page.coverage != Coverage::Complete {
        return None;
    }
    let mut count = 0;
    for discussion in &page.items {
        let mut unresolved = false;
        for note in &discussion.notes {
            match (note.resolvable, note.resolved) {
                (Some(true), Some(false)) => unresolved = true,
                (Some(true), Some(true)) | (Some(false), _) => {}
                _ => return None,
            }
        }
        if unresolved {
            count += 1;
        }
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Provider, Source};
    use serde_json::json;

    fn identity(host: &str) -> PrIdentity {
        PrIdentity {
            source: Source {
                provider: Provider::Gitlab,
                host: host.into(),
            },
            repo: "group/subgroup/project".into(),
            number: 7,
        }
    }

    fn core() -> Value {
        json!({
            "id": 481, "iid": 7, "title": "Synthetic change", "state": "opened",
            "web_url": "https://gitlab.com/group/subgroup/project/-/merge_requests/7",
            "draft": true, "description": "Body", "author": {"username":"fixture"},
            "source_branch": "topic", "target_branch": "main", "sha": "abc123",
            "detailed_merge_status": "draft_status", "merge_status": "can_be_merged",
            "blocking_discussions_resolved": true, "head_pipeline": null
        })
    }

    #[test]
    fn core_does_not_promote_coarse_merge_status_or_absent_ci() {
        let mapped = map_core(&core(), &identity("gitlab.com")).unwrap();
        assert_eq!(
            mapped.detailed_merge_status.as_deref(),
            Some("draft_status")
        );
        assert!(mapped.is_draft);
        assert_eq!(mapped.head_pipeline, None);
        assert_eq!(mapped.identity, identity("gitlab.com"));
        let mut wrong = core();
        wrong["web_url"] = json!("https://other.example/group/subgroup/project/-/merge_requests/7");
        assert!(map_core(&wrong, &identity("gitlab.com")).is_none());
        wrong = core();
        wrong["iid"] = json!(8);
        assert!(map_core(&wrong, &identity("gitlab.com")).is_none());
        assert_eq!(validate_identity(&identity("self.example")), Ok(()));
        assert_eq!(
            validate_identity(&identity("self.example/path")),
            Err(DetailIssue::UnsupportedHost)
        );
        assert_eq!(
            encode_project("group/subgroup/project"),
            "group%2Fsubgroup%2Fproject"
        );
    }

    #[test]
    fn zero_required_approvals_do_not_become_a_review_verdict() {
        let approvals = map_approvals(&json!({
            "approved": true, "approvals_required": 0, "approvals_left": 0,
            "approved_by": [], "user_has_approved": false, "user_can_approve": false
        }))
        .unwrap();
        assert_eq!(approvals.approved_by.len(), 0);
        assert_eq!(approvals.required, Some(0));
        assert_eq!(approvals.user_has_approved, Some(false));
        assert!(map_approvals(&json!({"approved": true})).is_none());
    }

    #[test]
    fn resolvable_discussions_require_complete_pages_and_known_note_flags() {
        let resolved = map_discussion(&json!({"id":"thread-a", "individual_note":false,
            "notes":[{"id":1,"type":"DiffNote","resolvable":true,"resolved":true}]}))
        .unwrap();
        let open = map_discussion(&json!({"id":"thread-b", "individual_note":false,
            "notes":[{"id":2,"type":"DiffNote","resolvable":true,"resolved":false},
                     {"id":3,"type":null,"resolvable":false,"resolved":null}]}))
        .unwrap();
        let page = Paged {
            items: vec![resolved, open],
            total: Some(2),
            coverage: Coverage::Complete,
        };
        assert_eq!(count_unresolved(&page), Some(1));
        let mut partial = page.clone();
        partial.coverage = Coverage::Partial { total: Some(3) };
        assert_eq!(count_unresolved(&partial), None);
        let mut unknown = page;
        unknown.items[1].notes[0].resolved = None;
        assert_eq!(count_unresolved(&unknown), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn graphql_schema_errors_survive_glab_nonzero_exit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        let body = json!({"errors":[{"extensions":{"code":"undefinedField","fieldName":"mergeTrainCar"}}]});
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\ncat >/dev/null\nprintf 'HTTP/2 200\\n\\n%s' '{}'\nexit 1\n",
                body
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(&program, async {
            let response = request_json(
                &program,
                "gitlab.com",
                "graphql",
                "POST",
                Some(json!({"query":"query { currentUser { username } }"})),
                Duration::from_secs(15),
            )
            .await
            .expect("GraphQL schema errors must reach capability fallback");
            assert_eq!(response.body, body);
            assert!(matches!(
                request_json(
                    &program,
                    "gitlab.com",
                    "projects/1",
                    "GET",
                    None,
                    Duration::from_secs(15)
                )
                .await,
                Err(DetailIssue::Request)
            ));
        })
        .await;
    }

    #[test]
    fn response_status_and_headers_are_typed_without_exposing_body() {
        let response = parse_response(
            b"HTTP/2 200\r\nx-total: 0\r\nx-next-page:\r\n\r\n[]",
            true,
            false,
        )
        .unwrap();
        assert_eq!(response.total, Some(0));
        assert!(response.terminal_known);
        assert_eq!(response.body, json!([]));
        assert!(matches!(
            parse_response(b"HTTP/2 401\n\n{\"token\":\"secret\"}", false, false),
            Err(DetailIssue::Unauthorized)
        ));
        assert!(matches!(
            parse_response(b"HTTP/2 429\n\n{}", false, false),
            Err(DetailIssue::RateLimited)
        ));
        assert!(matches!(
            parse_response(b"HTTP/2 404\n\n{}", false, false),
            Err(DetailIssue::NotFound)
        ));
        assert!(matches!(
            parse_response(b"secret", false, false),
            Err(DetailIssue::Request)
        ));
        assert_eq!(
            remaining(tokio::time::Instant::now(), Duration::ZERO),
            Err(DetailIssue::BudgetExhausted)
        );
    }

    #[cfg(unix)]
    async fn scripted(script: &str, budget: Duration) -> Result<MergeRequestDetail, DetailIssue> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("glab");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::gitlab::test_support::scripted(
            &program,
            fetch_with_program(&program, &identity("gitlab.com"), budget),
        )
        .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn detail_keeps_independent_receipts_and_current_head_jobs() {
        let detail = core();
        let pipeline = json!([{"id":11,"project_id":15,"sha":"abc123","status":"success","web_url":"https://gitlab.com/pipeline/11"}]);
        let jobs = json!([{"id":21,"name":"test","status":"success","allow_failure":false}]);
        let approvals = json!({"approved":true,"approvals_required":0,"approvals_left":0,"approved_by":[],"user_has_approved":false});
        let discussions = json!([{"id":"thread","individual_note":false,"notes":[{"id":30,"body":"fixed","type":"DiffNote","resolvable":true,"resolved":true}]}]);
        let script = format!(
            r#"
test "$1 $2 $3" = 'api --hostname gitlab.com' && test "$4" = '-i' || exit 90
case "$5" in
  projects/group%2Fsubgroup%2Fproject/merge_requests/7) printf 'HTTP/2 200\n\n%s' '{detail}' ;;
  */merge_requests/7/pipelines*) printf 'HTTP/2 200\nx-total: 1\nx-next-page:\n\n%s' '{pipeline}' ;;
  projects/15/pipelines/11/jobs*) printf 'HTTP/2 200\nx-total: 1\nx-next-page:\n\n%s' '{jobs}' ;;
  */merge_requests/7/approvals) printf 'HTTP/2 200\n\n%s' '{approvals}' ;;
  */merge_requests/7/approval_state) printf 'HTTP/2 403\n\n{{}}'; exit 1 ;;
  */merge_requests/7/notes*) printf 'HTTP/2 200\nx-total: 0\nx-next-page:\n\n[]' ;;
  */merge_requests/7/discussions*) printf 'HTTP/2 200\nx-total: 1\nx-next-page:\n\n%s' '{discussions}' ;;
  *) exit 91 ;;
esac
"#
        );
        let receipt = scripted(&script, Duration::from_secs(5)).await.unwrap();
        assert!(matches!(
            receipt.pipelines.pipelines,
            ReadState::Available { .. }
        ));
        assert!(matches!(
            receipt.pipelines.current_head_jobs,
            Some(ReadState::Available { .. })
        ));
        assert!(matches!(
            receipt.approval_rules,
            ReadState::Unavailable {
                issue: DetailIssue::Forbidden
            }
        ));
        assert!(matches!(receipt.approvals, ReadState::Available { .. }));
        assert!(matches!(
            receipt.comments,
            ReadState::Available {
                value: Paged {
                    total: Some(0),
                    coverage: Coverage::Complete,
                    ..
                }
            }
        ));
        assert!(matches!(
            receipt.discussions,
            ReadState::Available {
                value: DiscussionRead {
                    unresolved_resolvable: Some(0),
                    ..
                }
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn later_page_timeout_retains_comments_as_partial() {
        let detail = core();
        let comments = json!([{"id":1,"body":"first"}]);
        let script = format!(
            r#"
case "$5" in
  */merge_requests/7) printf 'HTTP/2 200\n\n%s' '{detail}' ;;
  */notes*page=1) printf 'HTTP/2 200\nx-total: 2\nx-next-page: 2\n\n%s' '{comments}' ;;
  */notes*page=2) touch "$0.timeout"; exec sleep 60 ;;
  *) printf 'HTTP/2 200\nx-total: 0\nx-next-page:\n\n[]' ;;
esac
"#
        );
        let receipt = scripted(&script, Duration::from_millis(300)).await.unwrap();
        assert!(matches!(
            receipt.comments,
            ReadState::Available {
                value: Paged {
                    total: Some(2),
                    coverage: Coverage::Partial { total: Some(2) },
                    ..
                }
            }
        ));
        if let ReadState::Available { value } = receipt.comments {
            assert_eq!(value.items.len(), 1);
        }
    }
}
