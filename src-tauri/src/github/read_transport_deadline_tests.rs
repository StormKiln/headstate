use super::super::{admission::ReadClass, client::GitHubClient, stats::Budget};
use super::*;
use serde_json::json;
use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> Octocrab {
    Octocrab::builder()
        .base_uri(server.uri())
        .unwrap()
        .personal_token("synthetic")
        .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
        .build()
        .unwrap()
}
async fn read(
    transport: &ReadTransport,
    client: &Octocrab,
    graphql: bool,
) -> Result<Value, ClientError> {
    if graphql {
        transport.post(client, &json!({"query":"synthetic"})).await
    } else {
        transport
            .get(
                client,
                "/synthetic",
                &Budget::new(),
                ReadContext::new(ReadClass::Foreground, READ_BUDGET),
            )
            .await
    }
}
async fn write(client: &GitHubClient, graphql: bool) -> Result<Value, ClientError> {
    if graphql {
        client
            .graphql_mutation_data(&json!({"query":"mutation { synthetic }"}))
            .await
    } else {
        client
            .rest_put("/synthetic", &json!({}), &client.request_budget())
            .await
            .map(|(_, value)| value)
    }
}

#[tokio::test]
async fn actual_read_and_write_transports_keep_independent_header_constraints() {
    // Synthetic provider shapes, not assertions about observed live accounts.
    for graphql in [true, false] {
        for writing in [false, true] {
            for (remaining, reset, retry, status, advance, allowed) in [
                (Some("1000"), Some(1800), None, 429, 61, true),
                (Some("0"), Some(1800), Some("1"), 429, 2, false),
                (Some("0"), Some(1800), Some("3600"), 429, 1801, false),
                (None, None, None, 429, 61, true),
                (Some("invalid"), None, Some("invalid"), 429, 61, true),
                (Some("1000"), Some(1800), Some("2"), 429, 3, true),
                (Some("1000"), Some(1800), Some("date"), 429, 3, true),
                (Some("0"), Some(1800), None, 200, 2, false),
                (Some("1000"), Some(1800), None, 403, 0, true),
            ] {
                let server = MockServer::start().await;
                let octocrab = client(&server);
                let base = GitHubClient::new(octocrab.clone());
                let transport = ReadTransport::default();
                let mut response = ResponseTemplate::new(status)
                    .set_body_json(json!({"data":{"useful":true},"message":"synthetic refusal"}));
                if let Some(remaining) = remaining {
                    response = response.insert_header("x-ratelimit-remaining", remaining);
                }
                if let Some(reset) = reset {
                    response = response.insert_header(
                        "x-ratelimit-reset",
                        (chrono::Utc::now().timestamp() + reset).to_string(),
                    );
                }
                if let Some(retry) = retry {
                    response = response.insert_header(
                        "retry-after",
                        if retry == "date" {
                            (chrono::Utc::now() + chrono::Duration::seconds(2)).to_rfc2822()
                        } else {
                            retry.to_string()
                        },
                    );
                }
                Mock::given(any())
                    .respond_with(response)
                    .mount(&server)
                    .await;
                let first = if writing {
                    write(&base, graphql).await
                } else {
                    read(&transport, &octocrab, graphql).await
                };
                if status == 200 {
                    assert!(first.is_ok(), "useful final-quota data is retained");
                }
                assert_eq!(
                    server.received_requests().await.unwrap().len(),
                    1,
                    "refusals/writes must not retry"
                );
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(advance)).await;
                tokio::time::resume();
                server.reset().await;
                Mock::given(any())
                    .respond_with(
                        ResponseTemplate::new(200).set_body_json(json!({"data":{"useful":true}})),
                    )
                    .mount(&server)
                    .await;
                let after = if writing {
                    write(&base, graphql).await
                } else {
                    read(&transport, &octocrab, graphql).await
                };
                assert_eq!(after.is_ok(), allowed, "graphql={graphql} writing={writing} remaining={remaining:?} retry={retry:?} status={status}");
                assert_eq!(
                    server.received_requests().await.unwrap().len(),
                    usize::from(allowed)
                );
            }
        }
    }
}

#[tokio::test]
async fn actual_graphql_partial_bodies_keep_data_and_independent_deadlines_for_reads_and_writes() {
    for writing in [false, true] {
        for (remaining, retry, advance, allowed) in [
            (1000, None, 61, true),
            (0, Some("1"), 2, false),
            (0, Some("3600"), 1801, false),
        ] {
            let server = MockServer::start().await;
            let octocrab = client(&server);
            let base = GitHubClient::new(octocrab.clone());
            let transport = ReadTransport::default();
            let mut response = ResponseTemplate::new(200).set_body_json(json!({"data":{"useful":true,"rateLimit":{"remaining":remaining,"resetAt":(chrono::Utc::now()+chrono::Duration::seconds(1800)).to_rfc3339()}},"errors":[{"type":"RATE_LIMITED","message":"secondary rate limit"}]}));
            if let Some(retry) = retry {
                response = response.insert_header("retry-after", retry);
            }
            Mock::given(any())
                .respond_with(response)
                .mount(&server)
                .await;
            if writing {
                assert!(write(&base, true).await.is_err());
            } else {
                assert_eq!(
                    read(&transport, &octocrab, true).await.unwrap()["data"]["useful"],
                    true
                );
            }
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(advance)).await;
            tokio::time::resume();
            server.reset().await;
            Mock::given(any())
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"data":{"useful":true}})),
                )
                .mount(&server)
                .await;
            let after = if writing {
                write(&base, true).await
            } else {
                read(&transport, &octocrab, true).await
            };
            assert_eq!(
                after.is_ok(),
                allowed,
                "writing={writing} remaining={remaining} retry={retry:?}"
            );
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                usize::from(allowed)
            );
        }
    }
}
