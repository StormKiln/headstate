//! GitLab writes require a fresh permission receipt for the exact MR. A 2xx
//! mutation response is never the success receipt: each write has a readback.
use super::detail::{self, DetailIssue, MrCore};
use crate::identity::PrIdentity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

const BUDGET: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_JOBS: &str = "/jobs?include_retried=false&per_page=100&page=1";
const LEGACY_QUERY: &str = "query($path: ID!, $iid: String!) { currentUser { username } project(fullPath: $path) { mergeRequest(iid: $iid) { iid webUrl diffHeadSha rebaseCommitSha autoMergeEnabled availableAutoMergeStrategies userPermissions { canApprove canMerge createNote updateMergeRequest pushToSourceBranch } headPipeline { id sha retryable userPermissions { updatePipeline } } discussions(first: 100) { nodes { replyId resolvable resolved userPermissions { resolveNote } notes(first: 1) { nodes { system userPermissions { createNote } } } } pageInfo { hasNextPage } } } } }";
const QUERY: &str = "query($path: ID!, $iid: String!) { currentUser { username } project(fullPath: $path) { mergeRequest(iid: $iid) { iid webUrl diffHeadSha changeRequesters(first: 100) { nodes { username } pageInfo { hasNextPage } } mergeTrainCar { index } autoMergeStrategy rebaseCommitSha autoMergeEnabled availableAutoMergeStrategies userPermissions { canApprove canMerge createNote updateMergeRequest pushToSourceBranch } headPipeline { id sha retryable userPermissions { updatePipeline } } discussions(first: 100) { nodes { replyId resolvable resolved userPermissions { resolveNote } notes(first: 1) { nodes { system userPermissions { createNote } } } } pageInfo { hasNextPage } } } } }";

// Older self-managed schemas may lack optional CI/auto-merge/discussion
// fields. A schema error there must not remove basic read/write permission.
const CORE_QUERY: &str = "query($path: ID!, $iid: String!) { currentUser { username } project(fullPath: $path) { mergeRequest(iid: $iid) { iid webUrl diffHeadSha userPermissions { canMerge createNote updateMergeRequest } } } }";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Approve,
    RequestChanges,
    EnqueueTrain,
    Comment,
    Reply,
    Resolve,
    Unresolve,
    Merge,
    Close,
    Reopen,
    Draft,
    Ready,
    Rebase,
    RetryCi,
    EnableAutoMerge,
    DisableAutoMerge,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub action: Action,
    pub allowed: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscussionCapability {
    pub id: String,
    pub can_reply: bool,
    pub can_resolve: bool,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub viewer: Option<String>,
    pub identity: PrIdentity,
    pub head_oid: Option<String>,
    pub actions: Vec<Capability>,
    pub discussions: Vec<DiscussionCapability>,
    pub discussions_complete: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionRequest {
    pub expected_viewer: Option<String>,
    pub identity: PrIdentity,
    pub action: Action,
    pub expected_head: Option<String>,
    pub body: Option<String>,
    pub discussion_id: Option<String>,
}

/// Unverified is deliberately a resolved receipt, not a retryable error. The
/// write may have landed, so neither desktop nor phone should repeat it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Verified,
    Unverified,
}

#[derive(Debug, Clone, Serialize)]
pub struct Receipt {
    pub identity: PrIdentity,
    pub action: Action,
    pub outcome: Outcome,
    pub message: String,
}

struct Context {
    core: MrCore,
    raw: Value,
    permissions: Value,
    capabilities: Capabilities,
}

struct Session<'a> {
    program: &'a Path,
    identity: &'a PrIdentity,
    start: tokio::time::Instant,
}

impl Session<'_> {
    fn base(&self) -> String {
        format!(
            "projects/{}/merge_requests/{}",
            detail::encode_project(&self.identity.repo),
            self.identity.number
        )
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, DetailIssue> {
        let timeout = BUDGET
            .checked_sub(self.start.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or(DetailIssue::BudgetExhausted)?;
        detail::request_json(
            self.program,
            &self.identity.source.host,
            path,
            method,
            body,
            timeout.min(REQUEST_TIMEOUT),
        )
        .await
        .map(|r| r.body)
    }

    async fn core(&self) -> Result<Value, DetailIssue> {
        let raw = self
            .request(
                "GET",
                &format!("{}?include_rebase_in_progress=true", self.base()),
                None,
            )
            .await?;
        detail::map_core(&raw, self.identity).ok_or(DetailIssue::InvalidResponse)?;
        Ok(raw)
    }

    async fn context(&self) -> Result<Context, DetailIssue> {
        let raw = self.core().await?;
        let core = detail::map_core(&raw, self.identity).ok_or(DetailIssue::InvalidResponse)?;
        let mut response = Value::Null;
        for query in [QUERY, LEGACY_QUERY, CORE_QUERY] {
            response = self.request("POST", "graphql", Some(json!({
                "query": query, "variables": {"path": self.identity.repo, "iid": self.identity.number.to_string()}
            }))).await?;
            let missing_field = response
                .get("errors")
                .and_then(Value::as_array)
                .is_some_and(|errors| {
                    !errors.is_empty()
                        && errors.iter().all(|error| {
                            error.pointer("/extensions/code").and_then(Value::as_str)
                                == Some("undefinedField")
                        })
                });
            if !missing_field {
                break;
            }
        }
        if response
            .get("errors")
            .is_some_and(|e| !e.as_array().is_some_and(Vec::is_empty))
        {
            return Err(DetailIssue::InvalidResponse);
        }
        let mut permissions = response
            .pointer("/data/project/mergeRequest")
            .cloned()
            .ok_or(DetailIssue::InvalidResponse)?;
        if permissions["iid"].as_str() != Some(self.identity.number.to_string().as_str())
            || permissions["webUrl"].as_str() != Some(core.url.as_str())
            || permissions["diffHeadSha"].as_str() != core.head_oid.as_deref()
        {
            return Err(DetailIssue::InvalidResponse);
        }
        permissions["viewer"] = response
            .pointer("/data/currentUser/username")
            .cloned()
            .unwrap_or(Value::Null);
        let mut raw = raw;
        raw["action_approvals"] = self
            .request("GET", &format!("{}/approvals", self.base()), None)
            .await
            .unwrap_or(Value::Null);
        let capabilities = capabilities_from(&core, &raw, &permissions);
        Ok(Context {
            core,
            raw,
            permissions,
            capabilities,
        })
    }
}

fn yes(v: &Value, field: &str) -> bool {
    v[field].as_bool() == Some(true)
}

fn discussion_id(global: &str) -> Option<&str> {
    let tail = global.strip_prefix("gid://gitlab/")?;
    let (kind, id) = tail.split_once('/')?;
    if !matches!(
        kind,
        "Discussion" | "DiffDiscussion" | "IndividualNoteDiscussion"
    ) || id.len() != 40
        || !id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(id)
}

fn pipeline_id(p: &Value) -> Option<u64> {
    p["id"]
        .as_str()?
        .strip_prefix("gid://gitlab/Ci::Pipeline/")?
        .parse()
        .ok()
}

fn capabilities_from(core: &MrCore, raw: &Value, data: &Value) -> Capabilities {
    let p = &data["userPermissions"];
    let opened = core.state == "opened";
    let mutable = yes(p, "updateMergeRequest");
    let head_known = core.head_oid.as_ref().is_some_and(|s| !s.is_empty());
    let auto = data["autoMergeEnabled"].as_bool();
    let auto_available =
        data["availableAutoMergeStrategies"]
            .as_array()
            .is_some_and(|strategies| {
                strategies
                    .iter()
                    .any(|s| s.as_str() == Some("merge_when_checks_pass"))
            });
    let pipeline = &data["headPipeline"];
    let retryable = yes(pipeline, "retryable")
        && yes(&pipeline["userPermissions"], "updatePipeline")
        && pipeline["sha"].as_str() == core.head_oid.as_deref()
        && pipeline_id(pipeline).is_some()
        && raw.pointer("/head_pipeline/id").and_then(Value::as_u64) == pipeline_id(pipeline)
        && raw
            .pointer("/head_pipeline/project_id")
            .and_then(Value::as_u64)
            .is_some();
    let mut actions: Vec<Capability> = [
        (
            Action::Approve,
            opened
                && head_known
                && yes(p, "canApprove")
                && yes(&raw["action_approvals"], "user_can_approve")
                && !yes(&raw["action_approvals"], "user_has_approved")
                && !yes(&raw["action_approvals"], "require_password_to_approve")
                && !yes(
                    &raw["action_approvals"],
                    "require_reauthentication_to_approve",
                ),
            "GitLab has not granted approval permission for this MR.",
        ),
        (
            Action::RequestChanges,
            opened
                && head_known
                && mutable
                && data["viewer"].as_str().is_some()
                && core.author.is_some()
                && data["viewer"].as_str() != core.author.as_deref()
                && data["changeRequesters"].is_object(),
            "Requesting changes requires an open MR, a current account, and update permission.",
        ),
        (
            Action::EnqueueTrain,
            opened
                && head_known
                && !core.is_draft
                && yes(p, "canMerge")
                && data.get("mergeTrainCar").is_some_and(Value::is_null)
                && data["availableAutoMergeStrategies"]
                    .as_array()
                    .is_some_and(|strategies| {
                        strategies.iter().any(|s| {
                            matches!(
                                s.as_str(),
                                Some("merge_train" | "add_to_merge_train_when_checks_pass")
                            )
                        })
                    }),
            "GitLab has not confirmed merge-train availability and merge permission for this MR.",
        ),
        (
            Action::Comment,
            yes(p, "createNote"),
            "GitLab has not granted comment permission for this MR.",
        ),
        (
            Action::Reply,
            yes(p, "createNote"),
            "GitLab has not granted comment permission for this MR.",
        ),
        (
            Action::Resolve,
            true,
            "Discussion permission is checked separately.",
        ),
        (
            Action::Unresolve,
            true,
            "Discussion permission is checked separately.",
        ),
        (
            Action::Merge,
            opened
                && head_known
                && !core.is_draft
                && yes(p, "canMerge")
                && core.detailed_merge_status.as_deref() == Some("mergeable"),
            "GitLab has not confirmed this MR can be merged now.",
        ),
        (
            Action::Close,
            opened && mutable,
            "Closing requires an open MR and update permission.",
        ),
        (
            Action::Reopen,
            core.state == "closed" && mutable,
            "Reopening requires a closed MR and update permission.",
        ),
        (
            Action::Draft,
            opened && !core.is_draft && mutable,
            "Marking draft requires an open ready MR and update permission.",
        ),
        (
            Action::Ready,
            opened && core.is_draft && mutable,
            "Marking ready requires an open draft MR and update permission.",
        ),
        (
            Action::Rebase,
            opened
                && head_known
                && yes(p, "pushToSourceBranch")
                && raw["rebase_in_progress"].as_bool() == Some(false),
            "Rebase requires source branch permission and known idle rebase state.",
        ),
        (
            Action::RetryCi,
            opened && head_known && retryable,
            "No retryable current-head pipeline with confirmed permission is available.",
        ),
        (
            Action::EnableAutoMerge,
            opened
                && head_known
                && !core.is_draft
                && yes(p, "canMerge")
                && auto == Some(false)
                && auto_available,
            "GitLab has not offered supported auto-merge for this MR.",
        ),
        (
            Action::DisableAutoMerge,
            opened && yes(p, "canMerge") && auto == Some(true),
            "Canceling auto-merge requires active auto-merge and merge permission.",
        ),
    ]
    .into_iter()
    .map(|(action, allowed, reason)| Capability {
        action,
        allowed,
        reason: (!allowed).then(|| reason.to_owned()),
    })
    .collect();
    let discussions: Vec<DiscussionCapability> = data
        .pointer("/discussions/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(DiscussionCapability {
                id: discussion_id(d["replyId"].as_str()?)?.to_owned(),
                can_reply: yes(p, "createNote")
                    && d.pointer("/notes/nodes/0/system").and_then(Value::as_bool) == Some(false)
                    && d.pointer("/notes/nodes/0/userPermissions/createNote")
                        .and_then(Value::as_bool)
                        == Some(true),
                can_resolve: yes(d, "resolvable") && yes(&d["userPermissions"], "resolveNote"),
                resolved: d["resolved"].as_bool()?,
            })
        })
        .collect();
    for cap in &mut actions {
        if cap.action == Action::Reply {
            cap.allowed = discussions.iter().any(|d| d.can_reply);
            cap.reason = (!cap.allowed)
                .then(|| "No discussion with confirmed reply permission is available.".into());
        }
        if matches!(cap.action, Action::Resolve | Action::Unresolve) {
            cap.allowed = discussions
                .iter()
                .any(|d| d.can_resolve && d.resolved == (cap.action == Action::Unresolve));
            cap.reason = (!cap.allowed)
                .then(|| "No discussion with confirmed resolution permission is available.".into());
        }
    }
    Capabilities {
        viewer: data["viewer"].as_str().map(str::to_owned),
        identity: core.identity.clone(),
        head_oid: core.head_oid.clone(),
        actions,
        discussions,
        discussions_complete: data
            .pointer("/discussions/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            == Some(false),
    }
}

pub async fn capabilities(identity: &PrIdentity) -> Result<Capabilities, String> {
    detail::validate_identity(identity).map_err(read_error)?;
    let program = super::auth::find_glab().ok_or_else(|| read_error(DetailIssue::MissingCli))?;
    let session = Session {
        program: &program,
        identity,
        start: tokio::time::Instant::now(),
    };
    let viewer = super::auth::viewer(&program, &identity.source.host)
        .await
        .map_err(read_error)?;
    static READS: std::sync::LazyLock<super::coalesce::Reads<Result<Capabilities, String>>> =
        std::sync::LazyLock::new(super::coalesce::Reads::default);
    let key = serde_json::to_string(&(identity, &viewer, super::queues::generation()))
        .expect("serializable identity");
    READS
        .run(key, async {
            let capabilities = session.context().await.map_err(read_error)?.capabilities;
            if capabilities.viewer.as_ref() != Some(&viewer) {
                return Err("GitLab account changed while checking actions.".into());
            }
            Ok(capabilities)
        })
        .await
}

fn read_error(issue: DetailIssue) -> String {
    format!("GitLab action checks unavailable ({issue:?}).")
}

pub async fn execute(request: &ActionRequest) -> Result<Receipt, String> {
    detail::validate_identity(&request.identity).map_err(read_error)?;
    let program = super::auth::find_glab().ok_or_else(|| read_error(DetailIssue::MissingCli))?;
    execute_with_program(&program, request).await
}

async fn execute_with_program(program: &Path, request: &ActionRequest) -> Result<Receipt, String> {
    detail::validate_identity(&request.identity).map_err(read_error)?;
    let session = Session {
        program,
        identity: &request.identity,
        start: tokio::time::Instant::now(),
    };
    let context = session.context().await.map_err(read_error)?;
    if request
        .expected_viewer
        .as_ref()
        .is_some_and(|viewer| context.capabilities.viewer.as_ref() != Some(viewer))
    {
        return Err("The GitLab account changed. Refresh before acting.".into());
    }
    let write = prepare(&session.base(), request, &context)?;
    let retry_before = if request.action == Action::RetryCi {
        let path = pipeline_path(&context, RETRY_JOBS)?;
        // Losing the optional proof must not turn an authorized retry into
        // a false success. We can still write, but its receipt stays unverified.
        let jobs = session.request("GET", &path, None).await.ok();
        let before = jobs.as_ref().and_then(|v| retry_jobs(v, &context));
        // Reading jobs adds latency: recheck the selected head/pipeline just
        // before writing. GitLab's retry endpoint has no atomic SHA guard.
        let fresh = session.core().await.map_err(read_error)?;
        if fresh["sha"].as_str() != request.expected_head.as_deref()
            || !same_pipeline(&fresh["head_pipeline"], &context)
        {
            return Err("The MR head or pipeline changed. Refresh before acting.".into());
        }
        before
    } else {
        None
    };
    if request.action == Action::RequestChanges {
        let fresh = session.core().await.map_err(read_error)?;
        if fresh["sha"].as_str() != request.expected_head.as_deref() {
            return Err("The MR head changed. Refresh before requesting changes.".into());
        }
    }
    let response = session
        .request(write.method, &write.path, Some(write.body))
        .await;
    let verified = match response {
        Ok(response) => verify(
            &session,
            request,
            &context,
            response,
            retry_before.as_deref(),
        )
        .await
        .unwrap_or(false),
        Err(_) => false,
    };
    Ok(Receipt { identity: request.identity.clone(), action: request.action,
        outcome: if verified { Outcome::Verified } else { Outcome::Unverified },
        message: if verified && request.action == Action::RetryCi {
            "At least one failed or canceled CI job has a new attempt. Refresh GitLab to check the other jobs."
        } else if verified { "GitLab action verified." } else { "The GitLab action could not be verified. Refresh the MR on GitLab before trying again; it may already have applied." }.into() })
}

struct Write {
    method: &'static str,
    path: String,
    body: Value,
}

fn prepare(base: &str, request: &ActionRequest, context: &Context) -> Result<Write, String> {
    let cap = context
        .capabilities
        .actions
        .iter()
        .find(|c| c.action == request.action)
        .ok_or("Unsupported GitLab action.")?;
    if !cap.allowed {
        return Err(cap
            .reason
            .clone()
            .unwrap_or_else(|| "GitLab action unavailable.".into()));
    }
    if matches!(
        request.action,
        Action::Approve
            | Action::RequestChanges
            | Action::EnqueueTrain
            | Action::Merge
            | Action::Rebase
            | Action::RetryCi
            | Action::EnableAutoMerge
    ) && (request
        .expected_head
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_none()
        || request.expected_head != context.core.head_oid)
    {
        return Err("The MR head changed or was not supplied. Refresh before acting.".into());
    }
    let body = if matches!(request.action, Action::Comment | Action::Reply) {
        let text = request
            .body
            .as_deref()
            .filter(|s| !s.trim().is_empty() && s.len() <= 32_768)
            .ok_or("A comment of 1–32768 bytes is required.")?;
        // GitLab interprets quick actions as additional writes. A comment must
        // not bypass the merge/approval/permission gates through slash commands.
        if text.lines().any(|line| line.trim_start().starts_with('/')) {
            return Err(
                "GitLab quick actions are not supported here. Remove lines beginning with '/'."
                    .into(),
            );
        }
        Some(text)
    } else {
        None
    };
    let discussion = if matches!(
        request.action,
        Action::Reply | Action::Resolve | Action::Unresolve
    ) {
        let id = request
            .discussion_id
            .as_deref()
            .filter(|id| {
                !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric())
            })
            .ok_or("A valid GitLab discussion ID is required.")?;
        let cap = context.capabilities.discussions.iter().find(|d| d.id == id).ok_or("Discussion permission is unavailable in the bounded permission read. Open it on GitLab.")?;
        if request.action == Action::Reply && !cap.can_reply {
            return Err("GitLab has not granted permission to reply to this discussion.".into());
        }
        if matches!(request.action, Action::Resolve | Action::Unresolve) && !cap.can_resolve {
            return Err("GitLab has not granted permission to resolve this discussion.".into());
        }
        Some(id)
    } else {
        None
    };
    let (method, path, payload) = match request.action {
        Action::Approve => (
            "POST",
            format!("{base}/approve"),
            json!({"sha": request.expected_head}),
        ),
        Action::RequestChanges => (
            "POST",
            "graphql".into(),
            json!({"query":"mutation($input: MergeRequestRequestChangesInput!) { mergeRequestRequestChanges(input: $input) { errors mergeRequest { iid } } }", "variables":{"input":{"projectPath":request.identity.repo,"iid":request.identity.number.to_string()}}}),
        ),
        Action::EnqueueTrain => (
            "POST",
            format!(
                "projects/{}/merge_trains/merge_requests/{}",
                detail::encode_project(&request.identity.repo),
                request.identity.number
            ),
            json!({"sha":request.expected_head,"auto_merge":true}),
        ),
        Action::Comment => ("POST", format!("{base}/notes"), json!({"body": body})),
        Action::Reply => (
            "POST",
            format!("{base}/discussions/{}/notes", discussion.unwrap()),
            json!({"body": body}),
        ),
        Action::Resolve | Action::Unresolve => (
            "PUT",
            format!("{base}/discussions/{}", discussion.unwrap()),
            json!({"resolved": request.action == Action::Resolve}),
        ),
        Action::Merge | Action::EnableAutoMerge => (
            "PUT",
            format!("{base}/merge"),
            json!({"sha": request.expected_head, "auto_merge": request.action == Action::EnableAutoMerge, "should_remove_source_branch": false}),
        ),
        Action::Close => ("PUT", base.into(), json!({"state_event": "close"})),
        Action::Reopen => ("PUT", base.into(), json!({"state_event": "reopen"})),
        Action::Draft | Action::Ready => (
            "POST",
            "graphql".into(),
            json!({"query": "mutation($input: MergeRequestSetDraftInput!) { mergeRequestSetDraft(input: $input) { errors mergeRequest { draft } } }", "variables": {"input": {"projectPath": request.identity.repo, "iid": request.identity.number.to_string(), "draft": request.action == Action::Draft}}}),
        ),
        Action::Rebase => ("PUT", format!("{base}/rebase"), json!({"skip_ci": false})),
        Action::RetryCi => ("POST", pipeline_path(context, "/retry")?, json!({})),
        Action::DisableAutoMerge => (
            "POST",
            format!("{base}/cancel_merge_when_pipeline_succeeds"),
            json!({}),
        ),
    };
    Ok(Write {
        method,
        path,
        body: payload,
    })
}

fn pipeline_path(context: &Context, suffix: &str) -> Result<String, String> {
    let id = pipeline_id(&context.permissions["headPipeline"])
        .ok_or("Pipeline identity unavailable.")?;
    let project = context
        .raw
        .pointer("/head_pipeline/project_id")
        .and_then(Value::as_u64)
        .ok_or("Pipeline project unavailable.")?;
    Ok(format!("projects/{project}/pipelines/{id}{suffix}"))
}

fn same_pipeline(pipeline: &Value, context: &Context) -> bool {
    pipeline["id"].as_u64() == pipeline_id(&context.permissions["headPipeline"])
        && pipeline["project_id"].as_u64() == context.raw["head_pipeline"]["project_id"].as_u64()
        && pipeline["sha"].as_str() == context.core.head_oid.as_deref()
}

struct RetryJob {
    id: u64,
    name: String,
    stage: String,
    retryable: bool,
}

/// GitLab's pipeline jobs endpoint returns current attempts newest ID first.
/// Retain only the first page: a new ID above its maximum cannot be an older
/// job missed on another page. We prove at least one retry, never all retries.
fn retry_jobs(value: &Value, context: &Context) -> Option<Vec<RetryJob>> {
    let values = value.as_array()?;
    if values.len() > 100 {
        return None;
    }
    let jobs: Vec<_> = values
        .iter()
        .map(|v| {
            if !same_pipeline(&v["pipeline"], context) {
                return None;
            }
            Some(RetryJob {
                id: v["id"].as_u64().filter(|id| *id > 0)?,
                name: v["name"].as_str().filter(|s| !s.is_empty())?.to_owned(),
                stage: v["stage"].as_str().filter(|s| !s.is_empty())?.to_owned(),
                retryable: matches!(v["status"].as_str()?, "failed" | "canceled"),
            })
        })
        .collect::<Option<_>>()?;
    // An unexpected sort order or duplicate ID invalidates the high-water mark.
    jobs.windows(2)
        .all(|pair| pair[0].id > pair[1].id)
        .then_some(jobs)
}

fn has_new_attempt(before: &[RetryJob], after: &[RetryJob]) -> bool {
    let Some(newest) = before.first() else {
        return false;
    };
    after.iter().any(|new| {
        new.id > newest.id
            && before
                .iter()
                .any(|old| old.retryable && old.name == new.name && old.stage == new.stage)
    })
}

async fn verify(
    session: &Session<'_>,
    request: &ActionRequest,
    context: &Context,
    response: Value,
    retry_before: Option<&[RetryJob]>,
) -> Result<bool, DetailIssue> {
    let base = session.base();
    match request.action {
        Action::Approve => {
            let v = session
                .request("GET", &format!("{base}/approvals"), None)
                .await?;
            let core = session.core().await?;
            Ok(yes(&v, "user_has_approved")
                && core["sha"].as_str() == request.expected_head.as_deref())
        }
        Action::RequestChanges => {
            if !response
                .pointer("/data/mergeRequestRequestChanges/errors")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
            {
                return Ok(false);
            }
            let fresh = session.context().await?;
            let viewer = context.permissions["viewer"].as_str();
            Ok(viewer.is_some()
                && fresh.permissions["viewer"].as_str() == viewer
                && fresh.core.head_oid == request.expected_head
                && fresh
                    .permissions
                    .pointer("/changeRequesters/nodes")
                    .and_then(Value::as_array)
                    .is_some_and(|nodes| {
                        nodes.iter().any(|node| node["username"].as_str() == viewer)
                    }))
        }
        Action::EnqueueTrain => {
            let fresh = session.context().await?;
            Ok(fresh.core.head_oid == request.expected_head
                && (fresh
                    .permissions
                    .pointer("/mergeTrainCar/index")
                    .and_then(Value::as_u64)
                    .is_some()
                    || (yes(&fresh.permissions, "autoMergeEnabled")
                        && fresh.permissions["autoMergeStrategy"].as_str()
                            == Some("add_to_merge_train_when_checks_pass"))))
        }
        Action::Comment | Action::Reply => {
            let Some(id) = response["id"].as_u64() else {
                return Ok(false);
            };
            let path = if request.action == Action::Reply {
                format!(
                    "{base}/discussions/{}/notes/{id}",
                    request.discussion_id.as_deref().unwrap_or_default()
                )
            } else {
                format!("{base}/notes/{id}")
            };
            let v = session.request("GET", &path, None).await?;
            Ok(v["id"].as_u64() == Some(id)
                && v["body"].as_str() == request.body.as_deref()
                && v["system"].as_bool() == Some(false))
        }
        Action::Resolve | Action::Unresolve => {
            let id = request.discussion_id.as_deref().unwrap_or_default();
            let v = session
                .request("GET", &format!("{base}/discussions/{id}"), None)
                .await?;
            let notes = v["notes"].as_array();
            Ok(v["id"].as_str() == Some(id)
                && notes.is_some_and(|notes| {
                    let resolvable: Vec<_> =
                        notes.iter().filter(|n| yes(n, "resolvable")).collect();
                    !resolvable.is_empty()
                        && resolvable.iter().all(|n| {
                            n["resolved"].as_bool() == Some(request.action == Action::Resolve)
                        })
                }))
        }
        Action::RetryCi => {
            let Some(before) = retry_before.filter(|jobs| jobs.iter().any(|j| j.retryable)) else {
                return Ok(false);
            };
            if !same_pipeline(&response, context) {
                return Ok(false);
            }
            let path =
                pipeline_path(context, RETRY_JOBS).map_err(|_| DetailIssue::InvalidResponse)?;
            for attempt in 0..3 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                let value = session.request("GET", &path, None).await?;
                let after = retry_jobs(&value, context).ok_or(DetailIssue::InvalidResponse)?;
                if has_new_attempt(before, &after) {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Action::Rebase => {
            // The endpoint enqueues work. Bounded polling may yield an
            // unverified receipt; an accepted enqueue is never called done.
            for attempt in 0..3 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                let fresh = session.context().await?;
                if fresh.raw["rebase_in_progress"].as_bool() == Some(false)
                    && fresh.raw["merge_error"].is_null()
                    && fresh.core.head_oid != request.expected_head
                    && fresh.permissions["rebaseCommitSha"]
                        .as_str()
                        .is_some_and(|sha| Some(sha) == fresh.core.head_oid.as_deref())
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Action::EnableAutoMerge | Action::DisableAutoMerge => {
            let fresh = session.context().await?;
            Ok(fresh.permissions["autoMergeEnabled"].as_bool()
                == Some(request.action == Action::EnableAutoMerge)
                && (request.action == Action::DisableAutoMerge
                    || fresh.core.head_oid == request.expected_head))
        }
        _ => {
            let v = session.core().await?;
            Ok(match request.action {
                Action::Merge => {
                    v["state"].as_str() == Some("merged")
                        && v["sha"].as_str() == request.expected_head.as_deref()
                }
                Action::Close => v["state"].as_str() == Some("closed"),
                Action::Reopen => v["state"].as_str() == Some("opened"),
                Action::Draft => v["draft"].as_bool() == Some(true),
                Action::Ready => v["draft"].as_bool() == Some(false),
                _ => false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Provider, Source};

    const THREAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BASE: &str = "projects/group%2Fsubgroup%2Fproject/merge_requests/7";

    fn identity() -> PrIdentity {
        PrIdentity {
            source: Source {
                provider: Provider::Gitlab,
                host: "gitlab.com".into(),
            },
            repo: "group/subgroup/project".into(),
            number: 7,
        }
    }

    fn raw() -> Value {
        json!({"id": 70, "iid":7, "web_url":"https://gitlab.com/group/subgroup/project/-/merge_requests/7", "title":"Review", "author":{"username":"author"}, "state":"opened", "draft":false, "sha":"old-head", "source_branch":"topic", "target_branch":"main", "detailed_merge_status":"mergeable", "rebase_in_progress":false, "merge_error":null, "head_pipeline":{"id":91,"project_id":42,"sha":"old-head","status":"failed"}, "action_approvals":{"user_can_approve":true,"user_has_approved":false}})
    }

    fn permissions() -> Value {
        json!({"iid":"7","webUrl":"https://gitlab.com/group/subgroup/project/-/merge_requests/7","diffHeadSha":"old-head","rebaseCommitSha":null,"autoMergeEnabled":false,"availableAutoMergeStrategies":["merge_when_checks_pass"],"userPermissions":{"canApprove":true,"canMerge":true,"createNote":true,"updateMergeRequest":true,"pushToSourceBranch":true},"headPipeline":{"id":"gid://gitlab/Ci::Pipeline/91","sha":"old-head","retryable":true,"userPermissions":{"updatePipeline":true}},"discussions":{"nodes":[{"replyId":format!("gid://gitlab/DiffDiscussion/{THREAD}"),"resolvable":true,"resolved":false,"userPermissions":{"resolveNote":true},"notes":{"nodes":[{"system":false,"userPermissions":{"createNote":true}}]}}],"pageInfo":{"hasNextPage":false}}})
    }

    fn context(raw: Value, permissions: Value) -> Context {
        let core = detail::map_core(&raw, &identity()).unwrap();
        let capabilities = capabilities_from(&core, &raw, &permissions);
        Context {
            core,
            raw,
            permissions,
            capabilities,
        }
    }

    fn request(action: Action) -> ActionRequest {
        ActionRequest {
            expected_viewer: None,
            identity: identity(),
            action,
            expected_head: Some("old-head".into()),
            body: Some("Please review this change.".into()),
            discussion_id: Some(THREAD.into()),
        }
    }

    #[test]
    fn gates_fail_closed_for_missing_permissions_and_unknown_pipeline() {
        let c = context(raw(), Value::Null);
        assert!(c.capabilities.actions.iter().all(|a| !a.allowed));
        let mut r = raw();
        r["head_pipeline"]["project_id"] = Value::Null;
        assert!(prepare(BASE, &request(Action::RetryCi), &context(r, permissions())).is_err());
        let mut p = permissions();
        p["headPipeline"]["sha"] = json!("stale");
        assert!(prepare(BASE, &request(Action::RetryCi), &context(raw(), p)).is_err());
    }

    #[test]
    fn approval_requires_rest_permission_and_reauthentication_is_not_bypassed() {
        for approvals in [
            Value::Null,
            json!({"user_can_approve":false}),
            json!({"user_can_approve":true,"require_password_to_approve":true}),
            json!({"user_can_approve":true,"require_reauthentication_to_approve":true}),
            json!({"user_can_approve":true,"user_has_approved":true}),
        ] {
            let mut r = raw();
            r["action_approvals"] = approvals;
            assert!(prepare(BASE, &request(Action::Approve), &context(r, permissions())).is_err());
        }
    }

    #[test]
    fn stale_head_refuses_every_commit_sensitive_action() {
        let c = context(raw(), permissions());
        for action in [
            Action::Approve,
            Action::Merge,
            Action::Rebase,
            Action::RetryCi,
            Action::EnableAutoMerge,
        ] {
            for expected in [None, Some(String::new()), Some("different".into())] {
                let mut req = request(action);
                req.expected_head = expected;
                assert!(prepare(BASE, &req, &c).is_err(), "{action:?}");
            }
        }
        let write = prepare(BASE, &request(Action::Merge), &c).unwrap();
        assert_eq!(write.body["sha"], "old-head");
        assert_eq!(write.body["should_remove_source_branch"], false);
    }

    #[test]
    fn no_coarse_merge_status_or_unknown_auto_merge_strategy_is_authority() {
        for status in [
            Value::Null,
            json!("checking"),
            json!("draft_status"),
            json!("not_approved"),
        ] {
            let mut r = raw();
            r["detailed_merge_status"] = status;
            r["merge_status"] = json!("can_be_merged");
            assert!(prepare(BASE, &request(Action::Merge), &context(r, permissions())).is_err());
        }
        for strategies in [
            Value::Null,
            json!([]),
            json!(["merge_train"]),
            json!(["unknown"]),
        ] {
            let mut p = permissions();
            p["availableAutoMergeStrategies"] = strategies;
            assert!(prepare(BASE, &request(Action::EnableAutoMerge), &context(raw(), p)).is_err());
        }
    }

    #[test]
    fn discussions_require_matching_bounded_permission_receipt() {
        let c = context(raw(), permissions());
        assert_eq!(c.capabilities.discussions[0].id, THREAD);
        let mut req = request(Action::Resolve);
        req.discussion_id = Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
        assert!(prepare(BASE, &req, &c).is_err());
        req.discussion_id = Some("../../other".into());
        assert!(prepare(BASE, &req, &c).is_err());
        let mut p = permissions();
        p["discussions"]["nodes"][0]["userPermissions"]["resolveNote"] = json!(false);
        assert!(prepare(BASE, &request(Action::Resolve), &context(raw(), p)).is_err());
    }

    #[test]
    fn comments_cannot_smuggle_additional_quick_action_writes() {
        for body in ["/merge", "text\n /approve", "\t/close", "", "  "] {
            let mut req = request(Action::Comment);
            req.body = Some(body.into());
            assert!(prepare(BASE, &req, &context(raw(), permissions())).is_err());
        }
    }

    #[test]
    fn writes_follow_documented_endpoints_and_do_not_delete_source_branches() {
        let c = context(raw(), permissions());
        for (action, method, path, payload) in [
            (
                Action::Approve,
                "POST",
                format!("{BASE}/approve"),
                json!({"sha":"old-head"}),
            ),
            (
                Action::Comment,
                "POST",
                format!("{BASE}/notes"),
                json!({"body":"Please review this change."}),
            ),
            (
                Action::Reply,
                "POST",
                format!("{BASE}/discussions/{THREAD}/notes"),
                json!({"body":"Please review this change."}),
            ),
            (
                Action::Resolve,
                "PUT",
                format!("{BASE}/discussions/{THREAD}"),
                json!({"resolved":true}),
            ),
            (
                Action::Merge,
                "PUT",
                format!("{BASE}/merge"),
                json!({"sha":"old-head","auto_merge":false,"should_remove_source_branch":false}),
            ),
            (
                Action::Close,
                "PUT",
                BASE.into(),
                json!({"state_event":"close"}),
            ),
            (
                Action::Rebase,
                "PUT",
                format!("{BASE}/rebase"),
                json!({"skip_ci":false}),
            ),
            (
                Action::RetryCi,
                "POST",
                "projects/42/pipelines/91/retry".into(),
                json!({}),
            ),
            (
                Action::EnableAutoMerge,
                "PUT",
                format!("{BASE}/merge"),
                json!({"sha":"old-head","auto_merge":true,"should_remove_source_branch":false}),
            ),
        ] {
            let write = prepare(BASE, &request(action), &c).unwrap();
            assert_eq!(
                (write.method, write.path, write.body),
                (method, path, payload),
                "{action:?}"
            );
        }
        let draft = prepare(BASE, &request(Action::Draft), &c).unwrap();
        assert_eq!(draft.method, "POST");
        assert_eq!(draft.path, "graphql");
        assert!(draft.body["query"]
            .as_str()
            .unwrap()
            .contains("mergeRequestSetDraft"));
        assert_eq!(
            draft.body["variables"]["input"],
            json!({"projectPath":"group/subgroup/project","iid":"7","draft":true})
        );
    }

    #[cfg(unix)]
    fn scripted(steps: &[Value]) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("steps.json"),
            serde_json::to_vec(steps).unwrap(),
        )
        .unwrap();
        let program = dir.path().join("glab");
        std::fs::write(
            &program,
            r#"#!/usr/bin/python3
import sys,json,pathlib,time
root=pathlib.Path(__file__).parent
counter=root/'count'
n=int(counter.read_text()) if counter.exists() else 0
steps=json.loads((root/'steps.json').read_text())
step=steps[n]
args=sys.argv[1:]
assert args[:4]==['api','--hostname','gitlab.com','-i']
method=args[args.index('--method')+1] if '--method' in args else 'GET'
assert method==step['method'] and args[4]==step['path'], (method,args[4])
if '--input' in args:
 body=json.load(sys.stdin)
 assert body==step['body'], 'request body differs'
 assert '--header' in args and 'Content-Type: application/json' in args
else: assert 'body' not in step
counter.write_text(str(n+1))
time.sleep(step.get('sleep_seconds',0))
print('HTTP/2 '+str(step.get('status',200))+'\n\n'+json.dumps(step['response']))
"#,
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, program)
    }

    #[cfg(unix)]
    fn read(path: String, response: Value) -> Value {
        json!({"method":"GET","path":path,"response":response})
    }

    #[cfg(unix)]
    fn context_steps(r: &Value, p: &Value) -> Vec<Value> {
        vec![
            read(format!("{BASE}?include_rebase_in_progress=true"), r.clone()),
            json!({"method":"POST","path":"graphql","body":{"query":QUERY,"variables":{"path":identity().repo,"iid":"7"}},"response":{"data":{"currentUser":{"username":p["viewer"]},"project":{"mergeRequest":p}}}}),
            read(format!("{BASE}/approvals"), r["action_approvals"].clone()),
        ]
    }

    fn pipeline() -> Value {
        json!({"id":91,"project_id":42,"sha":"old-head","status":"running"})
    }

    fn job(id: u64, name: &str, status: &str) -> Value {
        json!({"id":id,"name":name,"stage":"test","status":status,"pipeline":pipeline()})
    }

    #[cfg(unix)]
    fn retry_steps(before: Value, after: &[Value]) -> Vec<Value> {
        let mut steps = context_steps(&raw(), &permissions());
        steps.push(read(
            format!("projects/42/pipelines/91{RETRY_JOBS}"),
            before,
        ));
        steps.push(read(
            format!("{BASE}?include_rebase_in_progress=true"),
            raw(),
        ));
        steps.push(json!({"method":"POST","path":"projects/42/pipelines/91/retry","body":{},"response":pipeline()}));
        steps.extend(
            after
                .iter()
                .map(|v| read(format!("projects/42/pipelines/91{RETRY_JOBS}"), v.clone())),
        );
        steps
    }

    #[cfg(unix)]
    async fn check_retry(steps: &[Value], outcome: Outcome) -> Receipt {
        let (dir, program) = scripted(steps);
        let receipt = execute_with_program(&program, &request(Action::RetryCi))
            .await
            .unwrap();
        assert_eq!(receipt.outcome, outcome);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("count")).unwrap(),
            steps.len().to_string()
        );
        receipt
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retry_verifies_new_attempts_even_when_they_finish_before_readback() {
        for prior_status in ["failed", "canceled"] {
            for new_status in ["pending", "running", "success", "failed", "canceled"] {
                let before = json!([job(200, "other", "running"), job(100, "unit", prior_status)]);
                let after = json!([job(201, "unit", new_status), job(200, "other", "running")]);
                let receipt = check_retry(&retry_steps(before, &[after]), Outcome::Verified).await;
                assert!(receipt.message.starts_with("At least one"));
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retry_polls_for_delayed_evidence_but_does_not_repeat_the_write() {
        let before = json!([job(100, "unit", "failed")]);
        let after = json!([job(101, "unit", "pending")]);
        check_retry(
            &retry_steps(before.clone(), &[before.clone(), before, after]),
            Outcome::Verified,
        )
        .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retry_cannot_verify_unchanged_running_pipeline_or_unrelated_jobs() {
        let before = json!([job(200, "other", "running"), job(100, "unit", "failed")]);
        for after in [
            before.clone(),
            // Status changes are not new attempts.
            json!([job(200, "other", "running"), job(100, "unit", "pending")]),
            json!([job(201, "unrelated", "pending"), job(100, "unit", "failed")]),
            // An existing job from another page cannot be counted as new.
            json!([job(150, "unit", "pending")]),
            json!([]),
        ] {
            let receipt = check_retry(
                &retry_steps(before.clone(), &[after.clone(), after.clone(), after]),
                Outcome::Unverified,
            )
            .await;
            assert!(receipt.message.contains("may already have applied"));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retry_keeps_incomplete_or_failed_evidence_unverified() {
        let before = json!([job(100, "unit", "failed")]);
        let after = json!([job(101, "unit", "pending")]);
        for invalid in [Value::Null, json!({}), json!([{"id":101}])] {
            check_retry(&retry_steps(invalid.clone(), &[]), Outcome::Unverified).await;
            check_retry(
                &retry_steps(before.clone(), &[invalid]),
                Outcome::Unverified,
            )
            .await;
        }
        for prior in [json!([]), json!([job(100, "unit", "success")])] {
            check_retry(&retry_steps(prior, &[]), Outcome::Unverified).await;
        }
        for index in [3, 5, 6] {
            let mut steps = retry_steps(before.clone(), std::slice::from_ref(&after));
            steps[index]["status"] = json!(403);
            // Failed baseline still permits the write; failed write has no readback.
            if index != 6 {
                steps.truncate(6);
            }
            check_retry(&steps, Outcome::Unverified).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retry_readback_timeout_keeps_receipt_unverified_without_repeating_write() {
        let mut steps = retry_steps(
            json!([job(100, "unit", "failed")]),
            &[json!([job(101, "unit", "pending")])],
        );
        steps[6]["sleep_seconds"] = json!(30);
        let receipt = check_retry(&steps, Outcome::Unverified).await;
        assert!(receipt.message.contains("may already have applied"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retry_rejects_wrong_pipeline_project_or_head_in_every_evidence_source() {
        let before = json!([job(100, "unit", "failed")]);
        let after = json!([job(101, "unit", "pending")]);
        for (field, wrong) in [
            ("id", json!(92)),
            ("project_id", json!(43)),
            ("sha", json!("other-head")),
        ] {
            for index in [3, 5, 6] {
                for value in [wrong.clone(), Value::Null] {
                    let mut steps = retry_steps(before.clone(), std::slice::from_ref(&after));
                    if index == 5 {
                        steps[index]["response"][field] = value;
                    } else {
                        steps[index]["response"][0]["pipeline"][field] = value;
                    }
                    if index != 6 {
                        steps.truncate(6);
                    }
                    check_retry(&steps, Outcome::Unverified).await;
                }
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retry_rechecks_head_and_fork_pipeline_before_write() {
        for field in ["sha", "id", "project_id"] {
            let mut steps = retry_steps(json!([job(100, "unit", "failed")]), &[]);
            steps[4]["response"]["head_pipeline"][field] = Value::Null;
            steps.truncate(5);
            let (dir, program) = scripted(&steps);
            assert!(execute_with_program(&program, &request(Action::RetryCi))
                .await
                .unwrap_err()
                .contains("changed"));
            assert_eq!(
                std::fs::read_to_string(dir.path().join("count")).unwrap(),
                "5"
            );
        }
        let mut steps = retry_steps(json!([job(100, "unit", "failed")]), &[]);
        steps[4]["response"]["sha"] = json!("new-head");
        steps.truncate(5);
        let (_dir, program) = scripted(&steps);
        assert!(execute_with_program(&program, &request(Action::RetryCi))
            .await
            .is_err());
    }

    #[test]
    fn retry_requires_ordered_valid_job_evidence_and_respects_page_high_water_mark() {
        let c = context(raw(), permissions());
        for invalid in [
            json!([job(100, "unit", "failed"), job(101, "other", "running")]),
            json!([job(100, "unit", "failed"), job(100, "other", "running")]),
            json!([job(0, "unit", "failed")]),
            json!([job(100, "", "failed")]),
        ] {
            assert!(retry_jobs(&invalid, &c).is_none());
        }
        // A full page has unknown coverage, but its maximum still excludes
        // older jobs. A captured failed job can prove at least one retry.
        let page = Value::Array(
            (101..=200)
                .rev()
                .map(|id| job(id, &format!("job-{id}"), "failed"))
                .collect(),
        );
        let before = retry_jobs(&page, &c).unwrap();
        let after = retry_jobs(&json!([job(201, "job-101", "pending")]), &c).unwrap();
        assert!(has_new_attempt(&before, &after));
        let after = retry_jobs(&json!([job(99, "job-101", "pending")]), &c).unwrap();
        assert!(!has_new_attempt(&before, &after));
        let after = retry_jobs(&json!([job(201, "job-100", "pending")]), &c).unwrap();
        assert!(!has_new_attempt(&before, &after));
    }

    #[tokio::test]
    async fn retry_readback_budget_exhaustion_is_not_proof() {
        let c = context(raw(), permissions());
        let before = retry_jobs(&json!([job(100, "unit", "failed")]), &c).unwrap();
        let identity = identity();
        let session = Session {
            program: Path::new("/program-must-not-run"),
            identity: &identity,
            start: tokio::time::Instant::now() - BUDGET,
        };
        assert_eq!(
            verify(
                &session,
                &request(Action::RetryCi),
                &c,
                pipeline(),
                Some(&before)
            )
            .await,
            Err(DetailIssue::BudgetExhausted)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn system_or_unknown_discussion_reply_permission_refuses_before_post() {
        for note in [
            json!({"system":true,"userPermissions":{"createNote":true}}),
            json!({"system":null,"userPermissions":{"createNote":true}}),
            json!({"userPermissions":{"createNote":true}}),
            json!({"system":false,"userPermissions":{"createNote":false}}),
            json!({"system":false,"userPermissions":{"createNote":null}}),
            json!({"system":false}),
        ] {
            let mut p = permissions();
            // Another eligible discussion keeps the MR-level Reply action
            // enabled, forcing the selected discussion's own gate to decide.
            let mut other = p["discussions"]["nodes"][0].clone();
            other["replyId"] =
                json!("gid://gitlab/DiffDiscussion/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
            p["discussions"]["nodes"]
                .as_array_mut()
                .unwrap()
                .push(other);
            p["discussions"]["nodes"][0]["notes"]["nodes"][0] = note;
            let c = context(raw(), p.clone());
            assert!(!c.capabilities.discussions[0].can_reply);
            assert!(c.capabilities.discussions[1].can_reply);
            assert!(c
                .capabilities
                .actions
                .iter()
                .any(|c| c.action == Action::Reply && c.allowed));
            let steps = context_steps(&raw(), &p);
            let (dir, program) = scripted(&steps);
            let error = execute_with_program(&program, &request(Action::Reply))
                .await
                .unwrap_err();
            assert!(error.contains("permission to reply to this discussion"));
            assert_eq!(
                std::fs::read_to_string(dir.path().join("count")).unwrap(),
                "3"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn every_supported_action_requires_matching_readback_after_the_write() {
        for action in [
            Action::Approve,
            Action::Comment,
            Action::Reply,
            Action::Resolve,
            Action::Unresolve,
            Action::Merge,
            Action::Close,
            Action::Reopen,
            Action::Draft,
            Action::Ready,
            Action::Rebase,
            Action::EnableAutoMerge,
            Action::DisableAutoMerge,
        ] {
            let mut r = raw();
            let mut p = permissions();
            if action == Action::Ready {
                r["draft"] = json!(true);
            }
            if action == Action::Reopen {
                r["state"] = json!("closed");
            }
            if action == Action::DisableAutoMerge {
                p["autoMergeEnabled"] = json!(true);
            }
            if action == Action::Unresolve {
                p["discussions"]["nodes"][0]["resolved"] = json!(true);
            }
            let req = request(action);
            let c = context(r.clone(), p.clone());
            let write = prepare(BASE, &req, &c).unwrap();
            let mut steps = context_steps(&r, &p);
            steps.push(json!({"method":write.method,"path":write.path,"body":write.body,"response":{"id":123}}));
            match action {
                Action::Approve => {
                    steps.push(read(format!("{BASE}/approvals"), json!({"user_has_approved":true})));
                    steps.push(read(format!("{BASE}?include_rebase_in_progress=true"), r.clone()));
                }
                Action::Comment | Action::Reply => {
                    let path = if action == Action::Reply { format!("{BASE}/discussions/{THREAD}/notes/123") } else { format!("{BASE}/notes/123") };
                    steps.push(read(path, json!({"id":123,"body":req.body,"system":false})));
                }
                Action::Resolve | Action::Unresolve => steps.push(read(format!("{BASE}/discussions/{THREAD}"), json!({"id":THREAD,"notes":[{"resolvable":true,"resolved":action == Action::Resolve}]}))),
                Action::Rebase => {
                    r["sha"] = json!("new-head"); p["diffHeadSha"] = json!("new-head"); p["rebaseCommitSha"] = json!("new-head");
                    steps.extend(context_steps(&r, &p));
                }
                Action::EnableAutoMerge | Action::DisableAutoMerge => {
                    p["autoMergeEnabled"] = json!(action == Action::EnableAutoMerge);
                    steps.extend(context_steps(&r, &p));
                }
                _ => {
                    match action {
                        Action::Merge => r["state"] = json!("merged"),
                        Action::Close => r["state"] = json!("closed"),
                        Action::Reopen => r["state"] = json!("opened"),
                        Action::Draft => r["draft"] = json!(true),
                        Action::Ready => r["draft"] = json!(false),
                        _ => unreachable!(),
                    }
                    steps.push(read(format!("{BASE}?include_rebase_in_progress=true"), r.clone()));
                }
            }
            let (dir, program) = scripted(&steps);
            let receipt = execute_with_program(&program, &req).await.unwrap();
            assert_eq!(receipt.outcome, Outcome::Verified, "{action:?}");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("count")).unwrap(),
                steps.len().to_string()
            );
            // The exact same 2xx write with failed readback must never report success.
            steps[4]["status"] = json!(403);
            let (_dir, program) = scripted(&steps);
            assert_eq!(
                execute_with_program(&program, &req).await.unwrap().outcome,
                Outcome::Unverified,
                "{action:?}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provider_host_iid_and_graphql_errors_refuse_before_any_write() {
        for (provider, host, iid) in [
            (Provider::Github, "github.com", 7),
            (Provider::Gitlab, "gitlab.example", 7),
            (Provider::Gitlab, "gitlab.com", 0),
        ] {
            let mut req = request(Action::Close);
            req.identity.source = Source {
                provider,
                host: host.into(),
            };
            req.identity.number = iid;
            assert!(
                execute_with_program(Path::new("/program-must-not-run"), &req)
                    .await
                    .is_err()
            );
        }
        for response in [
            json!({"data":null,"errors":[{"message":"private message must not escape"}]}),
            json!({"data":{"project":{"mergeRequest":{"iid":"8"}}}}),
        ] {
            let mut steps = context_steps(&raw(), &permissions());
            steps[1]["response"] = response;
            let (dir, program) = scripted(&steps);
            let error = execute_with_program(&program, &request(Action::Close))
                .await
                .unwrap_err();
            assert!(!error.contains("private"));
            assert_eq!(
                std::fs::read_to_string(dir.path().join("count")).unwrap(),
                "2"
            );
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn optional_schema_failure_preserves_basic_permissions_without_guessing_optional_ones() {
        let r = raw();
        let mut steps = context_steps(&r, &permissions());
        steps[1]["response"] = json!({"errors":[{"extensions":{"code":"undefinedField"}}]});
        let basic = json!({"iid":"7","webUrl":r["web_url"],"diffHeadSha":r["sha"],"userPermissions":{"canMerge":true,"createNote":true,"updateMergeRequest":true}});
        steps.insert(2, json!({"method":"POST","path":"graphql","body":{"query":LEGACY_QUERY,"variables":{"path":identity().repo,"iid":"7"}},"response":{"data":{"project":{"mergeRequest":basic}}}}));
        let (_dir, program) = scripted(&steps);
        let identity = identity();
        let session = Session {
            program: &program,
            identity: &identity,
            start: tokio::time::Instant::now(),
        };
        let context = session.context().await.unwrap();
        assert!(context
            .capabilities
            .actions
            .iter()
            .any(|c| c.action == Action::Close && c.allowed));
        assert!(context
            .capabilities
            .actions
            .iter()
            .any(|c| c.action == Action::Comment && c.allowed));
        assert!(!context
            .capabilities
            .actions
            .iter()
            .any(|c| c.action == Action::RetryCi && c.allowed));
    }
    #[test]
    fn request_changes_and_train_actions_require_measured_capabilities_and_current_head() {
        let mut p = permissions();
        p["viewer"] = json!("reviewer");
        p["changeRequesters"] = json!({"nodes":[],"pageInfo":{"hasNextPage":false}});
        p["mergeTrainCar"] = Value::Null;
        p["availableAutoMergeStrategies"] = json!(["merge_train"]);
        let c = context(raw(), p.clone());
        for name in ["request_changes", "enqueue_train"] {
            let action: Action =
                serde_json::from_value(json!(name)).expect("supported GitLab action");
            let write = prepare(BASE, &request(action), &c).unwrap();
            assert_eq!(write.method, "POST");
            let mut stale = request(action);
            stale.expected_head = Some("stale-head".into());
            assert!(prepare(BASE, &stale, &c).is_err());
        }
        p["userPermissions"]["updateMergeRequest"] = json!(false);
        p["userPermissions"]["canMerge"] = json!(false);
        let denied = context(raw(), p);
        for name in ["request_changes", "enqueue_train"] {
            let action: Action = serde_json::from_value(json!(name)).unwrap();
            assert!(prepare(BASE, &request(action), &denied).is_err());
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn review_and_train_writes_need_independent_readback_and_never_repeat() {
        for action in [Action::RequestChanges, Action::EnqueueTrain] {
            for verified in [true, false] {
                let r = raw();
                let mut p = permissions();
                p["viewer"] = json!("reviewer");
                p["changeRequesters"] = json!({"nodes":[],"pageInfo":{"hasNextPage":false}});
                p["mergeTrainCar"] = Value::Null;
                p["availableAutoMergeStrategies"] = json!(["merge_train"]);
                let req = request(action);
                let write = prepare(BASE, &req, &context(r.clone(), p.clone())).unwrap();
                let mut steps = context_steps(&r, &p);
                if action == Action::RequestChanges {
                    steps.push(read(
                        format!("{BASE}?include_rebase_in_progress=true"),
                        r.clone(),
                    ));
                }
                steps.push(json!({"method":write.method,"path":write.path,"body":write.body,"response":{"data":{"mergeRequestRequestChanges":{"errors":[],"mergeRequest":{"iid":"7"}}}}}));
                if verified {
                    if action == Action::RequestChanges {
                        p["changeRequesters"]["nodes"] = json!([{"username":"reviewer"}]);
                    } else {
                        p["mergeTrainCar"] = json!({"index":0});
                    }
                }
                steps.extend(context_steps(&r, &p));
                let (dir, program) = scripted(&steps);
                let receipt = execute_with_program(&program, &req).await.unwrap();
                assert_eq!(
                    receipt.outcome,
                    if verified {
                        Outcome::Verified
                    } else {
                        Outcome::Unverified
                    }
                );
                assert_eq!(
                    std::fs::read_to_string(dir.path().join("count")).unwrap(),
                    steps.len().to_string()
                );
            }
        }
    }
}
