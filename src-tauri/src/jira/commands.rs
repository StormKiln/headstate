//! The Jira commands. Every one is gated on `UiPrefs::jira_enabled` and
//! spawns nothing while it is off: a switched-off integration reports
//! `Disabled` -- "we did not ask" -- never an empty answer (#1050).

use super::model::{IssueBatch, JiraConnection, ProjectList};
use crate::store::{open_db, settings};
use tauri::AppHandle;

async fn enabled(app: &AppHandle) -> bool {
    let app = app.clone();
    crate::commands::off_main_thread(move || crate::commands::read_ui_prefs(&app).jira_enabled)
        .await
}

/// The site `acli` last named, for browse links. `None` until a
/// connection check has seen one.
async fn site(app: &AppHandle) -> Option<String> {
    let db = crate::commands::db_path(app);
    crate::commands::off_main_thread(move || {
        open_db(&db).ok().and_then(|c| {
            settings::get::<String>(&c, settings::keys::JIRA_SITE)
                .ok()
                .flatten()
        })
    })
    .await
}

/// Whether Jira can be asked anything, and if not, what to do about it.
#[tauri::command]
pub async fn jira_connection(app: AppHandle) -> JiraConnection {
    if !enabled(&app).await {
        return JiraConnection::Disabled;
    }
    let Some(bin) = super::cli::find_acli() else {
        return JiraConnection::NotInstalled;
    };
    let got = super::cli::connection_at(&bin).await;
    if let JiraConnection::LoggedIn {
        site: Some(site), ..
    } = &got
    {
        let (db, site) = (crate::commands::db_path(&app), site.clone());
        crate::commands::off_main_thread(move || {
            if let Err(e) =
                open_db(&db).and_then(|c| settings::set(&c, settings::keys::JIRA_SITE, &site))
            {
                log::warn!("jira: could not remember the site: {e}");
            }
        })
        .await;
    }
    got
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
/// of pull requests is one `acli` search rather than one per row. Answers
/// are kept in the database (`read.rs`), so a repeat within a minute spawns
/// nothing and a parent is read once, not on every refresh.
#[tauri::command]
pub async fn jira_issues(app: AppHandle, keys: Vec<String>) -> Result<IssueBatch, String> {
    if !enabled(&app).await {
        return Err("Jira is switched off".into());
    }
    let bin = super::cli::find_acli().ok_or("acli is not installed")?;
    let site = site(&app).await;
    let db = crate::commands::db_path(&app);
    Ok(super::read::issues(&bin, &db, site.as_deref(), &keys).await)
}

/// How many assigned issues one read returns. Past this the page says "at
/// least" rather than presenting a partial list as the whole.
const ASSIGNED_LIMIT: usize = 100;

/// Open work assigned to the signed-in user -- including issues with no
/// pull request yet, which key detection alone can never find.
#[tauri::command]
pub async fn jira_assigned(app: AppHandle) -> Result<IssueBatch, String> {
    if !enabled(&app).await {
        return Err("Jira is switched off".into());
    }
    let bin = super::cli::find_acli().ok_or("acli is not installed")?;
    let site = site(&app).await;
    let db = crate::commands::db_path(&app);
    Ok(super::read::assigned(&bin, &db, site.as_deref(), ASSIGNED_LIMIT).await)
}
