//! Desktop shell only. Shared collector has no Tauri/preferences dependency.
use crate::measurement::{
    self, Config, ExportReceipt, JournalStatus, Platform, Recorder, Role, WriterState,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
static RECORDER: OnceLock<Recorder> = OnceLock::new();
static EXPORTING: AtomicBool = AtomicBool::new(false);
pub fn recorder() -> Option<&'static Recorder> {
    RECORDER.get()
}
pub fn initialize(app: &tauri::AppHandle) {
    use rand::TryRngCore;
    let mut epoch = [0; 16];
    if rand::rngs::OsRng.try_fill_bytes(&mut epoch).is_err() {
        return;
    }
    let Ok(root) = app.path().app_data_dir() else {
        return;
    };
    let platform = if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        Platform::Unknown
    };
    if let Ok(recorder) = Recorder::new(Config {
        directory: root.join("measurements"),
        epoch,
        role: Role::Desktop,
        platform,
        build: app.package_info().version.to_string(),
    }) {
        let _ = RECORDER.set(recorder);
    }
}
pub fn set_enabled(enabled: bool) {
    if let Some(recorder) = recorder() {
        recorder.set_enabled(enabled)
    }
}
pub fn status() -> JournalStatus {
    recorder().map_or_else(
        || JournalStatus {
            writer_state: WriterState::Unavailable,
            ..Default::default()
        },
        Recorder::status,
    )
}
pub async fn export(window: tauri::WebviewWindow) -> Result<ExportReceipt, String> {
    EXPORTING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| measurement::ExportError::Busy.to_string())?;
    struct Flight;
    impl Drop for Flight {
        fn drop(&mut self) {
            EXPORTING.store(false, Ordering::Release)
        }
    }
    // Own the picker/write independently of the invoking view's lifetime.
    tauri::async_runtime::spawn(async move {
        let _flight = Flight;
        let recorder =
            recorder().ok_or_else(|| measurement::ExportError::Unavailable.to_string())?;
        let (send, receive) = tokio::sync::oneshot::channel();
        window
            .dialog()
            .file()
            .set_parent(&window)
            .set_file_name("headstate-measurements.jsonl")
            .add_filter("Measurement report", &["jsonl"])
            .save_file(move |path| {
                let _ = send.send(path);
            });
        let path = receive
            .await
            .map_err(|_| "The save dialog closed unexpectedly. Try again.".to_string())?;
        match path {
            None => Ok(ExportReceipt::canceled()),
            Some(path) => recorder
                .export_to(
                    path.into_path()
                        .map_err(|_| measurement::ExportError::Destination.to_string())?,
                )
                .await
                .map_err(|e| e.to_string()),
        }
    })
    .await
    .map_err(|_| "Measurement export could not finish. Try again.".to_string())?
}
