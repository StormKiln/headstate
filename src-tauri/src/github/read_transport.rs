//! Bounded read retries. Writes use the client's non-retrying transport.
//! Diagnostics contain operation classes and numeric IDs, never query inputs.
use super::admission::{retry_seconds, Admission, Bucket, ReadContext};
use super::client::ClientError;
use octocrab::{FromResponse, Octocrab};
use serde_json::Value;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::time::Instant;

pub(super) const READ_BUDGET: Duration = Duration::from_secs(30);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Default, Debug)]
pub(super) struct ReadTransport {
    pub(super) admission: Admission,
}

fn operation(body: &Value) -> &'static str {
    let query = body["query"].as_str().unwrap_or_default();
    if query == super::query::PRS_QUERY {
        if body["variables"]["q"]
            .as_str()
            .is_some_and(|q| q.contains("review-requested:"))
        {
            "reviewing"
        } else {
            "authored"
        }
    } else if query == super::query::PR_DETAIL_QUERY {
        "detail"
    } else {
        "supporting-read"
    }
}

fn map_error(error: octocrab::Error) -> ClientError {
    match &error {
        octocrab::Error::Serde { .. } | octocrab::Error::Json { .. } => {
            ClientError::NotJson("non-JSON response".into())
        }
        octocrab::Error::GitHub { source, .. } if source.status_code.as_u16() == 401 => {
            ClientError::TokenRejected {
                said: source.message.clone(),
            }
        }
        _ => ClientError::Api(error),
    }
}

// The guard reports cancellation as well as completion: an outer operation
// deadline dropping this future must not look like an unexplained missing end.
struct RequestLog {
    id: u64,
    operation: &'static str,
    started: Instant,
    outcome: &'static str,
}
impl Drop for RequestLog {
    fn drop(&mut self) {
        crate::diag!(
            "[diag] provider read id={} operation={} outcome={} elapsed_ms={} budget_ms={}",
            self.id,
            self.operation,
            self.outcome,
            self.started.elapsed().as_millis(),
            READ_BUDGET.as_millis()
        );
    }
}

#[derive(Clone, Copy)]
enum Read<'a> {
    Graphql(&'a Value),
    Rest {
        path: &'a str,
        budget: &'a crate::github::stats::Budget,
    },
}

impl Read<'_> {
    fn bucket(self) -> Bucket {
        match self {
            Self::Graphql(_) => Bucket::Graphql,
            Self::Rest { .. } => Bucket::Rest,
        }
    }
}
impl ReadTransport {
    #[cfg(test)]
    pub(super) async fn post(&self, client: &Octocrab, body: &Value) -> Result<Value, ClientError> {
        self.post_with(
            client,
            body,
            ReadContext::new(super::admission::ReadClass::Foreground, READ_BUDGET),
        )
        .await
    }
    pub(super) async fn post_with(
        &self,
        client: &Octocrab,
        body: &Value,
        context: ReadContext,
    ) -> Result<Value, ClientError> {
        self.read(client, Read::Graphql(body), operation(body), context)
            .await
    }
    pub(super) async fn get(
        &self,
        client: &Octocrab,
        path: &str,
        budget: &crate::github::stats::Budget,
        context: ReadContext,
    ) -> Result<Value, ClientError> {
        self.read(client, Read::Rest { path, budget }, "rest-read", context)
            .await
    }
    pub(super) fn observe_headers(&self, bucket: Bucket, status: u16, headers: &hyper::HeaderMap) {
        let number = |key| {
            headers
                .get(key)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
        };
        let remaining = number("x-ratelimit-remaining");
        let reset = number("x-ratelimit-reset");
        let current_window = self.admission.observe(bucket, remaining, reset);
        let retry = headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| retry_seconds(v, chrono::Utc::now()));
        let exhausted = remaining == Some(0);
        let refused = status == 429 || (status == 403 && (retry.is_some() || exhausted));
        if exhausted && current_window {
            self.admission
                .limit(bucket, reset.map(seconds_until).unwrap_or(60), false);
        }
        if (refused && !exhausted) || (refused && retry.is_some()) {
            self.admission.limit(bucket, retry.unwrap_or(60), true);
        }
    }

    pub(super) fn observe_graphql(&self, value: &Value, retry: Option<u64>) {
        let quota = &value["data"]["rateLimit"];
        let reset = quota["resetAt"]
            .as_str()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|v| v.timestamp().max(0) as u64);
        let remaining = quota["remaining"].as_u64();
        let current_window = self.admission.observe(Bucket::Graphql, remaining, reset);
        if remaining == Some(0) && current_window {
            self.admission.limit(
                Bucket::Graphql,
                reset.map(seconds_until).unwrap_or(60),
                false,
            );
        }
        let secondary = value
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| {
                errors
                    .iter()
                    .any(|error| secondary_message(error["message"].as_str().unwrap_or_default()))
            });
        if secondary || (graphql_exhausted(value) && (remaining != Some(0) || retry.is_some())) {
            self.admission
                .limit(Bucket::Graphql, retry.unwrap_or(60), true);
        }
    }

    pub(super) fn observe_rest_body(&self, value: &Value, retry: Option<u64>) {
        if secondary_message(value["message"].as_str().unwrap_or_default()) {
            self.admission
                .limit(Bucket::Rest, retry.unwrap_or(60), true);
        }
    }
    pub(super) fn observe_error(
        &self,
        bucket: Bucket,
        error: &octocrab::Error,
        retry: Option<u64>,
    ) -> bool {
        if matches!(error, octocrab::Error::GitHub { source, .. } if secondary_message(&source.message))
        {
            self.admission.limit(bucket, retry.unwrap_or(60), true);
            true
        } else {
            false
        }
    }

    async fn read(
        &self,
        client: &Octocrab,
        read: Read<'_>,
        operation: &'static str,
        mut context: ReadContext,
    ) -> Result<Value, ClientError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut log = RequestLog {
            id,
            operation,
            started: Instant::now(),
            outcome: "cancelled",
        };
        if context.live.is_none() {
            context.deadline = context.deadline.min(Instant::now() + READ_BUDGET);
        }
        let result = if context.live.is_some() {
            // Live queued consumers own admission lifetime. The admitted
            // request freezes a bounded deadline inside attempts().
            self.attempts(client, read, id, operation, context).await
        } else {
            tokio::time::timeout_at(
                context.deadline,
                self.attempts(client, read, id, operation, context),
            )
            .await
            .unwrap_or(Err(ClientError::Timeout(READ_BUDGET.as_secs())))
        };
        log.outcome = if result.is_ok() { "ok" } else { "failed" };
        result
    }

    async fn attempts(
        &self,
        client: &Octocrab,
        read: Read<'_>,
        id: u64,
        operation: &str,
        context: ReadContext,
    ) -> Result<Value, ClientError> {
        for attempt in 1..=2 {
            let mut permit = self.admission.read(read.bucket(), context.clone()).await?;
            let started = Instant::now();
            let request_deadline = permit
                .deadline
                .unwrap_or(context.deadline)
                // First admission publishes the shared execution cap. Include
                // it in the first request as well as all subsequent documents.
                .min(
                    context
                        .live
                        .as_ref()
                        .and_then(|live| live.borrow().map(|d| d.deadline))
                        .unwrap_or(context.deadline),
                )
                .min(Instant::now() + READ_BUDGET);
            let request = async {
                let response = match read {
                    Read::Graphql(body) => client._post("/graphql", Some(body)).await,
                    Read::Rest { path, .. } => client._get(path).await,
                };
                // Headers, refused-body decoding and successful-body decoding
                // are one admitted request, with one frozen deadline/permit.
                match response {
                    Err(error) => Err(map_error(error)),
                    Ok(response) => {
                        let status = response.status().as_u16();
                        crate::diag!("[diag] provider read id={} operation={} attempt={} status={} headers_ms={}", id, operation, attempt, status, started.elapsed().as_millis());
                        let header_number = |name| {
                            response
                                .headers()
                                .get(name)
                                .and_then(|v| v.to_str().ok())
                                .and_then(|s| s.trim().parse::<u64>().ok())
                        };
                        let remaining = header_number("x-ratelimit-remaining");
                        if let Read::Rest { budget, .. } = read {
                            // A refused request spent quota too. Record its headers
                            // before any status mapping, rate-limit return or retry.
                            budget.record_rest_local(remaining);
                        }
                        let retry_after = response
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| retry_seconds(v, chrono::Utc::now()));
                        self.observe_headers(read.bucket(), status, response.headers());
                        let exhausted = remaining == Some(0);
                        let refused = status == 429
                            || (status == 403 && (retry_after.is_some() || exhausted));
                        // Still decode refused bodies: they may carry independent
                        // secondary evidence alongside primary header exhaustion.
                        if status >= 500 {
                            Err(ClientError::NotJson(format!("HTTP {status} response")))
                        } else {
                            match octocrab::map_github_error(response).await {
                                Ok(response) => {
                                    match Value::from_response(response).await.map_err(map_error) {
                                        Ok(value) => {
                                            if matches!(read, Read::Graphql(_)) {
                                                self.observe_graphql(&value, retry_after);
                                                if graphql_exhausted(&value)
                                                    && value.get("data").is_none_or(Value::is_null)
                                                {
                                                    return Err(ClientError::RateLimited(
                                                        "provider retry deadline is active".into(),
                                                    ));
                                                }
                                            }
                                            Ok(value)
                                        }
                                        Err(error) => Err(error),
                                    }
                                }
                                Err(error) => {
                                    if self.observe_error(read.bucket(), &error, retry_after)
                                        || refused
                                    {
                                        Err(ClientError::RateLimited(if refused {
                                            format!(
                                                "retry in {} seconds",
                                                self.admission.retry_wait(read.bucket())
                                            )
                                        } else {
                                            "provider retry deadline is active".into()
                                        }))
                                    } else {
                                        Err(map_error(error))
                                    }
                                }
                            }
                        }
                    }
                }
            };
            let result = tokio::time::timeout_at(request_deadline, request)
                .await
                .unwrap_or(Err(ClientError::Timeout(READ_BUDGET.as_secs())));
            if result.is_ok() {
                permit.complete();
            }
            drop(permit);
            match result {
                Err(ref error)
                    if attempt == 1
                        && error.is_transient()
                        && context
                            .attempts
                            .as_ref()
                            .is_none_or(|allowance| allowance.remaining() > 0) =>
                {
                    // The deadline includes the single bounded, jittered retry.
                    let delay = 250 + id % 251;
                    crate::diag!("[diag] provider read id={} operation={} attempt={} retry_delay_ms={} reason=transient", id, operation, attempt, delay);
                    if tokio::time::timeout_at(
                        request_deadline,
                        tokio::time::sleep(Duration::from_millis(delay)),
                    )
                    .await
                    .is_err()
                    {
                        return result;
                    }
                }
                other => return other,
            }
        }
        unreachable!("the second attempt always returns")
    }
}

pub(super) fn response_retry(headers: &hyper::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| retry_seconds(v, chrono::Utc::now()))
}
fn secondary_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("secondary rate limit") || message.contains("abuse detection")
}

fn seconds_until(epoch: u64) -> u64 {
    epoch.saturating_sub(chrono::Utc::now().timestamp().max(0) as u64)
}

fn graphql_exhausted(value: &Value) -> bool {
    value
        .pointer("/data/rateLimit/remaining")
        .and_then(Value::as_u64)
        == Some(0)
        || value
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| {
                errors.iter().any(|error| {
                    error.get("type").and_then(Value::as_str) == Some("RATE_LIMITED")
                        || error
                            .get("message")
                            .and_then(Value::as_str)
                            .is_some_and(|message| {
                                message.to_ascii_lowercase().contains("rate limit")
                            })
                })
            })
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::sync::Arc;
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

    async fn client(server: &MockServer) -> Octocrab {
        Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("synthetic")
            .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn stale_primary_write_headers_do_not_hide_fresh_secondary_body_evidence() {
        use super::super::client::GitHubClient;
        let server = MockServer::start().await;
        let base = GitHubClient::new(client(&server).await);
        let reset = chrono::Utc::now().timestamp() + 1800;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-reset", reset.to_string())
                    .insert_header("x-ratelimit-remaining", "1000")
                    .set_body_json(serde_json::json!({"ok":true})),
            )
            .mount(&server)
            .await;
        base.rest_put("/synthetic", &serde_json::json!({}), &base.request_budget())
            .await
            .unwrap();
        server.reset().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-reset", (reset - 3600).to_string())
                    .insert_header("x-ratelimit-remaining", "0")
                    .set_body_json(
                        serde_json::json!({"message":"You have exceeded a secondary rate limit"}),
                    ),
            )
            .mount(&server)
            .await;
        base.rest_put("/synthetic", &serde_json::json!({}), &base.request_budget())
            .await
            .unwrap();
        let _ = base
            .stats_graphql(&serde_json::json!({"query":"synthetic"}))
            .await;
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "new secondary body must suppress other-protocol HTTP despite stale primary window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn independent_primary_secondary_header_deadlines() {
        for bucket in [Bucket::Graphql, Bucket::Rest] {
            for (remaining, retry, after, allowed) in [
                (1000, None, 61, true),
                (0, Some("1"), 2, false),
                (0, Some("3600"), 1801, false),
            ] {
                let transport = ReadTransport::default();
                let mut headers = hyper::HeaderMap::new();
                headers.insert(
                    "x-ratelimit-remaining",
                    remaining.to_string().parse().unwrap(),
                );
                headers.insert(
                    "x-ratelimit-reset",
                    (chrono::Utc::now().timestamp() + 1800)
                        .to_string()
                        .parse()
                        .unwrap(),
                );
                if let Some(retry) = retry {
                    headers.insert("retry-after", retry.parse().unwrap());
                }
                transport.observe_headers(bucket, 429, &headers);
                tokio::time::advance(Duration::from_secs(after)).await;
                assert_eq!(
                    transport.admission.write(bucket).is_ok(),
                    allowed,
                    "remaining={remaining} retry={retry:?}"
                );
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_past_and_extreme_primary_hints_remain_bounded_or_saturating() {
        for (reset, after, allowed) in [
            ("malformed".to_string(), 2, false),
            ("malformed".to_string(), 61, true),
            ("0".to_string(), 2, true),
            (u64::MAX.to_string(), 3600, false),
        ] {
            let transport = ReadTransport::default();
            let mut headers = hyper::HeaderMap::new();
            headers.insert("x-ratelimit-reset", reset.parse().unwrap());
            headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
            headers.insert("retry-after", "1".parse().unwrap());
            transport.observe_headers(Bucket::Graphql, 429, &headers);
            tokio::time::advance(Duration::from_secs(after)).await;
            assert_eq!(
                transport.admission.write(Bucket::Graphql).is_ok(),
                allowed,
                "reset={reset}"
            );
        }
    }

    #[tokio::test]
    async fn foreground_identity_waiter_outlives_background_leaders_deadline() {
        use super::super::{admission::ReadClass, client::GitHubClient};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(serde_json::json!({"data":{"viewer":{"login":"synthetic"}}})),
            )
            .expect(2)
            .mount(&server)
            .await;
        let base = GitHubClient::new(client(&server).await);
        let background = base.with_read_context(ReadContext::new(
            ReadClass::Background,
            Duration::from_millis(40),
        ));
        let worker = tokio::spawn(async move {
            background
                .stats_viewer_metered(&background.request_budget())
                .await
        });
        while server.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        let foreground = base.with_read_context(ReadContext::new(
            ReadClass::Foreground,
            Duration::from_secs(1),
        ));
        assert_eq!(
            foreground
                .stats_viewer_metered(&foreground.request_budget())
                .await
                .unwrap(),
            "synthetic"
        );
        assert!(worker.await.unwrap().is_err());
        let expired = base
            .with_read_context(ReadContext::new(ReadClass::Background, Duration::ZERO))
            .with_read_context(ReadContext::new(
                ReadClass::Foreground,
                Duration::from_secs(10),
            ));
        assert!(expired
            .stats_graphql(&serde_json::json!({"query":"synthetic"}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn old_window_exhaustion_does_not_block_a_new_window() {
        let transport = ReadTransport::default();
        let reset = chrono::Utc::now().timestamp() as u64 + 3600;
        let mut headers = hyper::HeaderMap::new();
        headers.insert("x-ratelimit-reset", reset.to_string().parse().unwrap());
        headers.insert("x-ratelimit-remaining", "5000".parse().unwrap());
        transport.observe_headers(Bucket::Rest, 200, &headers);
        headers.insert(
            "x-ratelimit-reset",
            (reset - 3600).to_string().parse().unwrap(),
        );
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        transport.observe_headers(Bucket::Rest, 403, &headers);
        assert!(transport.admission.write(Bucket::Rest).is_ok());
    }

    #[tokio::test]
    async fn scoped_clients_share_limits_and_preserve_foreground_stats_slots() {
        use super::super::{admission::ReadClass, client::GitHubClient};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(500))
                    .set_body_json(serde_json::json!({"data":{"ok":true}})),
            )
            .mount(&server)
            .await;
        let base = GitHubClient::new(client(&server).await);
        let mut tasks = tokio::task::JoinSet::new();
        for class in [ReadClass::Background, ReadClass::Advisory] {
            let scoped = base.with_read_context(ReadContext::new(class, Duration::from_secs(2)));
            tasks.spawn(async move {
                scoped
                    .stats_graphql(&serde_json::json!({"query":"background"}))
                    .await
            });
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while server.received_requests().await.unwrap().len() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let scoped = base.with_read_context(ReadContext::new(
            ReadClass::Background,
            Duration::from_secs(2),
        ));
        tasks.spawn(async move {
            scoped
                .stats_graphql(&serde_json::json!({"query":"waiting"}))
                .await
        });
        for _ in 0..2 {
            let user = base.clone();
            tasks.spawn(async move {
                user.stats_graphql(&serde_json::json!({"query":"foreground"}))
                    .await
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.body_json::<Value>().unwrap()["query"] == "foreground")
                .count(),
            2
        );
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        // Cancellation releases transport capacity; base view did not inherit a scoped deadline.
        assert_eq!(base.read_context().class, ReadClass::Foreground);
        assert!(base
            .stats_graphql(&serde_json::json!({"query":"after cancellation"}))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn advisory_retries_spend_actual_attempts_and_foreground_remains_eligible() {
        use super::super::{admission::ReadClass, client::GitHubClient};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let base = GitHubClient::new(client(&server).await);
        let advisory = base.with_read_context(ReadContext::new(
            ReadClass::Advisory,
            Duration::from_secs(10),
        ));
        for _ in 0..4 {
            assert!(advisory
                .stats_graphql(&serde_json::json!({"query":"synthetic"}))
                .await
                .is_err());
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 8);
        assert!(matches!(
            advisory
                .stats_graphql(&serde_json::json!({"query":"synthetic"}))
                .await,
            Err(ClientError::NotDispatched(_))
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 8);
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":{"ok":true}})),
            )
            .mount(&server)
            .await;
        assert!(base
            .stats_graphql(&serde_json::json!({"query":"user stats"}))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn account_bound_budget_ignores_other_clients_observations() {
        let server = MockServer::start().await;
        let a = super::super::client::GitHubClient::new(client(&server).await);
        let b = super::super::client::GitHubClient::new(client(&server).await);
        // Use the transport through a real response; never modify test-shared legacy globals.
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data":{"rateLimit":{"remaining":499}}})),
            )
            .mount(&server)
            .await;
        a.stats_graphql(&serde_json::json!({"query":"synthetic"}))
            .await
            .unwrap();
        assert!(!a.request_budget().permits(1));
        assert!(b.request_budget().permits(1));
    }

    #[tokio::test]
    async fn write_during_known_cooldown_is_definitely_not_dispatched() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "60")
                    .set_body_json(serde_json::json!({"message":"limited"})),
            )
            .mount(&server)
            .await;
        let client = super::super::client::GitHubClient::new(client(&server).await);
        let _ = client
            .graphql_partial_ok(&serde_json::json!({"query":"synthetic"}))
            .await;
        let result = client
            .graphql_mutation_data(&serde_json::json!({"query":"mutation { synthetic }"}))
            .await;
        assert!(
            matches!(result, Err(ClientError::NotDispatched(_))),
            "{result:?}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn concurrent_reads_share_four_transport_slots() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(500))
                    .set_body_json(serde_json::json!({"data": {"ok": true}})),
            )
            .mount(&server)
            .await;
        let client = Arc::new(client(&server).await);
        let transport = Arc::new(ReadTransport::default());
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..6 {
            let (client, transport) = (client.clone(), transport.clone());
            tasks.spawn(async move {
                transport
                    .post(&client, &serde_json::json!({"query":"synthetic"}))
                    .await
            });
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
}

#[cfg(test)]
#[path = "read_transport_deadline_tests.rs"]
mod deadline_tests;
