use super::{
    accounting, discovery, measure,
    model::{Observation, Settings, Status, MAX_ROOTS},
    platform,
};
use crate::store::{disk_observations, open_db, settings};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};
use tauri::AppHandle;
const SETTINGS_KEY: &str = "disk_inventory_settings_v1";

struct Run {
    status: Status,
    stop: Arc<AtomicBool>,
}
struct Controller {
    state: Mutex<Run>,
    configuration: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    hooks: TestHooks,
}
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    before_settings_write: Option<Arc<dyn Fn() + Send + Sync>>,
    before_walk: Option<Arc<dyn Fn() + Send + Sync>>,
    permits: Option<Arc<tokio::sync::Semaphore>>,
    volumes: Option<Vec<super::model::Volume>>,
    finished: tokio::sync::Notify,
}
impl Controller {
    fn new() -> Self {
        Self {
            state: Mutex::new(Run {
                status: Status::default(),
                stop: Arc::new(AtomicBool::new(false)),
            }),
            configuration: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            hooks: TestHooks::default(),
        }
    }
    fn status(&self) -> Result<Status, String> {
        Ok(self
            .state
            .lock()
            .map_err(|_| "Scan state unavailable")?
            .status
            .clone())
    }
    fn cancel(&self, run_id: &str) -> Result<(), String> {
        let state = self.state.lock().map_err(|_| "Scan state unavailable")?;
        if state.status.run_id.as_deref() != Some(run_id) {
            return Err("That scan is no longer current.".into());
        }
        state.stop.store(true, Ordering::Relaxed);
        Ok(())
    }
    async fn set_settings(self: Arc<Self>, db: PathBuf, value: Settings) -> Result<(), String> {
        let configuration = self.configuration.clone().lock_owned().await;
        if value.external_roots.len() > MAX_ROOTS
            || value
                .external_roots
                .iter()
                .any(|p| p.len() > 4096 || !std::path::Path::new(p).is_absolute())
        {
            return Err(
                "Choose at most 32 absolute folder paths, each at most 4096 characters.".into(),
            );
        }
        if self.status()?.running {
            return Err("Stop the current scan before changing its locations.".into());
        }
        tauri::async_runtime::spawn_blocking(move || {
            // The write, not its possibly canceled caller, owns exclusion.
            let _configuration = configuration;
            #[cfg(test)]
            if let Some(hook) = &self.hooks.before_settings_write {
                hook();
            }
            let conn = open_db(&db).map_err(|e| e.to_string())?;
            settings::set(&conn, SETTINGS_KEY, &value).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }
    async fn start(self: Arc<Self>, db: PathBuf, roots: Vec<String>) -> Result<Status, String> {
        let _configuration = self.configuration.clone().lock_owned().await;
        let settings = load_settings(db.clone()).await?;
        if roots.len() > MAX_ROOTS {
            return Err("Select at most 32 repository roots for disk discovery.".into());
        }
        let (id, stop, status) = {
            let mut state = self.state.lock().map_err(|_| "Scan state unavailable")?;
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
        let controller = self.clone();
        tauri::async_runtime::spawn(async move {
            let current_id = id.clone();
            let worker_controller = controller.clone();
            let work = move || worker_controller.walk(db, roots, settings, id, stop);
            #[cfg(test)]
            let result = if let Some(permits) = &controller.hooks.permits {
                crate::commands::blocking_under(permits.clone(), work).await
            } else {
                crate::commands::scan_blocking(work).await
            };
            #[cfg(not(test))]
            let result = crate::commands::scan_blocking(work).await;
            controller.publish(&current_id, result.and_then(|r| r));
        });
        Ok(status)
    }
    fn walk(
        &self,
        db: PathBuf,
        roots: Vec<String>,
        settings: Settings,
        id: String,
        stop: Arc<AtomicBool>,
    ) -> Result<Vec<Observation>, String> {
        #[cfg(test)]
        if let Some(hook) = &self.hooks.before_walk {
            hook();
        }
        let started = std::time::Instant::now();
        let configuration =
            serde_json::to_string(&(&roots, &settings)).map_err(|e| e.to_string())?;
        #[cfg(test)]
        let volumes = self
            .hooks
            .volumes
            .clone()
            .unwrap_or_else(|| platform::volumes(&stop));
        #[cfg(not(test))]
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
                if let Ok(mut state) = self.state.lock() {
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
            Err(error) => {
                for observation in &mut observations {
                    observation.history_error = Some(error.to_string());
                }
            }
        }
        Ok(observations)
    }
    fn publish(&self, id: &str, result: Result<Vec<Observation>, String>) {
        if let Ok(mut state) = self.state.lock() {
            if state.status.run_id.as_deref() == Some(id) {
                state.status.running = false;
                state.status.current_path = None;
                match result {
                    Ok(observations) => state.status.observations = observations,
                    Err(error) => state.status.error = Some(error),
                }
            }
        }
        #[cfg(test)]
        self.hooks.finished.notify_one();
    }
}
fn controller() -> Arc<Controller> {
    static CONTROLLER: OnceLock<Arc<Controller>> = OnceLock::new();
    CONTROLLER
        .get_or_init(|| Arc::new(Controller::new()))
        .clone()
}
async fn load_settings(db: PathBuf) -> Result<Settings, String> {
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
pub async fn disk_inventory_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings(crate::commands::db_path(&app)).await
}
#[tauri::command]
pub async fn set_disk_inventory_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    controller()
        .set_settings(crate::commands::db_path(&app), settings)
        .await
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
    controller().status()
}
#[tauri::command]
pub fn cancel_disk_inventory(run_id: String) -> Result<(), String> {
    controller().cancel(&run_id)
}
#[tauri::command]
pub async fn start_disk_inventory(app: AppHandle) -> Result<Status, String> {
    controller()
        .start(
            crate::commands::db_path(&app),
            crate::commands::get_worktree_dirs(app),
        )
        .await
}

#[cfg(test)]
mod tests;
