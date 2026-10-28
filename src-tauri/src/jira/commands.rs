//! The Jira commands. Every one is gated on `UiPrefs::jira_enabled` and
//! spawns nothing while it is off: a switched-off integration reports
//! `Disabled` -- "we did not ask" -- never an empty answer (#1050).

use super::model::{IssueBatch, JiraConnection, LinkIndex, ProjectList};
use crate::store::{open_db, settings};
use std::collections::HashSet;
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
    let list = super::cli::projects_at(&bin).await;
    if list.error.is_none() {
        remember_projects(&app, list.projects.iter().map(|p| p.key.clone()).collect()).await;
    }
    Ok(list)
}

/// Keep the dictionary, so the index can be read without asking Jira.
async fn remember_projects(app: &AppHandle, keys: Vec<String>) {
    let db = crate::commands::db_path(app);
    crate::commands::off_main_thread(move || {
        if let Err(e) =
            open_db(&db).and_then(|c| settings::set(&c, settings::keys::JIRA_PROJECTS, &keys))
        {
            log::warn!("jira: could not remember projects: {e}");
        }
    })
    .await;
}

/// Every place a key in the user's projects is named (#2035).
///
/// The dictionary is the stored project list, read from Jira once if there
/// is none yet, less the projects turned off in Settings. The pull-request
/// part of the index is rebuilt first if any cached list changed.
#[tauri::command]
pub async fn jira_links(app: AppHandle) -> Result<LinkIndex, String> {
    if !enabled(&app).await {
        return Err("Jira is switched off".into());
    }
    let db = crate::commands::db_path(&app);
    let path = db.clone();
    let stored = crate::commands::off_main_thread(move || {
        open_db(&path).and_then(|c| settings::get::<Vec<String>>(&c, settings::keys::JIRA_PROJECTS))
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut error = None;
    let projects = match stored {
        Some(p) => p,
        None => {
            let bin = super::cli::find_acli().ok_or("acli is not installed")?;
            let list = super::cli::projects_at(&bin).await;
            let keys: Vec<String> = list.projects.iter().map(|p| p.key.clone()).collect();
            match list.error {
                // Not stored: an empty dictionary from a failed read would
                // hide every link until Settings listed projects again.
                Some(e) => error = Some(format!("Could not list your Jira projects: {e}")),
                None => remember_projects(&app, keys.clone()).await,
            }
            keys
        }
    };
    let app2 = app.clone();
    let excluded: HashSet<String> = crate::commands::off_main_thread(move || {
        crate::commands::read_ui_prefs(&app2).jira_excluded_projects
    })
    .await
    .into_iter()
    .collect();
    let dictionary: HashSet<String> = projects
        .into_iter()
        .filter(|p| !excluded.contains(p))
        .collect();
    let dict = dictionary.clone();
    let links = crate::commands::off_main_thread(move || {
        let mut conn = open_db(&db)?;
        super::index::refresh_prs(&mut conn)?;
        super::index::load(&conn, &dict)
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut projects: Vec<String> = dictionary.into_iter().collect();
    projects.sort();
    Ok(LinkIndex {
        links,
        projects,
        error,
    })
}

/// Note a worktree scan in the index, when Jira is on. A failure is logged:
/// the scan's own answer must not depend on it.
pub async fn note_worktrees(app: &AppHandle, scan: &crate::worktrees::RepoScan) {
    if !enabled(app).await {
        return;
    }
    let repos: Vec<(String, Vec<(String, String)>)> = scan
        .repos
        .iter()
        .map(|r| {
            (
                r.name.clone(),
                r.worktrees
                    .iter()
                    .filter(|w| !w.branch.is_empty())
                    .map(|w| (w.path.clone(), w.branch.clone()))
                    .collect(),
            )
        })
        .collect();
    let db = crate::commands::db_path(app);
    crate::commands::off_main_thread(move || {
        if let Err(e) = open_db(&db).and_then(|mut c| super::index::note_worktrees(&mut c, &repos))
        {
            log::warn!("jira: could not index worktrees: {e}");
        }
    })
    .await;
}

/// Note a repository's local branches in the index, when Jira is on.
pub async fn note_branches(app: &AppHandle, repo_path: &str, branches: &[crate::branches::Branch]) {
    if !enabled(app).await {
        return;
    }
    let names: Vec<String> = branches.iter().map(|b| b.name.clone()).collect();
    let (db, repo_path) = (crate::commands::db_path(app), repo_path.to_string());
    crate::commands::off_main_thread(move || {
        if let Err(e) =
            open_db(&db).and_then(|mut c| super::index::note_branches(&mut c, &repo_path, &names))
        {
            log::warn!("jira: could not index branches: {e}");
        }
    })
    .await;
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
