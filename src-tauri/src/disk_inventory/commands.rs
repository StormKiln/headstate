use super::{
    accounting, discovery, measure,
    model::{Observation, Settings, Status, MAX_ROOTS},
    platform,
};
use crate::store::{disk_observations, open_db, settings};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use tauri::AppHandle;
// Serializes configuration persistence with capturing a new scan's settings.
// The filesystem worker never holds this lock.
static CONFIGURATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const SETTINGS_KEY: &str = "disk_inventory_settings_v1";
struct Run {
    status: Status,
    stop: Arc<AtomicBool>,
}
fn run() -> &'static Mutex<Run> {
    static RUN: OnceLock<Mutex<Run>> = OnceLock::new();
    RUN.get_or_init(|| {
        Mutex::new(Run {
            status: Status::default(),
            stop: Arc::new(AtomicBool::new(false)),
        })
    })
}
#[tauri::command]
pub async fn disk_inventory_settings(app: AppHandle) -> Result<Settings, String> {
    let db = crate::commands::db_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&db).map_err(|e| e.to_string())?;
        settings::get(&conn, SETTINGS_KEY)
            .map(|s| s.unwrap_or_default())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn set_disk_inventory_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let _configuration = CONFIGURATION.lock().await;
    if settings.external_roots.len() > MAX_ROOTS
        || settings
            .external_roots
            .iter()
            .any(|p| p.len() > 4096 || !std::path::Path::new(p).is_absolute())
    {
        return Err(
            "Choose at most 32 absolute folder paths, each at most 4096 characters.".into(),
        );
    }
    if run()
        .lock()
        .map_err(|_| "Scan state unavailable")?
        .status
        .running
    {
        return Err("Stop the current scan before changing its locations.".into());
    }
    let db = crate::commands::db_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        // The actual write owns configuration exclusion even if its caller goes away.
        let _configuration = _configuration;
        let conn = open_db(&db).map_err(|e| e.to_string())?;
        crate::store::settings::set(&conn, SETTINGS_KEY, &settings).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn disk_inventory_history(app: AppHandle) -> Result<Vec<Observation>, String> {
    let db = crate::commands::db_path(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&db).map_err(|e| e.to_string())?;
        disk_observations::history(&conn)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub fn disk_inventory_status() -> Result<Status, String> {
    Ok(run()
        .lock()
        .map_err(|_| "Scan state unavailable")?
        .status
        .clone())
}
#[tauri::command]
pub fn cancel_disk_inventory(run_id: String) -> Result<(), String> {
    let state = run().lock().map_err(|_| "Scan state unavailable")?;
    if state.status.run_id.as_deref() != Some(&run_id) {
        return Err("That scan is no longer current.".into());
    }
    state.stop.store(true, Ordering::Relaxed);
    Ok(())
}
#[tauri::command]
pub async fn start_disk_inventory(app: AppHandle) -> Result<Status, String> {
    let _configuration = CONFIGURATION.lock().await;
    let settings = disk_inventory_settings(app.clone()).await?;
    let roots = crate::commands::get_worktree_dirs(app.clone());
    if roots.len() > MAX_ROOTS {
        return Err("Select at most 32 repository roots for disk discovery.".into());
    }
    let (id, stop, status) = {
        let mut state = run().lock().map_err(|_| "Scan state unavailable")?;
        if state.status.running {
            return Ok(state.status.clone());
        }
        let id = format!(
            "{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        state.stop = Arc::new(AtomicBool::new(false));
        state.status.run_id = Some(id.clone());
        state.status.running = true;
        state.status.visited = 0;
        state.status.error = None;
        (id, state.stop.clone(), state.status.clone())
    };
    let db = crate::commands::db_path(&app);
    tauri::async_runtime::spawn(async move {
        let current_id = id.clone();
        let result = crate::commands::scan_blocking(move || {
            let started = std::time::Instant::now();
            let configuration =
                serde_json::to_string(&(&roots, &settings)).map_err(|e| e.to_string())?;
            let volumes = platform::volumes(&stop);
            let found = discovery::discover(&roots, &settings, &stop, started);
            let mut observations = measure::measure(
                found,
                volumes,
                configuration,
                &id,
                &stop,
                started,
                |visited, path| {
                    if let Ok(mut state) = run().lock() {
                        if state.status.run_id.as_deref() == Some(&id) {
                            state.status.visited = visited;
                            state.status.current_path = Some(path.into());
                        }
                    }
                },
            );
            match open_db(&db) {
                Ok(mut conn) => {
                    for observation in &mut observations {
                        match disk_observations::latest(&conn, &observation.volume.id) {
                            Ok(previous) => {
                                observation.comparison =
                                    accounting::compare(observation, previous.as_ref())
                            }
                            Err(error) => observation.history_error = Some(error),
                        }
                        if let Err(error) = disk_observations::save(&mut conn, observation) {
                            observation.history_error = Some(error);
                        }
                    }
                }
                Err(e) => {
                    for observation in &mut observations {
                        observation.history_error = Some(e.to_string());
                    }
                }
            }
            Ok::<_, String>(observations)
        })
        .await
        .and_then(|r| r);
        if let Ok(mut state) = run().lock() {
            if state.status.run_id.as_deref() == Some(&current_id) {
                state.status.running = false;
                state.status.current_path = None;
                match result {
                    Ok(observations) => state.status.observations = observations,
                    Err(error) => state.status.error = Some(error),
                }
            }
        }
    });
    Ok(status)
}
