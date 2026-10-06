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
    tokio::time::timeout(Duration::from_secs(5), controller.hooks.finished.notified())
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
        .is_empty());
}
