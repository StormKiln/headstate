//! Independent phone journal. No paired desktop, RPC, or remote preference state.
use crate::{measurement as m, store};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Manager, State};
const PREFS_KEY: &str = "phone-measurement-prefs";
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefs {
    pub enabled: bool,
}
#[derive(Serialize)]
pub struct ControlReply {
    pub enabled: bool,
    pub capture: Option<m::CaptureIdentity>,
}
pub struct PhoneMeasurement {
    store: Arc<dyn store::Store>,
    recorder: Option<m::Recorder>,
    directory: PathBuf,
    control: Mutex<()>,
    sharing: AtomicBool,
}
impl PhoneMeasurement {
    pub fn new(
        store: Arc<dyn store::Store>,
        directory: PathBuf,
        recorder: Option<m::Recorder>,
    ) -> Self {
        let enabled = store::get_json::<Prefs>(store.as_ref(), PREFS_KEY)
            .ok()
            .flatten()
            .unwrap_or_default()
            .enabled;
        if let Some(r) = &recorder {
            r.set_enabled(enabled);
        }
        Self {
            store,
            recorder,
            directory,
            control: Mutex::new(()),
            sharing: AtomicBool::new(false),
        }
    }
    fn prefs(&self) -> Result<Prefs, String> {
        store::get_json(self.store.as_ref(), PREFS_KEY)
            .map(|p| p.unwrap_or_default())
            .map_err(|_| "Phone measurement preferences could not be read.".into())
    }
    fn set(&self, prefs: Prefs) -> Result<ControlReply, String> {
        let _control = self
            .control
            .lock()
            .map_err(|_| "Phone measurement preferences are unavailable.")?;
        store::put_json(self.store.as_ref(), PREFS_KEY, &prefs)
            .map_err(|_| "Phone measurement preferences could not be saved.")?;
        if let Some(r) = &self.recorder {
            r.set_enabled(prefs.enabled);
        }
        Ok(ControlReply {
            enabled: prefs.enabled,
            capture: self
                .recorder
                .as_ref()
                .and_then(|r| r.preference_capture(true)),
        })
    }
    fn status(&self) -> m::JournalStatus {
        self.recorder.as_ref().map_or_else(
            || m::JournalStatus {
                writer_state: m::WriterState::Unavailable,
                ..Default::default()
            },
            m::Recorder::status,
        )
    }
}
pub fn initialize(app: &tauri::App, store: Arc<dyn store::Store>, root: PathBuf) {
    // Resolve only the OS-owned application root; journal children still reject symlinks.
    let directory = root
        .canonicalize()
        .unwrap_or(root)
        .join("phone-measurements");
    let recorder = crate::keys::random_bytes::<16>().ok().and_then(|epoch| {
        m::Recorder::new(m::Config {
            directory: directory.clone(),
            epoch,
            role: m::Role::Phone,
            platform: if cfg!(target_os = "ios") {
                m::Platform::Ios
            } else if cfg!(target_os = "android") {
                m::Platform::Android
            } else {
                m::Platform::Unknown
            },
            build: app.package_info().version.to_string(),
        })
        .ok()
    });
    app.manage(Arc::new(PhoneMeasurement::new(store, directory, recorder)));
}
#[tauri::command]
pub fn get_phone_measurement_prefs(
    state: State<'_, Arc<PhoneMeasurement>>,
) -> Result<Prefs, String> {
    state.prefs()
}
#[tauri::command]
pub fn set_phone_measurement_prefs(
    state: State<'_, Arc<PhoneMeasurement>>,
    prefs: Prefs,
) -> Result<ControlReply, String> {
    state.set(prefs)
}
#[tauri::command]
pub fn phone_measurement_status(state: State<'_, Arc<PhoneMeasurement>>) -> m::JournalStatus {
    state.status()
}
#[tauri::command]
pub fn record_phone_measurements(
    state: State<'_, Arc<PhoneMeasurement>>,
    batch: Vec<m::ClientMeasurement>,
) -> Result<(), String> {
    if let Some(r) = &state.recorder {
        r.client_events(batch).map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[derive(Serialize)]
pub struct ShareReply {
    outcome: String,
    report: m::ExportReceipt,
}
#[tauri::command]
pub async fn export_phone_measurements(
    app: tauri::AppHandle,
    state: State<'_, Arc<PhoneMeasurement>>,
) -> Result<ShareReply, String> {
    let state = state.inner().clone();
    if state
        .sharing
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("A measurement report is already being shared.".into());
    }
    // The owner survives JS command cancellation until native presentation finishes.
    tauri::async_runtime::spawn(async move {
        struct Guard(Arc<PhoneMeasurement>, PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.1);
                self.0.sharing.store(false, Ordering::Release);
            }
        }
        let path = state.directory.join("recent-share.jsonl");
        let _guard = Guard(state.clone(), path.clone());
        let recorder = state
            .recorder
            .as_ref()
            .ok_or("Phone measurements are unavailable.")?;
        let report = recorder
            .export_recent_to(path.clone())
            .await
            .map_err(|e| e.to_string())?;
        let content = tauri::async_runtime::spawn_blocking(move || {
            use std::io::Read;
            let file = std::fs::File::open(path)
                .map_err(|_| "Could not read the phone measurement report.")?;
            let mut bytes = Vec::new();
            file.take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "Could not read the phone measurement report.")?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err("Phone measurement report exceeds 8 MiB.");
            }
            String::from_utf8(bytes).map_err(|_| "Phone measurement report is invalid.")
        })
        .await
        .map_err(|_| "Could not prepare the phone measurement report.")??;
        let outcome = tauri_plugin_headstate_export::share_kind(
            app,
            content,
            tauri_plugin_headstate_export::ExportKind::MeasurementJsonl,
        )
        .await?;
        Ok(ShareReply { outcome, report })
    })
    .await
    .map_err(|_| "Could not finish sharing phone measurements.".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn instance(store: Arc<dyn store::Store>, root: PathBuf) -> PhoneMeasurement {
        let recorder = m::Recorder::new(m::Config {
            directory: root.clone(),
            epoch: [6; 16],
            role: m::Role::Phone,
            platform: m::Platform::Ios,
            build: "test".into(),
        })
        .unwrap();
        PhoneMeasurement::new(store, root, Some(recorder))
    }
    #[test]
    fn local_capture_defaults_off_persists_and_restarts_without_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(store::MemoryStore::default());
        let phone = instance(
            store.clone(),
            dir.path().canonicalize().unwrap().join("journal"),
        );
        assert!(!phone.prefs().unwrap().enabled);
        assert!(!phone.status().enabled);
        let enabled = phone.set(Prefs { enabled: true }).unwrap();
        assert!(enabled.capture.is_some());
        assert!(phone.status().enabled);
        drop(phone);
        let phone = instance(store, dir.path().canonicalize().unwrap().join("journal"));
        assert!(phone.status().enabled);
        let off = phone.set(Prefs { enabled: false }).unwrap();
        assert!(off.capture.is_none());
        assert!(!phone.status().enabled);
    }
    #[test]
    fn failed_preference_store_cannot_enable_capture() {
        struct Failed;
        impl store::Store for Failed {
            fn get(&self, _: &str) -> Result<Option<Vec<u8>>, store::StoreError> {
                Ok(None)
            }
            fn put(&self, _: &str, _: &[u8]) -> Result<(), store::StoreError> {
                Err(store::StoreError::Backend("synthetic".into()))
            }
            fn remove(&self, _: &str) -> Result<(), store::StoreError> {
                Ok(())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let phone = instance(
            Arc::new(Failed),
            dir.path().canonicalize().unwrap().join("journal"),
        );
        assert!(phone.set(Prefs { enabled: true }).is_err());
        assert!(!phone.status().enabled);
    }
}
