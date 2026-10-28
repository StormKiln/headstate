//! The Jira commands. Every one is gated on `UiPrefs::jira_enabled` and
//! spawns nothing while it is off: a switched-off integration reports
//! `Disabled` -- "we did not ask" -- never an empty answer (#1050).

use super::model::{IssueBatch, JiraConnection, ProjectList};
use tauri::AppHandle;

async fn enabled(app: &AppHandle) -> bool {
    let app = app.clone();
    crate::commands::off_main_thread(move || crate::commands::read_ui_prefs(&app).jira_enabled)
        .await
}

/// Whether Jira can be asked anything, and if not, what to do about it.
#[tauri::command]
pub async fn jira_connection(app: AppHandle) -> JiraConnection {
    if !enabled(&app).await {
        return JiraConnection::Disabled;
    }
    match super::cli::find_acli() {
        Some(bin) => super::cli::connection_at(&bin).await,
        None => JiraConnection::NotInstalled,
    }
}

/// Every project the signed-in user can see: the dictionary that tells a
/// ticket key from a look-alike such as `UTF-8`.
#[tauri::command]
pub async fn jira_projects(app: AppHandle) -> Result<ProjectList, String> {
    if !enabled(&app).await {
        return Err("Jira is switched off".into());
    }
    let bin = super::cli::find_acli().ok_or("acli is not installed")?;
    Ok(super::cli::projects_at(&bin).await)
}

/// What Jira says about `keys`: found, missing, or never asked.
///
/// The frontend batches every visible row's keys into one call, so a page
/// of pull requests is one `acli` search rather than one per row.
#[tauri::command]
pub async fn jira_issues(app: AppHandle, keys: Vec<String>) -> Result<IssueBatch, String> {
    if !enabled(&app).await {
        return Err("Jira is switched off".into());
    }
    let bin = super::cli::find_acli().ok_or("acli is not installed")?;
    Ok(super::cli::issues_at(&bin, &keys).await)
}
