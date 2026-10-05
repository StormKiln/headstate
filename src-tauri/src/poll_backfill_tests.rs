//! Synthetic worker regressions for #1626; no installed-account fixtures.
use super::backfill_tick;
use crate::github::{
    client::GitHubClient,
    stats::{backfill as bf, budget},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

fn run<F: std::future::Future<Output = ()>>(test: impl FnOnce() -> F) {
    budget::note_remaining(5000);
    tauri::async_runtime::block_on(async {
        let _permits = budget::READ_PERMIT_TEST_LOCK.lock().await;
        test().await;
    });
}
struct Rig {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    server: wiremock::MockServer,
    client: Arc<GitHubClient>,
    fault: Arc<Mutex<String>>,
    demand: Arc<crate::stats_demand::Registry>,
    from: String,
    to: String,
}
impl Rig {
    async fn new(total: usize, days: u32) -> Self {
        let server = wiremock::MockServer::start().await;
        let fault = Arc::new(Mutex::new(String::new()));
        let response_fault = fault.clone();
        wiremock::Mock::given(wiremock::matchers::method("POST")).respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let doc = body["query"].as_str().unwrap();
            let fault = response_fault.lock().unwrap().clone();
            let mut data = json!({"viewer":{"login":"fixture-viewer"}});
            for i in 0..5 {
                let Some((_,tail)) = doc.split_once(&format!("s{i}: search(")) else {continue};
                let day = &tail.split_once("merged:").unwrap().1[..10];
                let args = tail.split_once(")").unwrap().0;
                let after = args.split_once("after: ").and_then(|(_,s)|s.trim_matches('"').parse::<usize>().ok()).unwrap_or(0);
                if fault == "degrade" && args.contains("first: 50") { return wiremock::ResponseTemplate::new(502); }
                let page_size = if args.contains("first: 25") {25} else {50};
                let end = (after+page_size).min(total).min(1000);
                let base: u64 = day.replace('-',"").parse::<u64>().unwrap()*10000;
                let nodes:Vec<_> = (after..end).map(|n|json!({
                    "number":base+n as u64+1,"title":"fixture","url":"https://example.com/pr",
                    "repository":{"nameWithOwner":"fixture/repo"},"author":{"login":"fixture-author"},
                    "createdAt":format!("{day}T00:00:00Z"),"mergedAt":format!("{day}T01:00:00Z"),
                    "additions":1,"deletions":0,"changedFiles":1,"reviews":{"totalCount":0}
                })).collect();
                let mut page = json!({"issueCount":total,"nodes":nodes,"pageInfo":{"hasNextPage":end<total.min(1000),"endCursor":end.to_string()}});
                match fault.as_str() {
                    "missing-count" => {page.as_object_mut().unwrap().remove("issueCount");},
                    "missing-page-info" => {page.as_object_mut().unwrap().remove("pageInfo");},
                    "missing-alias" => continue,
                    "changed-count" => page["issueCount"] = json!(total+1),
                    "repeated-cursor" => page["pageInfo"]["endCursor"] = json!(after.to_string()),
                    "missing-cursor" => page["pageInfo"]["endCursor"] = Value::Null,
                    "cursor-absent" => {page["pageInfo"].as_object_mut().unwrap().remove("endCursor");},
                    "cursor-cycle" => page["pageInfo"]["endCursor"] = json!("50"),
                    "cursor-empty" => page["pageInfo"]["endCursor"] = json!(""),
                    "cursor-number" => page["pageInfo"]["endCursor"] = json!(120),
                    "duplicate" if after>0 => page["nodes"][0]["number"] = json!(base+1),
                    "short-terminal" => page["pageInfo"]["hasNextPage"] = json!(false),
                    "bad-node" => {page["nodes"][0].as_object_mut().unwrap().remove("number");},
                    _ => {},
                }
                data[format!("s{i}")] = page;
            }
            let mut response = json!({"data":data});
            if fault == "refused" {response["errors"] = json!([{"message":"synthetic refusal"}]);}
            let reply = wiremock::ResponseTemplate::new(200)
                .insert_header("x-ratelimit-remaining", if fault == "low-budget" { "1" } else { "5000" })
                .set_body_json(response);
            if fault == "delay" {reply.set_delay(std::time::Duration::from_millis(200))} else {reply}
        }).mount(&server).await;
        let client = Arc::new(GitHubClient::new(
            octocrab::Octocrab::builder()
                .base_uri(server.uri())
                .unwrap()
                .personal_token("fixture-token".to_string())
                .build()
                .unwrap(),
        ));
        client.fetch_viewer().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("worker.db");
        let conn = crate::store::open_db(&db).unwrap();
        crate::store::stats_owner::capture_verified(&conn, "fixture-viewer").unwrap();
        let (from, to) = bf::horizon_window(chrono::Utc::now(), days).unwrap();
        let rig = Self {
            _dir: dir,
            db,
            server,
            client,
            fault,
            demand: Arc::new(crate::stats_demand::Registry::default()),
            from,
            to,
        };
        rig.register("fixture", days);
        rig
    }
    fn register(&self, org: &str, days: u32) {
        let conn = crate::store::open_db(&self.db).unwrap();
        crate::store::pr_backfill_scope::note_seen(
            &conn,
            &crate::store::pr_backfill_scope::BackfillScope {
                scope_key: format!("merged|*|org:{org}"),
                scope_kind: "org".into(),
                scope_value: org.into(),
                measure: "merged".into(),
                horizon_days: days,
            },
            chrono::Utc::now(),
        )
        .unwrap();
        let tx =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)
                .unwrap();
        let owner = crate::store::stats_owner::current_for_verified(&tx, "fixture-viewer")
            .unwrap()
            .unwrap();
        self.demand
            .acquire(
                &tx,
                &owner,
                &crate::remote::context::DispatchContext::desktop(),
                crate::store::pr_backfill_scope::BackfillScope {
                    scope_key: format!("merged|*|org:{org}"),
                    scope_kind: "org".into(),
                    scope_value: org.into(),
                    measure: "merged".into(),
                    horizon_days: days,
                },
                std::time::Instant::now(),
                true,
            )
            .unwrap();
        tx.commit().unwrap();
    }
    fn fault(&self, f: &str) {
        *self.fault.lock().unwrap() = f.into();
    }
    async fn tick(&self) -> bf::TickOutcome {
        backfill_tick(self.db.clone(), &self.client, self.demand.clone())
            .await
            .outcome
    }
    fn count(&self) -> u64 {
        crate::store::pr_history::count(
            &crate::store::open_db(&self.db).unwrap(),
            "merged|*|org:fixture",
            &self.from,
            &self.to,
        )
        .unwrap()
    }
}

#[test]
fn dense_backfill_bad_pages_never_complete_and_recover() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        for fault in [
            "missing-count",
            "missing-page-info",
            "missing-alias",
            "missing-cursor",
            "bad-node",
            "refused",
            "short-terminal",
        ] {
            let r = Rig::new(120, 1).await;
            r.fault(fault);
            assert!(
                matches!(r.tick().await, bf::TickOutcome::Failed(_)),
                "{fault}"
            );
            let cov = crate::store::pr_slice::coverage(
                &crate::store::open_db(&r.db).unwrap(),
                "merged|*|org:fixture",
                &r.from,
                &r.to,
            )
            .unwrap();
            assert_eq!(cov.days_covered(), 0, "{fault}");
            r.fault("");
            for _ in 0..3 {
                r.tick().await;
            }
            assert!(
                matches!(r.tick().await, bf::TickOutcome::NoScope),
                "{fault}"
            );
            assert_eq!(r.count(), 120);
        }
    });
}
#[test]
fn dense_backfill_reordered_or_changed_pages_restart_without_false_counts() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        for fault in ["repeated-cursor", "duplicate", "changed-count"] {
            let r = Rig::new(120, 1).await;
            assert!(matches!(r.tick().await, bf::TickOutcome::Advanced { .. }));
            r.fault(fault);
            assert!(
                matches!(r.tick().await, bf::TickOutcome::Failed(_)),
                "{fault}"
            );
            r.fault("");
            for _ in 0..3 {
                r.tick().await;
            }
            assert!(
                matches!(r.tick().await, bf::TickOutcome::NoScope),
                "{fault}"
            );
            assert_eq!(r.count(), 120);
        }
    });
}
#[test]
fn dense_backfill_dates_and_scopes_get_turns() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let r = Rig::new(120, 6).await;
        r.tick().await;
        let conn = crate::store::open_db(&r.db).unwrap();
        assert_eq!(
            crate::store::pr_history::count(&conn, "merged|*|org:fixture", &r.to, &r.to).unwrap(),
            0
        );
        r.tick().await;
        assert_eq!(
            crate::store::pr_history::count(&conn, "merged|*|org:fixture", &r.to, &r.to).unwrap(),
            50
        );
        r.register("other", 1);
        r.tick().await;
        // A new scope joins behind already waiting demand instead of resetting
        // priority to zero. With two eligible scopes its bound is two turns.
        assert_eq!(
            crate::store::pr_history::count(&conn, "merged|*|org:other", &r.to, &r.to).unwrap(),
            0
        );
        r.tick().await;
        assert_eq!(
            crate::store::pr_history::count(&conn, "merged|*|org:other", &r.to, &r.to).unwrap(),
            50
        );
    });
}
#[test]
fn dense_backfill_search_cap_is_stalled_not_converged() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let r = Rig::new(1200, 1).await;
        for _ in 0..20 {
            assert!(!matches!(r.tick().await, bf::TickOutcome::Complete));
        }
        assert_eq!(r.count(), 1000);
        let requests = r.server.received_requests().await.unwrap().len();
        assert!(matches!(r.tick().await, bf::TickOutcome::NoScope));
        assert_eq!(r.server.received_requests().await.unwrap().len(), requests);
    });
}
#[test]
fn dense_backfill_account_switch_discards_inflight_page() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let r = Rig::new(120, 1).await;
        r.fault("delay");
        let db = r.db.clone();
        let client = r.client.clone();
        let demand = r.demand.clone();
        let task = tokio::spawn(async move { backfill_tick(db, &client, demand).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while r.server.received_requests().await.unwrap().len() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let conn = crate::store::open_db(&r.db).unwrap();
        crate::store::stats_owner::capture_verified(&conn, "other-viewer").unwrap();
        crate::store::pr_backfill_page::clear(&conn).unwrap();
        assert!(matches!(
            task.await.unwrap().outcome,
            bf::TickOutcome::Failed(_)
        ));
        assert_eq!(r.count(), 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM pr_backfill_page", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    });
}
#[test]
fn dense_backfill_budget_gate_does_not_issue_or_advance_pages() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let r = Rig::new(120, 1).await;
        r.fault("low-budget");
        // The ordinary viewer is already cached. Observe a real response via
        // the uncached metered read; headers affect only this client's quota.
        r.client
            .fetch_viewer_metered(&r.client.request_budget())
            .await
            .unwrap();
        assert_eq!(r.client.observed_remaining(), Some(1));
        let conn = crate::store::open_db(&r.db).unwrap();
        let cursor_rows = || {
            conn.query_row("SELECT COUNT(*) FROM pr_backfill_page", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        };
        assert_eq!(cursor_rows(), 0);
        let n = r.server.received_requests().await.unwrap().len();
        let outcome = r.tick().await;
        assert_eq!(
            r.server.received_requests().await.unwrap().len(),
            n,
            "budget refusal must issue no HTTP requests"
        );
        assert_eq!(r.count(), 0);
        assert_eq!(
            cursor_rows(),
            0,
            "budget refusal must not create cursor progress"
        );
        assert!(matches!(
            outcome,
            bf::TickOutcome::Skipped { remaining: Some(1) }
        ));
    });
}

#[test]
fn dense_backfill_degradation_keeps_its_cursor_and_measured_empty_is_complete() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let empty = Rig::new(0, 1).await;
        assert!(matches!(empty.tick().await, bf::TickOutcome::Complete));
        let r = Rig::new(120, 1).await;
        r.fault("degrade");
        for expected in [25, 50, 75, 100, 120] {
            let outcome = r.tick().await;
            assert_eq!(r.count(), expected);
            assert_eq!(
                matches!(outcome, bf::TickOutcome::Complete),
                expected == 120
            );
        }
    });
}
#[test]
fn dense_backfill_cancellation_keeps_the_last_committed_cursor() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let r = Rig::new(120, 1).await;
        r.tick().await;
        assert_eq!(r.count(), 50);
        r.fault("delay");
        let before = r.server.received_requests().await.unwrap().len();
        let db = r.db.clone();
        let client = r.client.clone();
        let demand = r.demand.clone();
        let task = tokio::spawn(async move { backfill_tick(db, &client, demand).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while r.server.received_requests().await.unwrap().len() == before {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(matches!(task.await,Err(e) if e.is_cancelled()));
        assert_eq!(r.count(), 50);
        r.fault("");
        r.tick().await;
        assert_eq!(r.count(), 100);
        assert!(matches!(r.tick().await, bf::TickOutcome::Complete));
    });
}

#[test]
fn dense_backfill_terminal_cursor_must_be_fresh_and_present() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        for fault in [
            "repeated-cursor",
            "missing-cursor",
            "cursor-absent",
            "cursor-cycle",
            "cursor-empty",
            "cursor-number",
        ] {
            let r = Rig::new(120, 1).await;
            for _ in 0..2 {
                assert!(matches!(r.tick().await, bf::TickOutcome::Advanced { .. }));
            }
            assert_eq!(r.count(), 100);
            r.fault(fault);
            assert!(
                matches!(r.tick().await, bf::TickOutcome::Failed(_)),
                "terminal {fault} must not settle coverage"
            );
            assert_eq!(r.count(), 120, "useful terminal rows survive {fault}");
            let conn = crate::store::open_db(&r.db).unwrap();
            let coverage =
                crate::store::pr_slice::coverage(&conn, "merged|*|org:fixture", &r.from, &r.to)
                    .unwrap();
            assert_eq!(coverage.days_covered(), 0, "terminal {fault}");
            r.fault("");
            // Inconsistency resets the pass, preserving rows. Only a new
            // sound pass can certify the terminal receipt.
            for _ in 0..2 {
                assert!(matches!(r.tick().await, bf::TickOutcome::Advanced { .. }));
            }
            assert!(matches!(r.tick().await, bf::TickOutcome::Complete));
            assert_eq!(r.count(), 120);
        }
        let empty = Rig::new(0, 1).await;
        empty.fault("missing-cursor"); // GraphQL legitimately returns null for an empty result.
        assert!(matches!(empty.tick().await, bf::TickOutcome::Complete));
    });
}

#[test]
fn evicted_150_row_day_refills_in_three_real_provider_pages_after_reopen() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let r = Rig::new(150, 1).await;
        for expected in [50, 100, 150] {
            r.tick().await;
            assert_eq!(r.count(), expected);
        }
        let conn = crate::store::open_db(&r.db).unwrap();
        conn.execute("UPDATE pr_history SET stored_at='1900-01-01'", [])
            .unwrap();
        conn.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<199851)
            INSERT INTO pr_history(scope_key,slice_from,slice_to,repo,number,merged_at,title,url,author,cycle_time_hours,size,additions,deletions,changed_files,reviews_received,stored_at)
            SELECT 'unrelated','2026-01-01','2026-01-31','fixture/padding',x,'2026-01-15','fixture','https://example.test/pr','fixture',1,1,1,0,1,0,'2026-01-01' FROM n;").unwrap();
        assert_eq!(crate::store::pr_history::total_rows(&conn).unwrap(), 200001);
        conn.execute_batch("CREATE TRIGGER refuse_eviction BEFORE UPDATE ON pr_scope_evidence BEGIN SELECT RAISE(ABORT,'synthetic rollback'); END;").unwrap();
        assert!(crate::store::pr_history::prune(&conn).is_err());
        assert_eq!(r.count(), 150);
        assert_eq!(crate::store::pr_history::total_rows(&conn).unwrap(), 200001);
        conn.execute_batch("DROP TRIGGER refuse_eviction;").unwrap();
        assert_eq!(crate::store::pr_history::prune(&conn).unwrap(), 20001);
        drop(conn);
        assert_eq!(r.count(), 0);
        let conn = crate::store::open_db(&r.db).unwrap();
        assert_eq!(
            crate::store::pr_slice::uncovered_days(&conn, "merged|*|org:fixture", &r.from, &r.to)
                .unwrap(),
            vec![r.from.clone()]
        );
        assert_eq!(
            crate::store::pr_history::count(&conn, "unrelated", "2026-01-01", "2026-01-31")
                .unwrap(),
            180000
        );
        let before = r.server.received_requests().await.unwrap().len();
        for expected in [50, 100, 150] {
            r.tick().await;
            assert_eq!(r.count(), expected);
        }
        assert_eq!(
            r.server.received_requests().await.unwrap().len(),
            before + 3
        );
        assert!(crate::store::pr_slice::uncovered_days(
            &conn,
            "merged|*|org:fixture",
            &r.from,
            &r.to
        )
        .unwrap()
        .is_empty());
    });
}

#[test]
fn a_page_dispatched_before_pruning_cannot_restore_coverage_after_reopen() {
    let _observed = budget::observed_test_lock();
    let _restore = budget::RestoreObserved::capture();
    run(|| async {
        let r = Rig::new(150, 1).await;
        r.tick().await;
        r.tick().await;
        assert_eq!(r.count(), 100);
        let key = "merged|*|org:fixture";
        let (owner, pages) = {
            let mut conn = crate::store::open_db(&r.db).unwrap();
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            let owner = crate::store::stats_owner::current_for_verified(&tx, "fixture-viewer")
                .unwrap()
                .unwrap();
            let pages = crate::store::pr_backfill_page::select_in(
                &tx,
                &owner,
                key,
                std::slice::from_ref(&r.from),
                1,
            )
            .unwrap();
            tx.commit().unwrap();
            (owner, pages)
        };
        let query = bf::query_for("org", "fixture", "merged").unwrap();
        let slices = bf::day_slices(std::slice::from_ref(&r.from));
        let fetched = crate::github::stats::fetch::load_backfill_pages(
            &r.client,
            &query,
            &slices,
            &[pages[0].after.clone()],
            &r.client.request_budget(),
        )
        .await
        .unwrap();
        {
            let conn = crate::store::open_db(&r.db).unwrap();
            conn.execute("UPDATE pr_history SET stored_at='1900-01-01'", [])
                .unwrap();
            conn.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<199901)
                INSERT INTO pr_history(scope_key,slice_from,slice_to,repo,number,merged_at,title,url,author,cycle_time_hours,size,additions,deletions,changed_files,reviews_received,stored_at)
                SELECT 'padding','2026-01-01','2026-01-31','fixture/padding',x,'2026-01-15','fixture','https://example.test/pr','fixture',1,1,1,0,1,0,'2026-01-01' FROM n;").unwrap();
            crate::store::pr_history::prune(&conn).unwrap();
        }
        let mut conn = crate::store::open_db(&r.db).unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        assert!(crate::store::pr_backfill_page::commit_in(
            &tx,
            &owner,
            key,
            &pages,
            &fetched,
            chrono::Utc::now()
        )
        .is_err());
        tx.rollback().unwrap();
        assert_eq!(r.count(), 0);
        for expected in [50, 100, 150] {
            r.tick().await;
            assert_eq!(r.count(), expected);
        }
    });
}
