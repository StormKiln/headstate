use super::*;
use crate::github::client::GitHubClient;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct Provider {
    client: GitHubClient,
    calls: Arc<AtomicUsize>,
    partial: Arc<AtomicBool>,
    only: Arc<AtomicUsize>,
    remaining: Arc<AtomicUsize>,
    blocked: Arc<AtomicBool>,
    tail: Arc<AtomicBool>,
    hang_index: Arc<AtomicUsize>,
    arrived: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn provider(viewer: &str, value: u64) -> Provider {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let partial = Arc::new(AtomicBool::new(false));
    let only = Arc::new(AtomicUsize::new(0));
    let remaining = Arc::new(AtomicUsize::new(5000));
    let blocked = Arc::new(AtomicBool::new(false));
    let tail = Arc::new(AtomicBool::new(false));
    let hang_index = Arc::new(AtomicUsize::new(100));
    let arrived = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (c, p, b, a, r, v, t, o, left, hang) = (
        calls.clone(),
        partial.clone(),
        blocked.clone(),
        arrived.clone(),
        release.clone(),
        viewer.to_string(),
        tail.clone(),
        only.clone(),
        remaining.clone(),
        hang_index.clone(),
    );
    let app = axum::Router::new().route("/graphql", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let (c,p,b,a,r,v,t,o,left,hang) = (c.clone(),p.clone(),b.clone(),a.clone(),r.clone(),v.clone(),t.clone(),o.clone(),left.clone(),hang.clone());
        async move {
            c.fetch_add(1,Ordering::SeqCst);
            let query = body["query"].as_str().unwrap_or_default();
            if !query.contains("viewer {") && (query.contains(&format!("m{}:",hang.load(Ordering::SeqCst))) || query.contains(&format!("v{}:",hang.load(Ordering::SeqCst))) || b.load(Ordering::SeqCst) || (t.load(Ordering::SeqCst) && (query.contains("m40:") || query.contains("v40:")))) { a.notify_one(); r.notified().await; }
            let mut data = json!({"viewer":{"login":v}});
            let yesterday = (chrono::Utc::now()-chrono::Duration::days(1)).format("%Y-%m-%dT12:00:00Z").to_string();
            for i in 0..100 {
                data[format!("s{i}")] = json!({"issueCount":if p.load(Ordering::SeqCst) {1500} else {value},"nodes":[{"number":value,"title":"synthetic","url":"https://example.test/pr","repository":{"nameWithOwner":"fixture/repo"},"author":{"login":v},"createdAt":"2026-01-01T00:00:00Z","mergedAt":yesterday,"additions":value,"deletions":1,"changedFiles":1,"reviews":{"totalCount":1}}]});
                if !p.load(Ordering::SeqCst) || i == o.load(Ordering::SeqCst) {
                    data[format!("m{i}")] = json!({"issueCount":value});
                    data[format!("o{i}")] = json!({"issueCount":value+1});
                    data[format!("v{i}")] = json!({"issueCount":value});
                }
            }
            data["rateLimit"] = json!({"cost":1,"remaining":left.load(Ordering::SeqCst),"resetAt":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()});
            let mut response=json!({"data":data});
            if p.load(Ordering::SeqCst) && query.contains("s0:") {response["errors"]=json!([{"message":"synthetic field refusal"}]);}
            axum::Json(response)
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = GitHubClient::new(
        octocrab::Octocrab::builder()
            .base_uri(url)
            .unwrap()
            .personal_token("synthetic")
            .build()
            .unwrap(),
    );
    Provider {
        client,
        calls,
        partial,
        only,
        remaining,
        blocked,
        tail,
        hang_index,
        arrived,
        release,
        server,
    }
}
async fn series(p: &Provider, db: std::path::PathBuf) -> crate::github::stats::Series {
    stats_series_for_client(
        &p.client,
        db,
        None,
        "org".into(),
        Some("fixture-org".into()),
        7,
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn series_partial_retry_reaches_provider() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    p.partial.store(true, Ordering::SeqCst);
    let first = series(&p, db.clone()).await;
    assert_eq!(first.points.len(), 1);
    assert_eq!(first.failed_days.len(), 6);
    p.partial.store(false, Ordering::SeqCst);
    let second = series(&p, db).await;
    assert_eq!(
        second.points.len(),
        7,
        "Retry must measure the six previously missing days"
    );
    assert!(second.is_complete());
}
#[tokio::test]
async fn board_owner_aba_rejects_old_generation_with_nonempty_data() {
    let a = provider("alice", 11).await;
    let b = provider("bob", 22).await;
    let anew = provider("alice", 33).await;
    a.client
        .stats_viewer_metered(&a.client.request_budget())
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    a.blocked.store(true, Ordering::SeqCst);
    let old_client = a.client.clone();
    let old_db = db.clone();
    let old = tokio::spawn(async move {
        stats_board_for_client(
            &old_client,
            old_db,
            &tokio::sync::Notify::new(),
            "org".into(),
            Some("fixture-org".into()),
            "merged".into(),
            7,
        )
        .await
        .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), a.arrived.notified())
        .await
        .unwrap();
    let load = |client: GitHubClient, db| async move {
        stats_board_for_client(
            &client,
            db,
            &tokio::sync::Notify::new(),
            "org".into(),
            Some("fixture-org".into()),
            "merged".into(),
            7,
        )
        .await
        .unwrap()
    };
    let br = load(b.client.clone(), db.clone()).await;
    assert_eq!(br.viewer, "bob");
    assert_eq!(br.owner.as_ref().unwrap().generation(), 2);
    assert!(
        matches!(&br.backfill, BackfillRegistration::Registered(registered) if Some(&registered.owner) == br.owner.as_ref())
    );
    assert!(!br.board.rows.is_empty());
    let ar = load(anew.client.clone(), db.clone()).await;
    assert_eq!(ar.viewer, "alice");
    assert!(!ar.board.rows.is_empty());
    let conn = open_db(&db).unwrap();
    let current = stats_owner::current_for_verified(&conn, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(current.generation(), 3);
    a.blocked.store(false, Ordering::SeqCst);
    a.release.notify_waiters();
    let old = old.await.unwrap();
    assert_eq!(old.viewer, "alice");
    assert_eq!(
        serde_json::to_value(&old).unwrap()["owner"]["generation"],
        1,
        "the pure board reply must carry its captured generation"
    );
    assert!(!old.board.accumulating);
    assert!(
        matches!(old.backfill, BackfillRegistration::Failed(_)),
        "a superseded original fetch must not claim a current registration"
    );
    assert_eq!(
        stats_owner::current_for_verified(&conn, "alice")
            .unwrap()
            .unwrap(),
        current
    );
    let again = load(anew.client.clone(), db.clone()).await;
    assert_eq!(again.owner.as_ref().unwrap().generation(), 3);
    assert_eq!(again.owner, ar.owner);
    assert_eq!(
        serde_json::to_value(again.board).unwrap(),
        serde_json::to_value(ar.board).unwrap()
    );
}

#[tokio::test]
async fn series_overall_deadline_keeps_completed_wave() {
    let p = provider("alice", 17).await;
    p.tail.store(true, Ordering::SeqCst);
    let client = p.client.clone();
    let days: Vec<String> = (1..=50)
        .map(|n| {
            (chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(n))
                .to_string()
        })
        .collect();
    let requested = days.clone();
    let task = tokio::spawn(async move {
        let q = crate::github::stats::StatsQuery::new(
            None,
            crate::github::stats::Scope::Org("fixture-org".into()),
            crate::github::stats::Measure::Merged,
        );
        crate::github::stats::load_series(&client, &q, &days, &client.request_budget()).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), p.arrived.notified())
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    let result = task
        .await
        .unwrap()
        .expect("completed first wave must survive overall expiry");
    tokio::time::resume();
    assert_eq!(result.points.len(), 40);
    assert_eq!(result.failed_days, requested[40..]);
    assert!(
        result.unmeasured.is_some(),
        "expiry must carry a timeout qualification"
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn warm_complete_series_has_no_http_or_replayed_spend() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let first = series(&p, db.clone()).await;
    let calls = p.calls.load(Ordering::SeqCst);
    p.server.abort();
    let second = series(&p, db).await;
    assert_eq!(p.calls.load(Ordering::SeqCst), calls);
    assert_eq!(second.points, first.points);
    assert_eq!(second.spend.requests, 0);
    assert_eq!(
        second.receipt.unwrap().fetched_at,
        first.receipt.unwrap().fetched_at
    );
}

#[tokio::test]
async fn answer_writes_reclaim_unreachable_daily_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let owner = capture_stats_owner(db.clone(), "alice".into())
        .await
        .unwrap();
    for day in 1..=8 {
        let now = chrono::DateTime::parse_from_rfc3339(&format!("2026-09-{day:02}T12:00:00Z"))
            .unwrap()
            .with_timezone(&chrono::Utc);
        let end = (now - chrono::Duration::days(1)).date_naive().to_string();
        stats_cache_put(
            db.clone(),
            owner.clone(),
            "count|merged|alice|org:fixture".into(),
            end.clone(),
            end,
            9,
            day % 2 == 0,
            "{}".into(),
            now,
            now,
        )
        .await
        .unwrap();
    }
    let conn = open_db(&db).unwrap();
    assert_eq!(
        crate::store::stats::count(&conn).unwrap(),
        3,
        "retain only three reachable UTC ends"
    );
}

#[tokio::test]
async fn concurrent_equivalent_reviewers_share_five_documents() {
    let p = provider("alice", 17).await;
    p.client
        .stats_viewer_metered(&p.client.request_budget())
        .await
        .unwrap();
    p.calls.store(0, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let logins: Vec<String> = (0..50).map(|n| format!("person{n:02}")).collect();
    let mut reverse = logins.clone();
    reverse.reverse();
    let (a, b) = tokio::join!(
        stats_reviewers_for_client(
            &p.client,
            db.clone(),
            "org".into(),
            Some("fixture-org".into()),
            logins,
            7,
            false
        ),
        stats_reviewers_for_client(
            &p.client,
            db.clone(),
            "org".into(),
            Some("fixture-org".into()),
            reverse,
            7,
            false
        )
    );
    assert_eq!(a.unwrap().rows, b.unwrap().rows);
    assert_eq!(
        p.calls.load(Ordering::SeqCst),
        5,
        "equivalent callers must share five reviewer documents"
    );
}

async fn count(p: &Provider, db: std::path::PathBuf) -> crate::github::stats::Outcome {
    stats_count_for_client(
        &p.client,
        db,
        Some("named-subject".into()),
        "org".into(),
        Some("fixture-org".into()),
        "merged".into(),
        7,
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn warm_count_reuses_verified_client_and_original_age() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let first = count(&p, db.clone()).await;
    assert_eq!(first.total, 17);
    let calls = p.calls.load(Ordering::SeqCst);
    p.server.abort();
    let second = count(&p, db).await;
    assert_eq!(second.total, 17);
    assert_eq!(second.spend.requests, 0);
    assert_eq!(
        second.receipt.unwrap().fetched_at,
        first.receipt.unwrap().fetched_at
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), calls);
}
#[tokio::test]
async fn count_and_series_late_owner_never_overwrite_current_receipts() {
    for is_series in [false, true] {
        let a = provider("alice", 11).await;
        let b = provider("bob", 22).await;
        a.client
            .stats_viewer_metered(&a.client.request_budget())
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("stats.db");
        a.blocked.store(true, Ordering::SeqCst);
        let client = a.client.clone();
        let path = db.clone();
        let old = tokio::spawn(async move {
            if is_series {
                let r = stats_series_for_client(
                    &client,
                    path,
                    Some("named-subject".into()),
                    "org".into(),
                    Some("fixture-org".into()),
                    7,
                )
                .await
                .unwrap();
                (r.points[0].merged, r.receipt.unwrap())
            } else {
                let r = stats_count_for_client(
                    &client,
                    path,
                    Some("named-subject".into()),
                    "org".into(),
                    Some("fixture-org".into()),
                    "merged".into(),
                    7,
                )
                .await
                .unwrap();
                (r.total, r.receipt.unwrap())
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), a.arrived.notified())
            .await
            .unwrap();
        let load = || async {
            if is_series {
                let r = stats_series_for_client(
                    &b.client,
                    db.clone(),
                    Some("named-subject".into()),
                    "org".into(),
                    Some("fixture-org".into()),
                    7,
                )
                .await
                .unwrap();
                (r.points[0].merged, r.receipt.unwrap())
            } else {
                let r = count(&b, db.clone()).await;
                (r.total, r.receipt.unwrap())
            }
        };
        let first = load().await;
        assert_eq!(first.0, 22);
        a.blocked.store(false, Ordering::SeqCst);
        a.release.notify_waiters();
        let old = old.await.unwrap();
        assert_eq!(old.0, 11);
        assert!(old.1.qualification.is_some());
        let calls = b.calls.load(Ordering::SeqCst);
        let again = load().await;
        assert_eq!(again.0, 22);
        assert_eq!(again.1.fetched_at, first.1.fetched_at);
        assert_eq!(b.calls.load(Ordering::SeqCst), calls);
    }
}
#[tokio::test]
async fn canceled_reviewer_producer_does_not_cancel_another_waiter() {
    let p = provider("alice", 17).await;
    p.client
        .stats_viewer_metered(&p.client.request_budget())
        .await
        .unwrap();
    p.calls.store(0, Ordering::SeqCst);
    p.blocked.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let load = |client: GitHubClient, path| async move {
        stats_reviewers_for_client(
            &client,
            path,
            "org".into(),
            Some("fixture-org".into()),
            (0..50).map(|n| format!("person{n:02}")).collect(),
            7,
            false,
        )
        .await
    };
    let first = tokio::spawn(load(p.client.clone(), db.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(5), p.arrived.notified())
        .await
        .unwrap();
    let second = tokio::spawn(load(p.client.clone(), db));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while crate::github::stats::reviewer_receipts::test_support::live(
            &p.client.reviewer_receipts,
        )
        .1 < 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.abort();
    first.await.unwrap_err();
    p.blocked.store(false, Ordering::SeqCst);
    p.release.notify_waiters();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.rows.len(), 50);
    assert_eq!(p.calls.load(Ordering::SeqCst), 5);
}
#[tokio::test]
async fn reviewer_complete_receipt_refresh_and_expiry_are_bounded() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let load = |refresh| {
        stats_reviewers_for_client(
            &p.client,
            db.clone(),
            "org".into(),
            Some("fixture-org".into()),
            vec![" Alice ".into(), "alice".into(), "BOB".into()],
            7,
            refresh,
        )
    };
    let first = load(false).await.unwrap();
    assert_eq!(first.rows.len(), 2);
    let calls = p.calls.load(Ordering::SeqCst);
    let cached = load(false).await.unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), calls);
    assert_eq!(cached.spend.requests, 0);
    assert_eq!(
        cached.receipt.unwrap().fetched_at,
        first.receipt.unwrap().fetched_at
    );
    load(true).await.unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), calls + 1);
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(301)).await;
    tokio::time::resume();
    load(false).await.unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), calls + 2);
}

#[tokio::test]
async fn same_owner_legacy_profile_preserves_history_when_initialized_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    let mut conn = open_db(&db).unwrap();
    crate::store::settings::set(&conn, crate::store::settings::keys::STATS_VIEWER, &"alice")
        .unwrap();
    crate::store::pr_slice::put(
        &conn,
        "legacy",
        &crate::store::pr_slice::SliceRow {
            from: "2026-09-01".into(),
            to: "2026-09-01".into(),
            state: crate::store::pr_slice::SliceState::Complete,
            issue_count: 0,
            retrieved: 0,
            refused_fields: 0,
        },
        chrono::Utc::now(),
    )
    .unwrap();
    let legacy = crate::store::pr_history::StoredPr {
        repo: "fixture/legacy".into(),
        number: 41,
        merged_at: "2026-09-01".into(),
        title: "preserved legacy row".into(),
        url: "https://example.test/41".into(),
        author: "alice".into(),
        cycle_time_hours: 3.0,
        size: 9,
        additions: 7,
        deletions: 2,
        changed_files: 1,
        reviews_received: 2,
    };
    crate::store::pr_history::put_many(
        &mut conn,
        "legacy",
        "2026-09-01",
        "2026-09-01",
        std::slice::from_ref(&legacy),
        chrono::Utc::now(),
    )
    .unwrap();
    assert!(stats_owner::current_for_verified(&conn, "alice")
        .unwrap()
        .is_none());
    assert!(
        crate::store::settings::get::<i64>(&conn, crate::store::settings::keys::STATS_GENERATION)
            .unwrap()
            .is_none(),
        "background lookup must not initialize ownership"
    );
    let p = provider("alice", 17).await;
    series(&p, db.clone()).await;
    let owner = stats_owner::current_for_verified(&conn, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(owner.generation(), 1);
    assert_eq!(
        crate::store::pr_history::load(&conn, "legacy", "2026-09-01", "2026-09-01").unwrap(),
        vec![legacy]
    );
    assert_eq!(crate::store::pr_slice::total_rows(&conn).unwrap(), 1);
    series(&p, db).await;
    assert_eq!(
        stats_owner::current_for_verified(&conn, "alice").unwrap(),
        Some(owner)
    );
}
#[tokio::test]
async fn warm_series_read_succeeds_while_unrelated_wal_writer_is_held() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    series(&p, db.clone()).await;
    let mut writer = open_db(&db).unwrap();
    let tx = writer
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    crate::store::settings::set(&tx, "unrelated-fixture", &7).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_millis(500), series(&p, db))
        .await
        .unwrap();
    assert_eq!(result.points.len(), 7);
    assert_eq!(result.spend.requests, 0);
}
#[tokio::test]
async fn stale_background_page_and_frame_cannot_cross_owner_aba() {
    let a = provider("alice", 11).await;
    let b = provider("bob", 22).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    series(&a, db.clone()).await;
    let mut conn = open_db(&db).unwrap();
    let old = stats_owner::current_for_verified(&conn, "alice")
        .unwrap()
        .unwrap();
    let pages = {
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let pages =
            crate::store::pr_backfill_page::select_in(&tx, &old, "key", &["2026-09-01".into()], 5)
                .unwrap();
        tx.commit().unwrap();
        pages
    };
    series(&b, db.clone()).await;
    series(&a, db).await;
    let current = stats_owner::current_for_verified(&conn, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(current.generation(), 3);
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(crate::store::pr_backfill_page::commit_in(
        &tx,
        &old,
        "key",
        &pages,
        &json!({}),
        chrono::Utc::now()
    )
    .is_err());
    assert_eq!(crate::store::pr_slice::total_rows(&tx).unwrap(), 0);
    tx.commit().unwrap();
    remember_backfill_frame(&StatsBackfillFrame {
        owner: old,
        scope_key: "key".into(),
        days_covered: 9,
        days_total: 30,
        collected: 111,
        total: Some(111),
        phase: crate::github::stats::backfill::BackfillPhase::Working,
        next_tick_at_ms: None,
    });
    let BackfillRegistration::Registered(registration) =
        backfill_registration(Ok(()), &current, "key")
    else {
        panic!()
    };
    assert!(registration.last_frame.is_none());
}

#[tokio::test]
async fn reviewer_receipts_evict_the_oldest_of_more_than_64_keys() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let load = |n| {
        stats_reviewers_for_client(
            &p.client,
            db.clone(),
            "org".into(),
            Some(format!("fixture{n}")),
            vec!["person".into()],
            7,
            false,
        )
    };
    for n in 0..65 {
        load(n).await.unwrap();
    }
    let calls = p.calls.load(Ordering::SeqCst);
    load(64).await.unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), calls);
    load(0).await.unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), calls + 1);
}
#[tokio::test]
async fn reviewer_active_key_capacity_refuses_without_detached_work() {
    let p = provider("alice", 17).await;
    p.client
        .stats_viewer_metered(&p.client.request_budget())
        .await
        .unwrap();
    p.blocked.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let mut tasks = Vec::new();
    for n in 0..16 {
        let client = p.client.clone();
        let path = db.clone();
        tasks.push(tokio::spawn(async move {
            stats_reviewers_for_client(
                &client,
                path,
                "org".into(),
                Some(format!("fixture{n}")),
                vec!["person".into()],
                7,
                false,
            )
            .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while crate::github::stats::reviewer_receipts::test_support::live(
            &p.client.reviewer_receipts,
        )
        .0 < 16
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let refused = stats_reviewers_for_client(
        &p.client,
        db.clone(),
        "org".into(),
        Some("seventeenth".into()),
        vec!["person".into()],
        7,
        false,
    )
    .await
    .unwrap_err();
    assert!(refused.contains("still running"));
    for task in tasks {
        task.abort();
        task.await.unwrap_err();
    }
    assert_eq!(
        crate::github::stats::reviewer_receipts::test_support::live(&p.client.reviewer_receipts).0,
        0
    );
    p.blocked.store(false, Ordering::SeqCst);
    p.release.notify_waiters();
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stats_reviewers_for_client(
            &p.client,
            db,
            "org".into(),
            Some("after-cancellation".into()),
            vec!["person".into()],
            7,
            false,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(answer.rows.len(), 1);
}
#[tokio::test]
async fn reviewer_timeout_retains_completed_wave_in_command_reply() {
    let p = provider("alice", 17).await;
    p.tail.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let client = p.client.clone();
    let task = tokio::spawn(async move {
        stats_reviewers_for_client(
            &client,
            db,
            "org".into(),
            Some("fixture-org".into()),
            (0..50).map(|n| format!("person{n:02}")).collect(),
            7,
            false,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), p.arrived.notified())
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    let result = task.await.unwrap().unwrap();
    tokio::time::resume();
    assert_eq!(result.rows.len(), 40);
    assert_eq!(result.unmeasured.len(), 10);
    assert!(matches!(
        result.stop_reason,
        Some(crate::github::stats::fetch::Unmeasured::Timeout)
    ));
}
#[tokio::test]
async fn derived_answer_cardinality_does_not_evict_history_or_refresh_age() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let owner = capture_stats_owner(db.clone(), "alice".into())
        .await
        .unwrap();
    let now = chrono::Utc::now();
    let end = (now - chrono::Duration::days(1)).date_naive().to_string();
    let conn = open_db(&db).unwrap();
    crate::store::pr_slice::put(
        &conn,
        "retained-proof",
        &crate::store::pr_slice::SliceRow {
            from: end.clone(),
            to: end.clone(),
            state: crate::store::pr_slice::SliceState::Complete,
            issue_count: 0,
            retrieved: 0,
            refused_fields: 0,
        },
        now,
    )
    .unwrap();
    for n in 0..2050 {
        stats_cache_put(
            db.clone(),
            owner.clone(),
            format!("count|{n}"),
            end.clone(),
            end.clone(),
            17,
            n % 2 == 0,
            "{}".into(),
            now,
            now,
        )
        .await
        .unwrap();
    }
    assert_eq!(crate::store::stats::count(&conn).unwrap(), 2048);
    assert_eq!(crate::store::pr_slice::total_rows(&conn).unwrap(), 1);
    let retained = crate::store::stats::get(&conn, "count|2049", &end, &end, now)
        .unwrap()
        .unwrap();
    assert!(!retained.complete);
    assert_eq!(retained.fetched_at, now);
    assert!(crate::store::stats::get(&conn, "count|0", &end, &end, now)
        .unwrap()
        .is_none());
    let original_age = now - chrono::Duration::hours(1);
    stats_cache_put(
        db,
        owner,
        "active-retained".into(),
        end.clone(),
        end.clone(),
        17,
        false,
        "{}".into(),
        original_age,
        now,
    )
    .await
    .unwrap();
    let active = crate::store::stats::get(&conn, "active-retained", &end, &end, now)
        .unwrap()
        .expect("maintenance must not immediately evict the active useful receipt");
    assert_eq!(active.fetched_at, original_age);
    assert_eq!(crate::store::stats::count(&conn).unwrap(), 2048);
}

#[tokio::test]
async fn superseded_series_retry_discards_its_retained_original_generation_fallback() {
    for useful in [true, false] {
        let a = provider("alice", 11).await;
        let b = provider("bob", 22).await;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("stats.db");
        a.partial.store(true, Ordering::SeqCst);
        let first = series(&a, db.clone()).await;
        assert_eq!(first.points.len(), 1);
        a.only
            .store(if useful { 1 } else { usize::MAX }, Ordering::SeqCst);
        a.blocked.store(true, Ordering::SeqCst);
        let client = a.client.clone();
        let path = db.clone();
        let retry = tokio::spawn(async move {
            stats_series_for_client(
                &client,
                path,
                None,
                "org".into(),
                Some("fixture-org".into()),
                7,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), a.arrived.notified())
            .await
            .unwrap();
        let current = series(&b, db.clone()).await;
        assert_eq!(current.points.len(), 7);
        a.blocked.store(false, Ordering::SeqCst);
        a.release.notify_waiters();
        let result = retry.await.unwrap();
        if useful {
            let result = result.unwrap();
            assert_eq!(result.points.len(), 1);
            assert_ne!(result.points[0].date, first.points[0].date);
            let receipt = result.receipt.unwrap();
            assert!(!receipt.retained);
            assert!(receipt.qualification.is_some());
        } else {
            assert!(result.unwrap_err().contains("account changed"));
        }
        let again = series(&b, db).await;
        assert_eq!(again.points, current.points);
    }
}
#[tokio::test]
async fn count_partial_retry_and_warm_reserve_receipts_remain_live() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    p.partial.store(true, Ordering::SeqCst);
    let first = count(&p, db.clone()).await;
    assert!(!first.is_complete());
    assert_eq!(first.total, 10500);
    p.partial.store(false, Ordering::SeqCst);
    let second = count(&p, db.clone()).await;
    assert!(second.is_complete());
    series(&p, db.clone()).await;
    p.remaining.store(100, Ordering::SeqCst);
    p.client
        .fetch_viewer_metered(&p.client.request_budget())
        .await
        .unwrap();
    let calls = p.calls.load(Ordering::SeqCst);
    p.server.abort();
    assert_eq!(count(&p, db.clone()).await.total, 17);
    assert_eq!(series(&p, db).await.points.len(), 7);
    assert_eq!(p.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn reviewer_waiter_deadline_retains_its_useful_partial_receipt_age() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    p.partial.store(true, Ordering::SeqCst);
    let logins = vec!["alice".into(), "bob".into()];
    let first = stats_reviewers_for_client(
        &p.client,
        db.clone(),
        "org".into(),
        Some("fixture-org".into()),
        logins.clone(),
        7,
        false,
    )
    .await
    .unwrap();
    assert_eq!(first.rows.len(), 1);
    p.blocked.store(true, Ordering::SeqCst);
    let short = p
        .client
        .with_read_context(crate::github::admission::ReadContext::new(
            crate::github::admission::ReadClass::Background,
            std::time::Duration::from_millis(150),
        ));
    let second = stats_reviewers_for_client(
        &short,
        db,
        "org".into(),
        Some("fixture-org".into()),
        logins,
        7,
        false,
    )
    .await
    .expect("a caller timeout must retain useful saved reviewer measurements");
    assert_eq!(second.rows, first.rows);
    let receipt = second.receipt.unwrap();
    assert_eq!(receipt.fetched_at, first.receipt.unwrap().fetched_at);
    assert!(receipt.retained);
    assert!(receipt.qualification.is_some());
}

#[tokio::test]
async fn shorter_reviewer_waiter_expires_without_canceling_remaining_waiter() {
    let p = provider("alice", 17).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    p.partial.store(true, Ordering::SeqCst);
    let logins = vec!["alice".into(), "bob".into()];
    let first = stats_reviewers_for_client(
        &p.client,
        db.clone(),
        "org".into(),
        Some("fixture-org".into()),
        logins.clone(),
        7,
        false,
    )
    .await
    .unwrap();
    p.partial.store(false, Ordering::SeqCst);
    p.blocked.store(true, Ordering::SeqCst);
    let client = p.client.clone();
    let path = db.clone();
    let roster = logins.clone();
    let long = tokio::spawn(async move {
        stats_reviewers_for_client(
            &client,
            path,
            "org".into(),
            Some("fixture-org".into()),
            roster,
            7,
            false,
        )
        .await
        .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), p.arrived.notified())
        .await
        .unwrap();
    let calls = p.calls.load(Ordering::SeqCst);
    let short = p
        .client
        .with_read_context(crate::github::admission::ReadContext::new(
            crate::github::admission::ReadClass::Background,
            std::time::Duration::from_millis(150),
        ));
    let expired = stats_reviewers_for_client(
        &short,
        db,
        "org".into(),
        Some("fixture-org".into()),
        logins,
        7,
        false,
    )
    .await
    .unwrap();
    assert_eq!(expired.rows, first.rows);
    assert_eq!(
        expired.receipt.unwrap().fetched_at,
        first.receipt.unwrap().fetched_at
    );
    assert_eq!(
        expired.spend.requests, 0,
        "a joined waiter must not replay producer spend"
    );
    assert!(!long.is_finished());
    p.blocked.store(false, Ordering::SeqCst);
    p.release.notify_waiters();
    let completed = long.await.unwrap();
    assert!(completed.is_complete());
    assert_eq!(completed.rows.len(), 2);
    assert_eq!(completed.spend.requests, 1);
    assert_eq!(p.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn a_new_client_must_verify_before_reusing_disk_identity() {
    let a = provider("alice", 11).await;
    let anew = provider("alice", 33).await;
    let unavailable = provider("bob", 22).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    let first = series(&a, db.clone()).await;
    unavailable.server.abort();
    let failed = stats_series_for_client(
        &unavailable.client,
        db.clone(),
        None,
        "org".into(),
        Some("fixture-org".into()),
        7,
    )
    .await;
    assert!(
        failed.is_err(),
        "disk ownership must not authenticate a new client"
    );
    let reused = series(&anew, db.clone()).await;
    assert_eq!(reused.points, first.points);
    assert_eq!(
        anew.calls.load(Ordering::SeqCst),
        1,
        "a new same-viewer client verifies once before reuse"
    );
    assert_eq!(reused.receipt.unwrap().owner, first.receipt.unwrap().owner);
}

#[tokio::test]
async fn reviewers_are_isolated_between_immutable_clients_and_owners() {
    let a = provider("alice", 11).await;
    let same_viewer_other_host = provider("alice", 33).await;
    let b = provider("bob", 22).await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stats.db");
    for (p, expected, generation) in [
        (&a, 11, 1),
        (&same_viewer_other_host, 33, 1),
        (&b, 22, 2),
        (&a, 11, 3),
    ] {
        let result = stats_reviewers_for_client(
            &p.client,
            db.clone(),
            "org".into(),
            Some("fixture-org".into()),
            vec!["member".into()],
            7,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.rows[0].reviews, expected);
        assert_eq!(result.receipt.unwrap().owner.generation(), generation);
    }
}

#[tokio::test]
async fn deadlines_retain_in_wave_progress_and_distinguish_fully_unmeasured() {
    for reviewers in [false, true] {
        for completed in [0usize, 10] {
            let p = provider("alice", 17).await;
            p.hang_index.store(completed, Ordering::SeqCst);
            let client = p.client.clone();
            let budget = client.request_budget();
            let observed = budget.clone();
            let subjects: Vec<String> = if reviewers {
                (0..completed + 10)
                    .map(|i| format!("member{i:02}"))
                    .collect()
            } else {
                (0..completed + 10)
                    .map(|i| format!("2026-09-{:02}", i + 1))
                    .collect()
            };
            let requested = subjects.clone();
            let task = tokio::spawn(async move {
                let q = crate::github::stats::StatsQuery::new(
                    None,
                    crate::github::stats::Scope::Org("fixture-org".into()),
                    crate::github::stats::Measure::Merged,
                );
                if reviewers {
                    let window = crate::github::stats::Slice {
                        from: "2026-09-01".into(),
                        to: "2026-09-20".into(),
                    };
                    let result = crate::github::stats::load_reviewers(
                        &client, &q, &subjects, &window, &budget,
                    )
                    .await
                    .unwrap();
                    (result.rows.len(), result.unmeasured, result.stop_reason)
                } else {
                    let result = crate::github::stats::load_series(&client, &q, &subjects, &budget)
                        .await
                        .unwrap();
                    (result.points.len(), result.failed_days, result.unmeasured)
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), p.arrived.notified())
                .await
                .unwrap();
            if completed > 0 {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while observed.snapshot().points < 1 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(60)).await;
            let (measured, missing, reason) = task.await.unwrap();
            tokio::time::resume();
            assert_eq!(measured, completed);
            assert_eq!(missing, requested[completed..]);
            assert!(matches!(
                reason,
                Some(crate::github::stats::fetch::Unmeasured::Timeout)
            ));
            assert_eq!(
                p.calls.load(Ordering::SeqCst),
                if completed == 0 { 1 } else { 2 }
            );
        }
    }
}

#[tokio::test]
async fn overall_deadline_precedes_the_live_tail_document_deadline() {
    for reviewers in [false, true] {
        let p = provider("alice", 17).await;
        p.blocked.store(true, Ordering::SeqCst);
        let client = p.client.clone();
        let subjects: Vec<String> = if reviewers {
            (0..90).map(|i| format!("member{i:02}")).collect()
        } else {
            (0..90)
                .map(|i| {
                    (chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
                        + chrono::Duration::days(i))
                    .to_string()
                })
                .collect()
        };
        let requested = subjects.clone();
        let task = tokio::spawn(async move {
            let q = crate::github::stats::StatsQuery::new(
                None,
                crate::github::stats::Scope::Org("fixture-org".into()),
                crate::github::stats::Measure::Merged,
            );
            let budget = client.request_budget();
            if reviewers {
                let window = crate::github::stats::Slice {
                    from: "2026-01-01".into(),
                    to: "2026-03-31".into(),
                };
                let answer =
                    crate::github::stats::load_reviewers(&client, &q, &subjects, &window, &budget)
                        .await
                        .expect("overall deadline must retain earlier reviewer waves");
                (answer.rows.len(), answer.unmeasured, answer.stop_reason)
            } else {
                let answer = crate::github::stats::load_series(&client, &q, &subjects, &budget)
                    .await
                    .expect("overall deadline must retain earlier series waves");
                (answer.points.len(), answer.failed_days, answer.unmeasured)
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while p.calls.load(Ordering::SeqCst) < 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::pause();
        for next_wave_calls in [8, 9] {
            tokio::time::advance(std::time::Duration::from_secs(25)).await;
            assert!(!task.is_finished());
            p.release.notify_waiters();
            // Keep the paused runtime runnable while actual local HTTP is
            // exchanged. No automatic jump to a request timeout can stand in
            // for this barrier: the next wave proves every previous join ran.
            for _ in 0..100_000 {
                if p.calls.load(Ordering::SeqCst) >= next_wave_calls {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(p.calls.load(Ordering::SeqCst), next_wave_calls);
        }
        let tail_started = tokio::time::Instant::now();
        assert!(!task.is_finished());
        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        let (measured, missing, reason) = task.await.unwrap();
        assert!(
            tail_started.elapsed() < std::time::Duration::from_secs(30),
            "the tail document's transport timeout must not explain finalization"
        );
        tokio::time::resume();
        assert_eq!(measured, 80);
        assert_eq!(missing, requested[80..]);
        assert!(matches!(
            reason,
            Some(crate::github::stats::fetch::Unmeasured::Timeout)
        ));
        assert_eq!(p.calls.load(Ordering::SeqCst), 9);
    }
}
