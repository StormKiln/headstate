use super::*;
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("output");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("data"), "measured").unwrap();
    let db = temp.path().join("state.db");
    open_db(&db).unwrap();
    (temp, db, root)
}
async fn finished(controller: &Controller) -> Status {
    // A hang guard, not a measurement: Windows CI runners under load have
    // taken longer than five seconds to publish a finished scan.
    tokio::time::timeout(
        Duration::from_secs(60),
        controller.hooks.finished.notified(),
    )
    .await
    .expect("scan publication");
    let status = controller.status().unwrap();
    assert!(!status.running);
    status
}
fn controlled() -> Controller {
    let mut controller = Controller::new();
    controller.hooks.volumes = Some(vec![]);
    controller.hooks.permits = Some(Arc::new(tokio::sync::Semaphore::new(1)));
    controller
}
#[tokio::test]
async fn canceled_settings_caller_keeps_configuration_exclusion_until_actual_write_finishes() {
    let (_temp, db, root) = fixture();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let entered = Mutex::new(Some(entered_tx));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release = Mutex::new(release_rx);
    let mut controller = controlled();
    controller.hooks.before_settings_write = Some(Arc::new(move || {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
    }));
    let controller = Arc::new(controller);
    let value = Settings {
        external_roots: vec![root.display().to_string()],
        additional_locations: false,
    };
    let mut writer = tokio::spawn(controller.clone().set_settings(db.clone(), value.clone()));
    tokio::select! { result=&mut writer=>panic!("writer finished before barrier: {result:?}"), result=tokio::time::timeout(Duration::from_secs(5),entered_rx)=>result.unwrap().unwrap() }
    writer.abort();
    assert!(writer.await.unwrap_err().is_cancelled());
    let mut start = tokio::spawn(controller.clone().start(db.clone(), vec![]));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut start)
            .await
            .is_err(),
        "start crossed unfinished configuration write"
    );
    release_tx.send(()).unwrap();
    assert!(start.await.unwrap().unwrap().running);
    let status = finished(&controller).await;
    // Configuration is structured JSON, not an unescaped path string.
    let (recorded_roots, recorded_settings): (Vec<String>, Settings) =
        serde_json::from_str(&status.observations[0].configuration).unwrap();
    assert!(recorded_roots.is_empty());
    assert_eq!(recorded_settings, value);
    assert_eq!(
        status.observations[0].locations[0].logical.as_deref(),
        Some("8")
    );
    assert_eq!(load_settings(db).await.unwrap(), value);
}
#[tokio::test]
async fn queued_scan_singleflight_cancel_and_stale_ids_use_actual_worker_permit() {
    let (_temp, db, root) = fixture();
    let mut controller = controlled();
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    controller.hooks.permits = Some(permits.clone());
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = starts.clone();
    controller.hooks.before_walk = Some(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    let controller = Arc::new(controller);
    let roots = vec![root.display().to_string()];
    let (first, second) = tokio::join!(
        controller.clone().start(db.clone(), roots.clone()),
        controller.clone().start(db.clone(), roots.clone())
    );
    let first = first.unwrap();
    assert_eq!(first.run_id, second.unwrap().run_id);
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    assert!(controller
        .clone()
        .set_settings(db.clone(), Settings::default())
        .await
        .is_err());
    assert!(controller.cancel("stale").is_err());
    assert!(!controller
        .state
        .lock()
        .unwrap()
        .stop
        .load(Ordering::Relaxed));
    controller.cancel(first.run_id.as_deref().unwrap()).unwrap();
    permits.add_permits(1);
    let canceled = finished(&controller).await;
    assert_eq!(canceled.observations[0].status, "canceled");
    assert_eq!(canceled.visited, 0);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let permit = permits.acquire().await.unwrap();
    let next = controller.clone().start(db, roots).await.unwrap();
    assert_ne!(first.run_id, next.run_id);
    assert!(controller.cancel(first.run_id.as_deref().unwrap()).is_err());
    assert!(!controller
        .state
        .lock()
        .unwrap()
        .stop
        .load(Ordering::Relaxed));
    controller.cancel(next.run_id.as_deref().unwrap()).unwrap();
    drop(permit);
    finished(&controller).await;
}
#[tokio::test]
async fn failed_history_save_still_publishes_current_actual_measurement() {
    let (_temp, db, root) = fixture();
    open_db(&db).unwrap().execute_batch("CREATE TRIGGER deny_disk BEFORE INSERT ON disk_observations BEGIN SELECT RAISE(ABORT,'controlled history refusal'); END;").unwrap();
    let controller = Arc::new(controlled());
    controller
        .clone()
        .start(db.clone(), vec![root.display().to_string()])
        .await
        .unwrap();
    let status = finished(&controller).await;
    assert!(status.error.is_none());
    assert_eq!(
        status.observations[0].locations[0].logical.as_deref(),
        Some("8")
    );
    assert!(status.observations[0]
        .history_error
        .as_ref()
        .unwrap()
        .contains("controlled history refusal"));
    assert!(disk_observations::history(&open_db(&db).unwrap())
        .unwrap()
        .observations
        .is_empty());
}

#[tokio::test]
async fn start_continues_bounded_observation_and_restart_keeps_durable_floor() {
    let (_temp, db, root) = fixture();
    for i in 0..20 {
        std::fs::write(root.join(format!("file-{i}")), [1u8; 10]).unwrap();
    }
    let mut controller = controlled();
    controller.hooks.measurement_steps = Some(9);
    let controller = Arc::new(controller);
    let roots = vec![root.display().to_string()];
    controller
        .clone()
        .start(db.clone(), roots.clone())
        .await
        .unwrap();
    let first = finished(&controller).await;
    assert!(first.resumable);
    assert!(first.remaining > 0);
    assert_eq!(first.phase.as_deref(), Some("measurement"));
    let observation_id = first.observations[0].id.clone();
    let floor = first.observations[0].known_allocated.clone();
    assert!(floor.is_some());
    assert!(disk_observations::history(&open_db(&db).unwrap())
        .unwrap()
        .observations
        .iter()
        .any(|o| o.id == observation_id && o.known_allocated == floor));
    let mut done = first;
    for _ in 0..100 {
        if !done.resumable {
            break;
        }
        let started = controller
            .clone()
            .start(db.clone(), roots.clone())
            .await
            .unwrap();
        assert!(
            !started.observations.is_empty(),
            "prior results survive while continuing"
        );
        done = finished(&controller).await;
    }
    assert!(!done.resumable);
    assert_eq!(done.observations[0].id, observation_id);
    assert_eq!(
        done.observations[0].locations[0].logical.as_deref(),
        Some("208")
    );
    // Process restart cannot resume open directory cursors, but does preserve
    // saved partial/completed observations and creates a fresh observation.
    let restarted = Arc::new(controlled());
    restarted.clone().start(db.clone(), roots).await.unwrap();
    let fresh = finished(&restarted).await;
    assert_ne!(fresh.observations[0].id, observation_id);
    assert_eq!(
        fresh.observations[0].known_allocated,
        done.observations[0].known_allocated
    );
}

#[tokio::test]
async fn discovery_frontier_continues_before_measurement_and_settings_reset_cursor() {
    let (_temp, db, root) = fixture();
    let mut deep = root.clone();
    for _ in 0..9 {
        deep = deep.join("nested");
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("payload"), "deep").unwrap();
    let controller = Arc::new(controlled());
    controller
        .clone()
        .start(db.clone(), vec![root.display().to_string()])
        .await
        .unwrap();
    let first = finished(&controller).await;
    assert!(first.resumable);
    assert_eq!(first.phase.as_deref(), Some("discovery"));
    assert_eq!(first.visited, 0);
    controller
        .clone()
        .start(db.clone(), vec![root.display().to_string()])
        .await
        .unwrap();
    let done = finished(&controller).await;
    assert!(!done.resumable);
    assert_eq!(
        done.observations[0].locations[0].logical.as_deref(),
        Some("12")
    );
    assert_eq!(first.observations[0].id, done.observations[0].id);
    // A changed root set starts new discovery, not a cursor in the old scope.
    controller
        .clone()
        .start(db.clone(), vec![root.display().to_string()])
        .await
        .unwrap();
    assert!(finished(&controller).await.resumable);
    controller
        .clone()
        .start(db, vec![deep.display().to_string()])
        .await
        .unwrap();
    let changed = finished(&controller).await;
    assert!(!changed.resumable);
    assert_eq!(
        changed.observations[0].locations[0].logical.as_deref(),
        Some("4")
    );
}

#[tokio::test]
#[ignore = "Explicit read-only host inventory; HEADSTATE_DISK_ACCEPTANCE_ROOTS JSON required"]
async fn host_inventory_bounded_continuation_acceptance() {
    let Some(roots) = std::env::var_os("HEADSTATE_DISK_ACCEPTANCE_ROOTS") else {
        return;
    };
    let roots: Vec<String> =
        serde_json::from_str(&roots.to_string_lossy()).expect("JSON array of selected roots");
    assert!(!roots.is_empty() && roots.len() <= MAX_ROOTS);
    let temporary = tempfile::tempdir().unwrap();
    let db = temporary.path().join("acceptance.sqlite");
    open_db(&db).unwrap();
    let mut controller = Controller::new();
    controller.hooks.permits = Some(Arc::new(tokio::sync::Semaphore::new(1)));
    let controller = Arc::new(controller);
    for batch in 1..=100 {
        controller
            .clone()
            .start(db.clone(), roots.clone())
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(300),
            controller.hooks.finished.notified(),
        )
        .await
        .expect("bounded batch publication");
        let status = controller.status().unwrap();
        assert!(status.error.is_none(), "Inventory worker failed");
        let summary:Vec<_>=status.observations.iter().map(|o|serde_json::json!({
            "status":o.status,"visited":o.visited,"locations":o.locations.len(),"known_allocated":o.known_allocated,
            "allocation_complete":o.allocation_complete,"coverage_count":o.coverage.len(),
            "incomplete_coverage_count":o.coverage.iter().filter(|c|c.affects_completeness).count(),
            "categories":o.categories.iter().map(|c|serde_json::json!({"name":c.name,"locations":c.locations.len(),"known_allocated":c.known_allocated})).collect::<Vec<_>>()
        })).collect();
        println!(
            "{}",
            serde_json::json!({"batch":batch,"phase":status.phase,"remaining":status.remaining,"resumable":status.resumable,"observations":summary})
        );
        if !status.resumable {
            return;
        }
    }
    panic!("Inventory remains resumable after 100 bounded batches");
}
