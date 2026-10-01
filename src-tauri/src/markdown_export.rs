//! Local-only native export. No transcript reads and no content-bearing errors.
use std::{
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use tauri_plugin_dialog::DialogExt;
const MAX_BYTES: usize = 8 * 1024 * 1024;
static BUSY: AtomicBool = AtomicBool::new(false);
struct Flight;
impl Drop for Flight {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
fn validate(markdown: &str) -> Result<(), String> {
    if markdown.len() > MAX_BYTES {
        return Err("The export exceeds 8 MiB. Select fewer turns and try again.".into());
    }
    Ok(())
}
fn atomic_write(path: &Path, markdown: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Choose a local destination folder and try again.")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "Could not create the export. Choose a writable folder and try again.")?;
    staged
        .write_all(markdown.as_bytes())
        .and_then(|_| staged.flush())
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|_| "Could not write the export. Check free space and try again.")?;
    staged
        .persist(path)
        .map_err(|_| "Could not replace the file. Choose another destination and try again.")?;
    Ok(())
}
fn save_selected(path: Option<std::path::PathBuf>, markdown: &str) -> Result<&'static str, String> {
    match path {
        None => Ok("cancelled"),
        Some(path) => {
            atomic_write(&path, markdown)?;
            Ok("saved")
        }
    }
}
pub async fn save_markdown(
    window: tauri::WebviewWindow,
    markdown: String,
) -> Result<String, String> {
    validate(&markdown)?;
    BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "An export is already open. Finish it before exporting again.")?;
    // Own the picker and write even if the invoking webview disappears.
    tauri::async_runtime::spawn(async move {
        let _flight = Flight;
        let (send, receive) = tokio::sync::oneshot::channel::<Option<tauri_plugin_fs::FilePath>>();
        window
            .dialog()
            .file()
            .set_parent(&window)
            .set_file_name("transcript.md")
            .add_filter("Markdown", &["md"])
            .save_file(move |path| {
                let _ = send.send(path);
            });
        let selection = receive
            .await
            .map_err(|_| "The save dialog closed unexpectedly. Try again.")?;
        let path = selection
            .map(|p| {
                p.into_path()
                    .map_err(|_| "Choose a local file destination.")
            })
            .transpose()?;
        tauri::async_runtime::spawn_blocking(move || {
            save_selected(path, &markdown).map(str::to_owned)
        })
        .await
        .map_err(|_| "The export could not finish. Try again.".to_string())?
    })
    .await
    .map_err(|_| "The export could not finish. Try again.".to_string())?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_and_atomic_replacement_preserve_exact_bytes_and_original_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.md");
        std::fs::write(&path, b"original").unwrap();
        assert_eq!(save_selected(None, "not written").unwrap(), "cancelled");
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let text = "# café 😀\nEarlier messages were not loaded.\n[masked]\n";
        assert_eq!(save_selected(Some(path.clone()), text).unwrap(), "saved");
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
        let folder = dir.path().join("existing");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("original"), b"keep").unwrap();
        assert!(atomic_write(&folder, "replacement").is_err());
        assert_eq!(std::fs::read(folder.join("original")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        atomic_write(&path, "retry").unwrap();
    }
    #[test]
    fn size_limit_is_utf8_bytes() {
        assert!(validate(&"é".repeat(MAX_BYTES / 2)).is_ok());
        assert!(validate(&"é".repeat(MAX_BYTES / 2 + 1)).is_err());
    }
}
