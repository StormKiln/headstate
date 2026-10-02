//! Synthetic HTTP contracts for #1600. Disable Octocrab's automatic retries,
//! exactly as production does: otherwise a write can be repeated below our code.
use headstate_lib::github::{
    client::{ClientError, GitHubClient},
    mutate::ReviewVerdict,
};
use octocrab::service::middleware::retry::RetryConfig;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn client(server: &MockServer, account: &str) -> GitHubClient {
    GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token(account.to_string())
            .add_retry_config(RetryConfig::None)
            .build()
            .unwrap(),
    )
}

fn page() -> serde_json::Value {
    let body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/search.json")).unwrap();
    serde_json::json!({"data": body})
}

async fn recover_after_one_bad_response(status: u16, reviewing: bool) {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(status)
                    .set_body_string("<html>temporary upstream failure</html>")
            } else {
                ResponseTemplate::new(200).set_body_json(page())
            }
        })
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-account-a");
    let result = if reviewing {
        client.fetch_reviewing_snapshot().await
    } else {
        client.fetch_prs_snapshot().await
    }
    .unwrap();
    assert_eq!(result.prs.len(), 3);
    assert_eq!(result.total, Some(3));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn authored_read_recovers_from_one_503_in_two_attempts() {
    recover_after_one_bad_response(503, false).await;
}

#[tokio::test]
async fn reviewing_read_recovers_from_one_html_response_in_two_attempts() {
    recover_after_one_bad_response(200, true).await;
}

#[tokio::test]
async fn repeated_503_stops_after_two_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-account-a");
    let error = client
        .fetch_prs_snapshot()
        .await
        .err()
        .expect("both attempts failed");
    assert!(error.is_transient(), "{error}");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn retry_after_blocks_fresh_reads_but_not_a_different_account() {
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "60")
                    .set_body_json(serde_json::json!({"message": "rate limited"}))
            } else {
                ResponseTemplate::new(200).set_body_json(page())
            }
        })
        .mount(&server)
        .await;
    let first = client(&server, "synthetic-account-a");
    let error = first
        .fetch_prs_snapshot()
        .await
        .err()
        .expect("rate limit is a refusal");
    assert!(!error.is_transient(), "{error}");
    assert!(error.to_string().contains("retry in"), "{error}");
    // A sequential refresh and a different list kind must both honor the
    // account's cooldown, rather than treating it as a cached search result.
    assert!(first.fetch_prs_snapshot().await.is_err());
    assert!(first.fetch_reviewing_snapshot().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    let second = client(&server, "synthetic-account-b");
    assert_eq!(
        second.fetch_reviewing_snapshot().await.unwrap().prs.len(),
        3
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn rejected_credentials_are_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({"message": "Bad credentials"})),
        )
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-account-a");
    let error = client
        .fetch_reviewing_snapshot()
        .await
        .err()
        .expect("token is rejected");
    assert!(!error.is_transient(), "{error}");
    assert!(
        error.to_string().contains("GitHub rejected the token"),
        "{error}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn overlapping_failed_reads_share_the_same_two_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_millis(50)))
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-account-a");
    let (first, second) = tokio::join!(
        client.fetch_reviewing_snapshot(),
        client.fetch_reviewing_snapshot()
    );
    let first = first.err().expect("first caller must see failure");
    let second = second
        .err()
        .expect("second caller must see the shared failure");
    assert!(first.is_transient());
    assert_eq!(first.to_string(), second.to_string());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "overlapping consumers must share one operation, including its retry"
    );
}

#[tokio::test]
async fn a_review_write_with_a_503_is_sent_exactly_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(503).set_body_string("upstream failed after accepting write"),
        )
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-account-a");
    let error = client
        .add_review("PR_SYNTHETIC", ReviewVerdict::Approve, "synthetic review")
        .await
        .expect_err("the write was not acknowledged");
    assert!(matches!(error, ClientError::UnconfirmedWrite), "{error}");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "an uncertain write must never be retried"
    );
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(body["query"]
        .as_str()
        .unwrap()
        .contains("addPullRequestReview"));
    assert_eq!(body["variables"]["id"], "PR_SYNTHETIC");
}

#[test]
fn production_disables_octocrabs_automatic_retry_layer() {
    // The synthetic HTTP client above cannot validate production's builder.
    // Strip comments so documenting RetryConfig::None cannot satisfy this guard.
    let source = include_str!("../src/auth.rs");
    let no_retry = ".add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)";
    let configured = |source: &str| {
        let source = source.replace("\r\n", "\n");
        let builder = source
            .split("pub fn build_client(")
            .nth(1)
            .unwrap()
            .split("\n}")
            .next()
            .unwrap();
        let code: String = builder
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .flat_map(|line| line.chars().filter(|c| !c.is_whitespace()))
            .collect();
        code.contains(no_retry)
    };
    assert!(
        configured(source),
        "src/auth.rs: build_client must disable automatic retries before any mutation is sent"
    );
    assert!(
        configured(&source.replace('\n', "\r\n")),
        "CRLF must not change the guard"
    );
    assert!(
        !configured(&source.replace(no_retry, "")),
        "removing the policy must fail the guard"
    );
    assert!(
        !configured(&source.replace(no_retry, &format!("// {no_retry}"))),
        "a commented-out policy must fail the guard"
    );
}

#[tokio::test]
async fn graphql_http_200_rate_limit_enters_cooldown() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)
        .insert_header("x-ratelimit-remaining","0")
        .set_body_json(serde_json::json!({"data":null,"errors":[{"message":"API rate limit exceeded"}]})))
        .mount(&server).await;
    let client = client(&server, "synthetic-account-a");
    assert!(client.fetch_prs_snapshot().await.is_err());
    assert!(client.fetch_reviewing_snapshot().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn refused_rerun_is_not_reported_as_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(
                serde_json::json!({"message":"This workflow run cannot be retried"}),
            ),
        )
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-account-a");
    assert!(client.rerun_failed_jobs("synthetic/demo", 1).await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn graphql_body_rate_limit_without_headers_enters_cooldown() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)
        .set_body_json(serde_json::json!({"data":null,"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]})))
        .mount(&server).await;
    let client = client(&server, "synthetic-body-limit");
    assert!(client.fetch_prs_snapshot().await.is_err());
    assert!(client.fetch_reviewing_snapshot().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn exhausted_graphql_response_keeps_usable_data_and_blocks_the_next_read() {
    let server = MockServer::start().await;
    let mut body = page();
    body["errors"] =
        serde_json::json!([{"type":"RATE_LIMITED", "message":"API rate limit exceeded"}]);
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ratelimit-remaining", "0")
                .set_body_json(body),
        )
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-partial-limit");
    assert_eq!(client.fetch_prs_snapshot().await.unwrap().prs.len(), 3);
    assert!(client.fetch_reviewing_snapshot().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn rest_get_retries_once_and_meters_the_refused_attempt() {
    use headstate_lib::github::{
        gates::{base_rules, BaseRules},
        stats::Budget,
    };
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503).set_body_string("unavailable")
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!([]))
            }
        })
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-rest-retry");
    let budget = Budget::new();
    let rules = base_rules(&client, &budget, "synthetic/rest-retry", "main").await;
    assert!(matches!(rules, BaseRules::Read { .. }), "{rules:?}");
    assert_eq!(
        budget.rest_requests(),
        2,
        "even the refused response spent a request"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn graphql_secondary_cooldown_blocks_rest_without_dispatch() {
    use headstate_lib::github::{
        gates::{base_rules, BaseRules},
        stats::Budget,
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "60")
                .set_body_json(serde_json::json!({"message":"rate limited"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-separate-pools");
    assert!(client.fetch_prs_snapshot().await.is_err());
    let budget = Budget::new();
    let rules = base_rules(&client, &budget, "synthetic/separate-pools", "main").await;
    assert!(matches!(rules, BaseRules::Declined { .. }), "{rules:?}");
    assert_eq!(budget.rest_requests(), 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn successful_exhausted_reads_keep_the_last_answer() {
    for header_only in [true, false] {
        let server = MockServer::start().await;
        let mut body = page();
        let response = if header_only {
            ResponseTemplate::new(200).insert_header("x-ratelimit-remaining", "0")
        } else {
            body["data"]["rateLimit"]["remaining"] = 0.into();
            body["data"]["rateLimit"]["resetAt"] = (chrono::Utc::now()
                + chrono::Duration::minutes(1))
            .to_rfc3339()
            .into();
            ResponseTemplate::new(200)
        };
        Mock::given(method("POST"))
            .respond_with(response.set_body_json(body))
            .mount(&server)
            .await;
        let client = client(&server, "synthetic-last-answer");
        assert_eq!(client.fetch_prs_snapshot().await.unwrap().prs.len(), 3);
        assert!(client.fetch_reviewing_snapshot().await.is_err());
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "header_only={header_only}"
        );
    }
}

#[tokio::test]
async fn rest_secondary_cooldown_blocks_both_resources_without_another_dispatch() {
    use headstate_lib::github::{
        gates::{base_rules, BaseRules},
        stats::Budget,
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "60")
                .set_body_json(serde_json::json!({"message":"rate limited"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page()))
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-rest-pool-limit");
    let budget = Budget::new();
    let first = base_rules(&client, &budget, "synthetic/rest-pool-limit", "first").await;
    // The rules API intentionally classifies both a received rate-limit refusal
    // and local cooldown suppression as Declined; wire counts distinguish them.
    assert!(matches!(first, BaseRules::Declined { .. }), "{first:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    let second = base_rules(&client, &budget, "synthetic/rest-pool-limit", "second").await;
    assert!(matches!(second, BaseRules::Declined { .. }), "{second:?}");
    assert_eq!(
        budget.rest_requests(),
        1,
        "the cooldown refusal made no HTTP request"
    );
    assert!(client.fetch_prs_snapshot().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn primary_resource_quotas_are_independent_without_secondary_cooldown() {
    use headstate_lib::github::{
        gates::{base_rules, BaseRules},
        stats::Budget,
    };
    for exhausted_graphql in [true, false] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "x-ratelimit-remaining",
                        if exhausted_graphql { "0" } else { "5000" },
                    )
                    .set_body_json(page()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "x-ratelimit-remaining",
                        if exhausted_graphql { "5000" } else { "0" },
                    )
                    .set_body_json(serde_json::json!([])),
            )
            .mount(&server)
            .await;
        let client = client(&server, "synthetic-primary-resources");
        let budget = Budget::new();
        if exhausted_graphql {
            assert_eq!(client.fetch_prs_snapshot().await.unwrap().prs.len(), 3);
            assert!(client.fetch_reviewing_snapshot().await.is_err());
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
            assert!(matches!(
                base_rules(&client, &budget, "octocat/hello-world", "main").await,
                BaseRules::Read { .. }
            ));
        } else {
            assert!(matches!(
                base_rules(&client, &budget, "octocat/hello-world", "main").await,
                BaseRules::Read { .. }
            ));
            assert!(matches!(
                base_rules(&client, &budget, "octocat/hello-world", "other").await,
                BaseRules::Declined { .. }
            ));
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
            assert_eq!(client.fetch_prs_snapshot().await.unwrap().prs.len(), 3);
        }
        assert_eq!(budget.rest_requests(), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn rest_write_success_accepts_an_empty_acknowledgement() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/repos/synthetic/demo/actions/runs/1/rerun-failed-jobs",
        ))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    client(&server, "synthetic-account-a")
        .rerun_failed_jobs("synthetic/demo", 1)
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_uncertain_rest_write_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/repos/synthetic/demo/actions/runs/1/rerun-failed-jobs",
        ))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream unavailable"))
        .mount(&server)
        .await;
    let error = client(&server, "synthetic-account-a")
        .rerun_failed_jobs("synthetic/demo", 1)
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::UnconfirmedWrite), "{error}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn stalled_rest_write_has_a_total_deadline_and_uncertain_outcome() {
    let server = MockServer::start().await;
    let seen = Arc::new(AtomicUsize::new(0));
    let observed = seen.clone();
    Mock::given(method("POST"))
        .and(path(
            "/repos/synthetic/demo/actions/runs/1/rerun-failed-jobs",
        ))
        .respond_with(move |_: &wiremock::Request| {
            observed.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(201).set_delay(Duration::from_secs(60))
        })
        .mount(&server)
        .await;
    let client = client(&server, "synthetic-account-a");
    let write = tokio::spawn(async move { client.rerun_failed_jobs("synthetic/demo", 1).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while seen.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("write must reach the synthetic server");
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    let error = write.await.unwrap().unwrap_err();
    assert!(matches!(error, ClientError::UnconfirmedWrite), "{error}");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}
