//! Real transport schedules for the shared operation's consumer lifetimes.
use super::*;
use std::time::Duration;
use wiremock::{
    matchers::{body_partial_json, method},
    Mock, MockServer, ResponseTemplate,
};

async fn fixture() -> (MockServer, GitHubClient) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"data":{"viewer":{"login":"fixture"}}})),
        )
        .mount(&server)
        .await;
    let client = GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(server.uri())
            .unwrap()
            .personal_token("synthetic")
            .build()
            .unwrap(),
    );
    client.fetch_viewer().await.unwrap();
    server.reset().await;
    Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        let mut data = json!({"viewer":{"login":"fixture"},"authored":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}});
        for i in 0..4 {
            if let Some(n) = body["variables"][format!("n{i}")].as_u64() {
                let mut raw: Value = serde_json::from_str(include_str!("../../../tests/fixtures/search.json")).unwrap();
                let mut node = raw["authored"]["nodes"][0].take();
                node["id"] = json!(format!("PR-{n}"));
                node["number"] = json!(n);
                node["state"] = json!("CLOSED");
                data[format!("p{i}")] = json!({"pullRequest":node});
                data[format!("m{i}")] = json!({"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}});
            }
        }
        ResponseTemplate::new(200).set_delay(Duration::from_millis(80)).set_body_json(json!({"data":data}))
    }).mount(&server).await;
    (server, client)
}
async fn requests(server: &MockServer, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while server.received_requests().await.unwrap().len() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected synthetic HTTP dispatch");
}
async fn callers(client: &GitHubClient, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let slot = client
                .scans
                .0
                .lock()
                .unwrap()
                .get(&(false, 0))
                .and_then(Weak::upgrade);
            if slot.is_some_and(|slot| slot.state.lock().unwrap().callers.len() == count) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected live scan registrations");
}
async fn blockers(
    server: &MockServer,
    client: &GitHubClient,
    all: bool,
) -> Vec<tokio::task::JoinHandle<()>> {
    Mock::given(body_partial_json(json!({"query":"blocker"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_json(json!({"data":{}})),
        )
        .with_priority(1)
        .mount(server)
        .await;
    let mut tasks = vec![];
    for n in 0..if all { 4 } else { 2 } {
        let client = client.with_read_context(ReadContext::new(
            if n < 2 {
                ReadClass::Background
            } else {
                ReadClass::Foreground
            },
            Duration::from_secs(30),
        ));
        tasks.push(tokio::spawn(async move {
            let _ = client.stats_graphql(&json!({"query":"blocker"})).await;
        }));
    }
    requests(server, tasks.len()).await;
    tasks
}
async fn stop(tasks: Vec<tokio::task::JoinHandle<()>>) {
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        let _ = task.await;
    }
}
fn scan(
    client: &GitHubClient,
    class: ReadClass,
    budget: Duration,
    mode: ScanMode,
    state: State,
) -> tokio::task::JoinHandle<Result<FetchedList, ClientError>> {
    let client = client.with_read_context(ReadContext::new(class, budget));
    tokio::spawn(async move {
        client
            .advance_scan_mode(
                CachedList::Authored,
                Loaded { revision: 0, state },
                &[],
                1000,
                mode,
            )
            .await
    })
}
fn completed_state() -> State {
    State {
        done: true,
        finished_at: Some(0),
        completed_at: Some(7),
        candidates: [super::tests::candidate(1)].into(),
        ..State::default()
    }
}
#[tokio::test]
async fn shorter_or_cancelled_leader_cannot_end_a_live_queued_joiner() {
    for cancel in [false, true] {
        let (server, client) = fixture().await;
        let mut held = blockers(&server, &client, true).await;
        let leader = scan(
            &client,
            ReadClass::Background,
            Duration::from_millis(150),
            ScanMode::Continue,
            State::default(),
        );
        callers(&client, 1).await;
        let joiner = scan(
            &client,
            ReadClass::Foreground,
            Duration::from_secs(3),
            ScanMode::Refresh,
            State::default(),
        );
        callers(&client, 2).await;
        if cancel {
            leader.abort();
            assert!(matches!(leader.await, Err(error) if error.is_cancelled()));
        } else {
            assert!(leader.await.unwrap().is_err());
        }
        callers(&client, 1).await;
        stop(held.drain(2..).collect()).await;
        let receipt = tokio::time::timeout(Duration::from_secs(1), joiner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(receipt.scan.unwrap().state.done);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            5,
            "one page after four real blockers"
        );
        stop(held).await;
    }
}
#[tokio::test]
async fn last_caller_cancellation_releases_queued_work_and_same_revision_can_restart() {
    let (server, client) = fixture().await;
    let held = blockers(&server, &client, false).await;
    let bounded = client.with_attempt_limit(3);
    let leader = scan(
        &bounded,
        ReadClass::Background,
        Duration::from_secs(3),
        ScanMode::Continue,
        State::default(),
    );
    callers(&client, 1).await;
    assert_eq!(bounded.attempts_remaining(), 3, "no debit while queued");
    leader.abort();
    let _ = leader.await;
    stop(held).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "no orphan dispatch"
    );
    scan(
        &client,
        ReadClass::Foreground,
        Duration::from_secs(3),
        ScanMode::Refresh,
        State::default(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
#[tokio::test]
async fn queued_refresh_promotes_mode_but_dispatched_continue_keeps_its_receipt() {
    for queued in [true, false] {
        let (server, client) = fixture().await;
        let held = if queued {
            blockers(&server, &client, false).await
        } else {
            vec![]
        };
        let leader = scan(
            &client,
            ReadClass::Background,
            Duration::from_secs(3),
            ScanMode::Continue,
            completed_state(),
        );
        callers(&client, 1).await;
        if !queued {
            requests(&server, 1).await;
        }
        let joiner = scan(
            &client,
            ReadClass::Foreground,
            Duration::from_secs(3),
            ScanMode::Refresh,
            completed_state(),
        );
        let first = leader.await.unwrap().unwrap();
        let second = joiner.await.unwrap().unwrap();
        assert_eq!(first.scan, second.scan);
        assert_eq!(
            first.scan.unwrap().state.completed_at,
            Some(if queued { 1000 } else { 7 })
        );
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            if queued { 4 } else { 1 }
        );
        stop(held).await;
    }
}
#[tokio::test]
async fn cancelled_foreground_joiner_demotes_queued_class_and_refresh_intent() {
    let (server, client) = fixture().await;
    let mut held = blockers(&server, &client, true).await;
    let leader = scan(
        &client,
        ReadClass::Background,
        Duration::from_secs(3),
        ScanMode::Continue,
        completed_state(),
    );
    callers(&client, 1).await;
    let joiner = scan(
        &client,
        ReadClass::Foreground,
        Duration::from_secs(3),
        ScanMode::Refresh,
        completed_state(),
    );
    callers(&client, 2).await;
    joiner.abort();
    let _ = joiner.await;
    callers(&client, 1).await;
    stop(held.drain(2..).collect()).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        4,
        "cancelled foreground demand cannot take reserved capacity"
    );
    stop(held).await;
    let receipt = leader.await.unwrap().unwrap();
    assert_eq!(receipt.scan.unwrap().state.completed_at, Some(7));
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        5,
        "cancelled Refresh cannot restart discovery"
    );
}

#[tokio::test]
async fn late_joiner_keeps_zero_dispatch_opportunity_beyond_first_callers_thirty_seconds() {
    let (server, client) = fixture().await;
    let held = blockers(&server, &client, true).await;
    let leader = scan(
        &client,
        ReadClass::Background,
        Duration::from_secs(30),
        ScanMode::Continue,
        State::default(),
    );
    callers(&client, 1).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(29)).await;
    let joiner = scan(
        &client,
        ReadClass::Foreground,
        Duration::from_secs(30),
        ScanMode::Refresh,
        State::default(),
    );
    callers(&client, 2).await;
    tokio::time::advance(Duration::from_secs(2)).await;
    // The original caller and four bounded HTTP blockers have expired. The
    // late caller still owns 28 seconds; no zero-dispatch receipt may end it.
    tokio::time::resume();
    assert!(leader.await.unwrap().is_err());
    stop(held).await;
    let result = tokio::time::timeout(Duration::from_secs(1), joiner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.scan.unwrap().state.done);
    assert_eq!(server.received_requests().await.unwrap().len(), 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retirement_racing_two_registrations_keeps_one_live_producer() {
    for _ in 0..8 {
        let (server, client) = fixture().await;
        let mut held = blockers(&server, &client, true).await;
        let leader = scan(
            &client,
            ReadClass::Background,
            Duration::from_secs(3),
            ScanMode::Continue,
            State::default(),
        );
        callers(&client, 1).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let retire_barrier = barrier.clone();
        let retirement = tokio::spawn(async move {
            retire_barrier.wait().await;
            leader.abort();
            let _ = leader.await;
        });
        let mut joined = vec![];
        for _ in 0..2 {
            let barrier = barrier.clone();
            let client = client.clone();
            joined.push(tokio::spawn(async move {
                barrier.wait().await;
                client
                    .advance_scan(
                        CachedList::Authored,
                        Loaded {
                            revision: 0,
                            state: State::default(),
                        },
                        &[],
                        1000,
                    )
                    .await
            }));
        }
        retirement.await.unwrap();
        callers(&client, 2).await;
        stop(held.drain(2..).collect()).await;
        let a = joined.remove(0).await.unwrap().unwrap();
        let b = joined.remove(0).await.unwrap().unwrap();
        assert_eq!(a.scan, b.scan);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            5,
            "retirement cannot create duplicate live producers"
        );
        stop(held).await;
    }
}

#[tokio::test]
async fn shared_execution_deadline_preserves_pages_received_before_later_timeout() {
    let (server, client) = fixture().await;
    server.reset().await;
    Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
        let body: Value = request.body_json().unwrap();
        if !body["variables"]["after"].is_null() {
            return ResponseTemplate::new(200).set_delay(Duration::from_secs(120)).set_body_json(json!({"data":{}}));
        }
        let raw: Value = serde_json::from_str(include_str!("../../../tests/fixtures/search.json")).unwrap();
        let nodes: Vec<_> = (1..=25).map(|n| {
            let mut node = raw["authored"]["nodes"][0].clone();
            node["id"] = json!(format!("PR-{n}")); node["number"] = json!(n); node
        }).collect();
        ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":50,"nodes":nodes,"pageInfo":{"hasNextPage":true,"endCursor":"next-page"}}}}))
    }).mount(&server).await;
    let worker = scan(
        &client,
        ReadClass::Foreground,
        Duration::from_secs(60),
        ScanMode::Refresh,
        State::default(),
    );
    requests(&server, 2).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::time::resume();
    let result = worker
        .await
        .unwrap()
        .expect("a later timed-out page cannot discard the shared operation's received first page");
    assert_eq!(result.prs.len(), 25);
    assert_eq!(
        result.scan.unwrap().state.after.as_deref(),
        Some("next-page")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn last_dispatched_caller_cancellation_releases_all_four_http_slots() {
    let (server, client) = fixture().await;
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_json(json!({"data":{}})),
        )
        .mount(&server)
        .await;
    let leader = scan(
        &client,
        ReadClass::Foreground,
        Duration::from_secs(3),
        ScanMode::Refresh,
        State::default(),
    );
    requests(&server, 1).await;
    leader.abort();
    let _ = leader.await;
    let mut work = vec![];
    for _ in 0..4 {
        let client = client.clone();
        work.push(tokio::spawn(async move {
            let _ = client
                .stats_graphql(&json!({"query":"capacity-after-cancellation"}))
                .await;
        }));
    }
    requests(&server, 5).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        5,
        "cancelled shared HTTP cannot keep a permit"
    );
    stop(work).await;
    server.reset().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":{"viewer":{"login":"fixture"},"authored":{"issueCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}}}))).mount(&server).await;
    let next = scan(
        &client,
        ReadClass::Foreground,
        Duration::from_secs(3),
        ScanMode::Refresh,
        State::default(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(next.scan.unwrap().state.done);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "retired dispatched operation cannot supply a receipt"
    );
}
