//! Bounded read retries. Writes use the client's non-retrying transport.
//! Diagnostics contain operation classes and numeric IDs, never query inputs.
use super::client::ClientError;
use octocrab::{FromResponse, Octocrab};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
use tokio::time::Instant;

pub(super) const READ_BUDGET: Duration = Duration::from_secs(30);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Default)]
pub(super) struct ReadTransport {
    graphql_cooldown: Mutex<Option<Instant>>,
    rest_cooldown: Mutex<Option<Instant>>,
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

impl ReadTransport {
    pub(super) async fn post(&self, client: &Octocrab, body: &Value) -> Result<Value, ClientError> {
        self.read(client, Read::Graphql(body), operation(body))
            .await
    }

    pub(super) async fn get(
        &self,
        client: &Octocrab,
        path: &str,
        budget: &crate::github::stats::Budget,
    ) -> Result<Value, ClientError> {
        self.read(client, Read::Rest { path, budget }, "rest-read")
            .await
    }

    fn cooldown(&self, read: Read<'_>) -> &Mutex<Option<Instant>> {
        match read {
            Read::Graphql(_) => &self.graphql_cooldown,
            Read::Rest { .. } => &self.rest_cooldown,
        }
    }

    fn remember_limit(&self, read: Read<'_>, seconds: u64) -> u64 {
        // Invalid or overflowing headers cannot panic or disable reads forever.
        // Concurrent responses can extend a cooldown, never shorten one.
        let seconds = seconds.clamp(1, 86_400);
        let until = Instant::now() + Duration::from_secs(seconds);
        let mut cooldown = self
            .cooldown(read)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *cooldown = Some(cooldown.map_or(until, |prior| prior.max(until)));
        seconds
    }

    async fn read(
        &self,
        client: &Octocrab,
        read: Read<'_>,
        operation: &'static str,
    ) -> Result<Value, ClientError> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut log = RequestLog {
            id,
            operation,
            started: Instant::now(),
            outcome: "cancelled",
        };
        let result = tokio::time::timeout(READ_BUDGET, self.attempts(client, read, id, operation))
            .await
            .unwrap_or(Err(ClientError::Timeout(READ_BUDGET.as_secs())));
        log.outcome = if result.is_ok() { "ok" } else { "failed" };
        result
    }

    async fn attempts(
        &self,
        client: &Octocrab,
        read: Read<'_>,
        id: u64,
        operation: &str,
    ) -> Result<Value, ClientError> {
        for attempt in 1..=2 {
            let wait = self
                .cooldown(read)
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(|until| until.saturating_duration_since(Instant::now()))
                .unwrap_or_default();
            if !wait.is_zero() {
                crate::diag!(
                    "[diag] provider read id={} operation={} cooldown_ms={}",
                    id,
                    operation,
                    wait.as_millis()
                );
                return Err(ClientError::RateLimited(format!(
                    "retry in {} seconds",
                    wait.as_secs().saturating_add(1)
                )));
            }
            let started = Instant::now();
            let response = match read {
                Read::Graphql(body) => client._post("/graphql", Some(body)).await,
                Read::Rest { path, .. } => client._get(path).await,
            };
            let result = match response {
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
                        budget.record_rest(remaining);
                    }
                    let retry_after = header_number("retry-after");
                    let reset = header_number("x-ratelimit-reset").map(seconds_until);
                    let exhausted = remaining == Some(0);
                    let refused =
                        status == 429 || (status == 403 && (retry_after.is_some() || exhausted));
                    if refused || exhausted {
                        let seconds =
                            self.remember_limit(read, retry_after.or(reset).unwrap_or(60));
                        crate::diag!(
                            "[diag] provider read id={} operation={} rate_limit_wait_seconds={}",
                            id,
                            operation,
                            seconds
                        );
                        if refused {
                            return Err(ClientError::RateLimited(format!(
                                "retry in {seconds} seconds"
                            )));
                        }
                        // A successful final-quota response is still an answer.
                        // Keep its data, but refuse the NEXT read until reset.
                    }
                    if status >= 500 {
                        Err(ClientError::NotJson(format!("HTTP {status} response")))
                    } else {
                        match octocrab::map_github_error(response).await {
                            Ok(response) => match Value::from_response(response)
                                .await
                                .map_err(map_error)
                            {
                                Ok(value) => {
                                    if matches!(read, Read::Graphql(_)) && graphql_exhausted(&value)
                                    {
                                        let body_reset = value
                                            .pointer("/data/rateLimit/resetAt")
                                            .and_then(Value::as_str)
                                            .and_then(|at| {
                                                chrono::DateTime::parse_from_rfc3339(at).ok()
                                            })
                                            .map(|at| seconds_until(at.timestamp().max(0) as u64));
                                        let seconds = self.remember_limit(
                                            read,
                                            retry_after.or(reset).or(body_reset).unwrap_or(60),
                                        );
                                        if value.get("data").is_none_or(Value::is_null) {
                                            return Err(ClientError::RateLimited(format!(
                                                "retry in {seconds} seconds"
                                            )));
                                        }
                                    }
                                    Ok(value)
                                }
                                Err(error) => Err(error),
                            },
                            Err(error) => Err(map_error(error)),
                        }
                    }
                }
            };
            match result {
                Err(ref error) if attempt == 1 && error.is_transient() => {
                    // The deadline includes the single bounded, jittered retry.
                    let delay = 250 + id % 251;
                    crate::diag!("[diag] provider read id={} operation={} attempt={} retry_delay_ms={} reason=transient", id, operation, attempt, delay);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                other => return other,
            }
        }
        unreachable!("the second attempt always returns")
    }
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
