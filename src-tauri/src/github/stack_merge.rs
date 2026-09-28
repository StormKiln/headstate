//! Merging or queueing a native GitHub stack (#1468).
//!
//! GitHub merges a stacked pull request only through its asynchronous
//! merge API: the schema's `mergePullRequest` "does not support stacked
//! pull requests", and gh-stack's reference (`docs/reference/merge-api.md`)
//! says the legacy merge endpoints and mutations cannot merge a stack.
//!
//! - `PUT /repos/{o}/{r}/pulls/{n}/merge-async` with `merge_action`
//!   (`merge_queue` or `direct_merge`) and the expected head `sha`.
//! - `GET .../merge-async/{uuid}` until the status is no longer `pending`.
//!
//! The request lands or queues EVERY open pull request beneath `n` too,
//! atomically. That is why the UI confirms with the list first; this layer
//! only carries out what was agreed.
//!
//! A submission GitHub accepted and that is still running when the poll
//! budget runs out is `InProgress`, never `Failed` -- "we stopped watching"
//! is not "it did not work".

use super::client::GitHubClient;
use super::stats::Budget;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// How long to keep polling before reporting "still in progress".
///
/// GitHub's docs say a stack merge "may take up to a few minutes". Thirty
/// seconds covers a direct merge or a queue entry of a short stack; past
/// it the answer is honest either way, since GitHub keeps the result for
/// 24 hours and the list refresh picks the outcome up.
pub const POLL_BUDGET: Duration = Duration::from_secs(30);

/// Between polls. GitHub's docs suggest "once a second".
pub const POLL_INTERVAL: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackMergeAction {
    /// Add the stack to the base branch's merge queue.
    MergeQueue,
    /// Merge the stack directly.
    DirectMerge,
}

impl StackMergeAction {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "merge_queue" => Ok(Self::MergeQueue),
            "direct_merge" => Ok(Self::DirectMerge),
            other => Err(format!("unknown stack merge action: {other}")),
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::MergeQueue => "merge_queue",
            Self::DirectMerge => "direct_merge",
        }
    }
}

/// How a stack merge ended, as far as Headstate saw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StackMergeOutcome {
    /// Merged directly; `sha` is the resulting commit when GitHub gave one.
    Merged { sha: Option<String> },
    /// Added to the base branch's merge queue.
    Enqueued,
    /// GitHub attempted it and it did not happen. Atomic, so nothing
    /// landed. `message` is GitHub's own reason.
    Failed { message: String },
    /// Accepted and still running when Headstate stopped checking. NOT a
    /// failure: the merge may yet land.
    InProgress { message: String },
}

/// One reading of a submit or poll response.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    Done(StackMergeOutcome),
    Pending { uuid: String },
}

fn message_of(body: &Value) -> Option<String> {
    body["details"]["message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .map(str::to_string)
}

/// Read a submit (`PUT`) or poll (`GET`) response.
///
/// The status codes are gh-stack's documented ones: 202 pending, 200 done,
/// 409 a merge already running (its `uuid` returned), 400 not mergeable
/// (`failed`), 404 unavailable for this repository, 422 a bad request.
pub fn read(status: u16, body: &Value) -> Result<Reading, String> {
    match status {
        404 => return Err("GitHub's stack merge is not available for this repository".into()),
        422 => {
            return Err(message_of(body)
                .unwrap_or_else(|| "GitHub rejected the stack merge request".into()))
        }
        _ => {}
    }
    match body["status"].as_str() {
        Some("pending") => match body["details"]["uuid"].as_str() {
            Some(uuid) if !uuid.is_empty() => Ok(Reading::Pending {
                uuid: uuid.to_string(),
            }),
            _ => Err("GitHub accepted the stack merge but gave no way to follow it".into()),
        },
        Some("merged") => Ok(Reading::Done(StackMergeOutcome::Merged {
            sha: body["details"]["sha"].as_str().map(str::to_string),
        })),
        Some("enqueued") => Ok(Reading::Done(StackMergeOutcome::Enqueued)),
        Some("failed") => Ok(Reading::Done(StackMergeOutcome::Failed {
            message: message_of(body).unwrap_or_else(|| "GitHub did not say why".into()),
        })),
        _ => Err(format!(
            "GitHub answered {status} without a stack merge result"
        )),
    }
}

impl GitHubClient {
    /// Submit a stack merge for `number` and follow it to a result.
    pub async fn merge_stack(
        &self,
        repo: &str,
        number: u64,
        action: StackMergeAction,
        expected_head: &str,
        budget: &Budget,
    ) -> Result<StackMergeOutcome, String> {
        self.merge_stack_with(
            repo,
            number,
            action,
            expected_head,
            budget,
            POLL_INTERVAL,
            POLL_BUDGET,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn merge_stack_with(
        &self,
        repo: &str,
        number: u64,
        action: StackMergeAction,
        expected_head: &str,
        budget: &Budget,
        interval: Duration,
        deadline: Duration,
    ) -> Result<StackMergeOutcome, String> {
        let (owner, name) = repo
            .split_once('/')
            .ok_or_else(|| format!("malformed repository: {repo}"))?;
        let path = format!("/repos/{owner}/{name}/pulls/{number}/merge-async");
        let mut body = json!({ "merge_action": action.wire() });
        // The head the user was looking at: GitHub refuses the merge if the
        // branch moved since, rather than landing commits nobody saw.
        if !expected_head.is_empty() {
            body["sha"] = json!(expected_head);
        }

        // A transport failure on the SUBMIT leaves it unknown whether GitHub
        // took the request, so the message says that rather than "failed".
        let (status, answer) = self.rest_put(&path, &body, budget).await.map_err(|e| {
            format!(
                "No answer from GitHub ({e}), so the stack may or may not have been \
                 submitted. Check the pull request on GitHub before trying again."
            )
        })?;
        let uuid = match read(status, &answer)? {
            Reading::Done(outcome) => return Ok(outcome),
            Reading::Pending { uuid } => uuid,
        };

        let started = tokio::time::Instant::now();
        let still_running = |why: &str| StackMergeOutcome::InProgress {
            message: why.to_string(),
        };
        loop {
            if started.elapsed() + interval > deadline {
                return Ok(still_running(
                    "Submitted to GitHub and still in progress there",
                ));
            }
            tokio::time::sleep(interval).await;
            // Headstate declining to spend the last of the REST pool is not
            // GitHub failing the merge; say which it is.
            if !budget.permits_rest(1) {
                return Ok(still_running(
                    "Submitted to GitHub; Headstate stopped checking to save its REST rate limit",
                ));
            }
            match self.rest_get(&format!("{path}/{uuid}"), budget).await {
                Ok(v) => match read(200, &v) {
                    Ok(Reading::Done(outcome)) => return Ok(outcome),
                    Ok(Reading::Pending { .. }) => continue,
                    Err(_) => {
                        return Ok(still_running(
                            "Submitted to GitHub; its progress could not be read",
                        ))
                    }
                },
                // The merge was accepted; a failed READ says nothing about
                // whether it lands.
                Err(_) => {
                    return Ok(still_running(
                        "Submitted to GitHub; its progress could not be read",
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn client_for(server: &MockServer) -> GitHubClient {
        let oc = octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("test-token".to_string())
            .build()
            .unwrap();
        GitHubClient::new(oc)
    }

    const PUT_PATH: &str = "/repos/acme/widgets/pulls/30/merge-async";
    const POLL_PATH: &str = "/repos/acme/widgets/pulls/30/merge-async/u-1";

    fn pending() -> Value {
        json!({ "status": "pending", "details": { "message": "Merge request enqueued.", "uuid": "u-1" } })
    }

    async fn run(server: &MockServer, action: StackMergeAction) -> StackMergeOutcome {
        client_for(server)
            .await
            .merge_stack_with(
                "acme/widgets",
                30,
                action,
                "abc123",
                &Budget::new(),
                Duration::from_millis(5),
                Duration::from_millis(500),
            )
            .await
            .unwrap()
    }

    /// The queue path asks for the QUEUE, with the head the user saw, and
    /// follows the request to GitHub's answer.
    #[tokio::test]
    async fn queueing_a_stack_sends_merge_queue_and_polls_to_enqueued() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(PUT_PATH))
            .and(body_partial_json(
                json!({ "merge_action": "merge_queue", "sha": "abc123" }),
            ))
            .respond_with(ResponseTemplate::new(202).set_body_json(pending()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(POLL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "enqueued",
                "details": { "message": "Pull request was added to the merge queue." }
            })))
            .mount(&server)
            .await;

        assert_eq!(
            run(&server, StackMergeAction::MergeQueue).await,
            StackMergeOutcome::Enqueued
        );
    }

    #[tokio::test]
    async fn a_direct_merge_polls_to_merged() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(PUT_PATH))
            .and(body_partial_json(json!({ "merge_action": "direct_merge" })))
            .respond_with(ResponseTemplate::new(202).set_body_json(pending()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(POLL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "merged", "details": { "message": "Pull request was merged.", "sha": "def456" }
            })))
            .mount(&server)
            .await;
        assert_eq!(
            run(&server, StackMergeAction::DirectMerge).await,
            StackMergeOutcome::Merged {
                sha: Some("def456".into())
            }
        );
    }

    /// A rule failure surfaces while polling, with GitHub's own reason.
    #[tokio::test]
    async fn a_failed_merge_reports_githubs_reason() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(PUT_PATH))
            .respond_with(ResponseTemplate::new(202).set_body_json(pending()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(POLL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "failed",
                "details": { "message": "Merge conflict: the pull request could not be merged." }
            })))
            .mount(&server)
            .await;
        assert_eq!(
            run(&server, StackMergeAction::MergeQueue).await,
            StackMergeOutcome::Failed {
                message: "Merge conflict: the pull request could not be merged.".into()
            }
        );
    }

    /// Still pending at the deadline is "in progress", never "failed".
    #[tokio::test]
    async fn a_merge_still_running_at_the_deadline_is_in_progress_not_failed() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(PUT_PATH))
            .respond_with(ResponseTemplate::new(202).set_body_json(pending()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(POLL_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(pending()))
            .mount(&server)
            .await;
        match run(&server, StackMergeAction::MergeQueue).await {
            StackMergeOutcome::InProgress { message } => {
                assert!(message.contains("still in progress"), "{message}")
            }
            other => panic!("expected in progress, got {other:?}"),
        }
    }

    /// A refused submission (400, `failed`) carries GitHub's message, and
    /// nothing is polled.
    #[tokio::test]
    async fn a_refused_submission_surfaces_githubs_message() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(PUT_PATH))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "status": "failed",
                "details": { "message": "Pull request is a draft." }
            })))
            .mount(&server)
            .await;
        assert_eq!(
            run(&server, StackMergeAction::DirectMerge).await,
            StackMergeOutcome::Failed {
                message: "Pull request is a draft.".into()
            }
        );
        let polls = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.method.as_str() == "GET")
            .count();
        assert_eq!(polls, 0);
    }

    /// A poll that cannot be read is not a failed merge.
    #[tokio::test]
    async fn an_unreadable_poll_is_in_progress() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(PUT_PATH))
            .respond_with(ResponseTemplate::new(202).set_body_json(pending()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(POLL_PATH))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            run(&server, StackMergeAction::MergeQueue).await,
            StackMergeOutcome::InProgress { .. }
        ));
    }

    #[test]
    fn the_documented_statuses_read_as_documented() {
        assert!(read(404, &json!({})).is_err(), "unavailable is an error");
        assert!(read(422, &json!({ "message": "bad" })).is_err());
        assert_eq!(
            read(409, &pending()).unwrap(),
            Reading::Pending { uuid: "u-1".into() },
            "an existing request is followed, not repeated"
        );
        assert_eq!(
            read(200, &json!({ "status": "enqueued", "details": {} })).unwrap(),
            Reading::Done(StackMergeOutcome::Enqueued)
        );
        assert!(
            read(202, &json!({ "status": "pending", "details": {} })).is_err(),
            "pending without a uuid cannot be followed"
        );
        assert!(read(200, &json!({})).is_err());
    }

    #[test]
    fn actions_parse_from_the_wire_names() {
        assert_eq!(
            StackMergeAction::parse("merge_queue"),
            Ok(StackMergeAction::MergeQueue)
        );
        assert_eq!(
            StackMergeAction::parse("direct_merge"),
            Ok(StackMergeAction::DirectMerge)
        );
        assert!(StackMergeAction::parse("merge").is_err());
    }
}
