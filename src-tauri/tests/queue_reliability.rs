use headstate_lib::github::client::GitHubClient;
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

async fn client(server: &MockServer) -> GitHubClient {
    GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("synthetic-token".to_string())
            .build()
            .unwrap(),
    )
}
fn page(total: u64) -> serde_json::Value {
    let mut body: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/search.json")).unwrap();
    body["authored"]["issueCount"] = total.into();
    serde_json::json!({"data":body})
}

#[tokio::test]
async fn overlapping_review_loads_share_one_request_but_later_refresh_is_fresh() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(50))
                .set_body_json(page(3)),
        )
        .mount(&server)
        .await;
    let client = client(&server).await;
    let (a, b) = tokio::join!(
        client.fetch_reviewing_snapshot(),
        client.fetch_reviewing_snapshot()
    );
    assert_eq!(a.unwrap().prs.len(), 3);
    assert_eq!(b.unwrap().prs.len(), 3);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    client.fetch_reviewing_snapshot().await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn dropping_search_cancels_later_page_before_it_can_retry() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(move |r: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            if !body["variables"]["after"].is_null() {
                calls.fetch_add(1, Ordering::SeqCst);
                return ResponseTemplate::new(503).set_delay(Duration::from_millis(250));
            }
            ResponseTemplate::new(200).set_body_json(page(50))
        })
        .mount(&server)
        .await;
    let client = client(&server).await;
    assert!(tokio::time::timeout(
        Duration::from_millis(100),
        client.fetch_reviewing_snapshot()
    )
    .await
    .is_err());
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "canceled page retried in the background"
    );
}

#[tokio::test]
async fn deadline_keeps_first_page_and_reports_missing_pages() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(|r: &wiremock::Request| {
            let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            let response = ResponseTemplate::new(200).set_body_json(page(50));
            if v["variables"]["after"].is_null() {
                response
            } else {
                response.set_delay(Duration::from_secs(2))
            }
        })
        .mount(&server)
        .await;
    let client = client(&server).await;
    let result = client
        .fetch_reviewing_snapshot_with_budget(Duration::from_millis(150))
        .await
        .unwrap();
    assert_eq!(result.prs.len(), 3);
    assert!(matches!(
        result.coverage,
        headstate_lib::store::source_cache::Coverage::Partial { total: Some(50) }
    ));
}

#[tokio::test]
async fn shared_failure_does_not_start_a_second_identical_load() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_delay(Duration::from_millis(50))
                .set_body_json(serde_json::json!({"message":"Bad credentials"})),
        )
        .mount(&server)
        .await;
    let client = client(&server).await;
    let (a, b) = tokio::join!(
        client.fetch_reviewing_snapshot(),
        client.fetch_reviewing_snapshot()
    );
    assert!(!a.err().unwrap().is_transient());
    assert!(!b.err().unwrap().is_transient());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn clients_and_list_kinds_do_not_share_results() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(50))
                .set_body_json(page(3)),
        )
        .mount(&server)
        .await;
    let a = client(&server).await;
    let b = client(&server).await;
    let (x, y, z) = tokio::join!(
        a.fetch_reviewing_snapshot(),
        a.fetch_prs_snapshot(),
        b.fetch_reviewing_snapshot()
    );
    assert!(x.is_ok() && y.is_ok() && z.is_ok());
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn overlapping_details_share_the_document_and_optional_stack_lookup() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(40))
                .set_body_json(
                    serde_json::json!({"data":{"repository":{"pullRequest":{"number":7}}}}),
                ),
        )
        .mount(&server)
        .await;
    let client = client(&server).await;
    let (a, b) = tokio::join!(
        client.fetch_pr_detail("synthetic/demo", 7),
        client.fetch_pr_detail("synthetic/demo", 7)
    );
    assert!(a.is_ok() && b.is_ok());
    let requests = server.received_requests().await.unwrap();
    let details = requests
        .iter()
        .filter(|r| {
            let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            v["query"].as_str() == Some(headstate_lib::github::query::PR_DETAIL_QUERY)
        })
        .count();
    assert_eq!(details, 1, "the same detail read was fetched twice");
}

#[tokio::test]
async fn post_write_refresh_does_not_wait_behind_an_old_generation() {
    use headstate_lib::github::mutate::ReviewVerdict;
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("POST")).respond_with(move |r:&wiremock::Request| {
        let v:serde_json::Value=serde_json::from_slice(&r.body).unwrap();
        if v["query"].as_str().unwrap_or_default().contains("addPullRequestReview") {
            return ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":{"addPullRequestReview":{"pullRequestReview":{"state":"APPROVED"}}}}));
        }
        let response=ResponseTemplate::new(200).set_body_json(page(3));
        if seen.fetch_add(1,Ordering::SeqCst)==0 {response.set_delay(Duration::from_secs(2))} else {response}
    }).mount(&server).await;
    let client = client(&server).await;
    let old_client = client.clone();
    let old = tokio::spawn(async move { old_client.fetch_reviewing_snapshot().await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client
        .add_review("synthetic", ReviewVerdict::Approve, "")
        .await
        .unwrap();
    let fresh = client
        .fetch_reviewing_snapshot_with_budget(Duration::from_millis(150))
        .await;
    old.abort();
    assert!(
        fresh.is_ok(),
        "post-write refresh spent its budget behind a stale read"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn post_write_detail_does_not_wait_behind_an_old_generation() {
    use headstate_lib::github::mutate::ReviewVerdict;
    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("POST"))
        .respond_with(move |r: &wiremock::Request| {
            let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            let query = v["query"].as_str().unwrap_or_default();
            if query.contains("addPullRequestReview") {
                return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "data":{"addPullRequestReview":{"pullRequestReview":{"state":"APPROVED"}}}
                }));
            }
            let response = ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data":{"repository":{"pullRequest":{"number":7}}}
            }));
            if query == headstate_lib::github::query::PR_DETAIL_QUERY
                && seen.fetch_add(1, Ordering::SeqCst) == 0
            {
                response.set_delay(Duration::from_secs(5))
            } else {
                response
            }
        })
        .mount(&server)
        .await;
    let client = client(&server).await;
    let old_client = client.clone();
    let old = tokio::spawn(async move { old_client.fetch_pr_detail("synthetic/demo", 7).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(client.has_interactive_reads());
    client
        .add_review("synthetic", ReviewVerdict::Approve, "")
        .await
        .unwrap();
    let fresh = tokio::time::timeout(
        Duration::from_secs(2),
        client.fetch_pr_detail("synthetic/demo", 7),
    )
    .await;
    old.abort();
    let _ = old.await;
    assert!(fresh
        .expect("new generation waited behind old detail")
        .is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        !client.has_interactive_reads(),
        "cancelled reads must release admission counters"
    );
}
