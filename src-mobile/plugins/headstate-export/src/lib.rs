//! Rust-owned local native share bridge. No direct JavaScript permissions.
#[cfg(mobile)]
use tauri::Manager;
use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Runtime,
};
pub mod cmd {
    pub const SHARE: &str = "share";
}
#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_headstate_export);
#[cfg(mobile)]
struct Native<R: Runtime>(tauri::plugin::PluginHandle<R>);
#[cfg(mobile)]
#[derive(serde::Deserialize)]
struct Reply {
    outcome: String,
}
pub async fn share<R: Runtime>(app: AppHandle<R>, markdown: String) -> Result<String, String> {
    if markdown.len() > 8 * 1024 * 1024 {
        return Err("The export exceeds 8 MiB. Select fewer turns and try again.".into());
    }
    #[cfg(mobile)]
    {
        let handle = app.state::<Native<R>>().0.clone();
        // Tauri's native callback sends to its oneshot with unwrap. Keep that
        // receiver owned until native completion, even if the command is dropped.
        tauri::async_runtime::spawn(async move {
            let reply: Reply = handle.run_mobile_plugin_async(cmd::SHARE, serde_json::json!({"markdown":markdown})).await.map_err(|_| "Could not open the share sheet. Try again, or copy the masked transcript.".to_string())?;
            match reply.outcome.as_str() {
                "cancelled" | "shared" | "presented" => Ok(reply.outcome),
                "capacity" => Err("Export storage is full for 24 hours. Reuse an unchanged export or copy the masked transcript.".into()),
                "busy" => Err("A share sheet is already open. Finish it before exporting again.".into()),
                _ => Err("Could not share the markdown. Try again, or copy the masked transcript.".into()),
            }
        }).await.map_err(|_| "Could not finish sharing. Try again.".to_string())?
    }
    #[cfg(not(mobile))]
    {
        let _ = (app, markdown);
        Err("Native sharing is unavailable on this platform.".into())
    }
}
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("headstate-export")
        .setup(|_app, _api| {
            #[cfg(target_os = "ios")]
            _app.manage(Native(
                _api.register_ios_plugin(init_plugin_headstate_export)?,
            ));
            #[cfg(target_os = "android")]
            _app.manage(Native(_api.register_android_plugin(
                "com.pktstorm.headstate.export",
                "HeadstateExportPlugin",
            )?));
            Ok(())
        })
        .build()
}
