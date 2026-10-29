//! The Jira commands. Every one is gated on `UiPrefs::jira_enabled` and
//! spawns nothing while it is off: a switched-off integration reports
//! `Disabled` or [`JiraError::NotAsked`] -- "we did not ask" -- never an
//! empty answer (#1050).
//!
//! Each command is a thin wrapper: it reads the switch and the paths it
//! needs from the app, then hands them to a `*_with` function that takes
//! the switch and the transport as values. The wrappers need a running
//! app; the `*_with` functions do not, which is what lets a test prove
//! that "off" spawns nothing (#2034).

use super::model::{
    IssueBatch, JiraConnection, JiraError, LinkIndex, ProjectList, ProjectStatuses,
};
use super::transport::Transport;
use crate::store::{open_db, settings};
use std::collections::HashSet;
use std::path::Path;
use tauri::AppHandle;

async fn enabled(app: &AppHandle) -> bool {
    let app = app.clone();
    crate::commands::off_main_thread(move || crate::commands::read_ui_prefs(&app).jira_enabled)
        .await
}

/// The transport, or why none will be asked. `locate` runs only when Jira
/// is on, so a switched-off app never even looks for `acli`.
fn gate(
    enabled: bool,
    locate: impl FnOnce() -> Result<Transport, JiraError>,
) -> Result<Transport, JiraError> {
    if !enabled {
        return Err(JiraError::NotAsked);
    }
    locate()
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
    let got = connection_with(gate(enabled(&app).await, Transport::locate)).await;
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

async fn connection_with(t: Result<Transport, JiraError>) -> JiraConnection {
    match t {
        Ok(t) => t.connection().await,
        Err(JiraError::NotAsked) => JiraConnection::Disabled,
        Err(JiraError::NotInstalled { searched }) => JiraConnection::NotInstalled { searched },
        Err(e) => JiraConnection::CannotTell {
            reason: e.to_string(),
        },
    }
}

/// Every project the signed-in user can see: the dictionary that tells a
/// ticket key from a look-alike such as `UTF-8`. A project list that could
/// not be read is `Ok` with its `error` set; only "not asked" and "not
/// installed" reject.
#[tauri::command]
pub async fn jira_projects(app: AppHandle) -> Result<ProjectList, JiraError> {
    let list = projects_with(gate(enabled(&app).await, Transport::locate)).await?;
    if list.error.is_none() {
        remember_projects(&app, list.projects.iter().map(|p| p.key.clone()).collect()).await;
    }
    Ok(list)
}

async fn projects_with(t: Result<Transport, JiraError>) -> Result<ProjectList, JiraError> {
    Ok(t?.projects().await)
}

/// The installed `acli`'s version, for Settings › Jira's Connection
/// section. Asked only while Jira is on: `tools/version.rs` deliberately
/// does not report `acli`, because that would spawn it while Jira is off
/// (#2033).
#[tauri::command]
pub async fn jira_acli_version(app: AppHandle) -> Result<String, JiraError> {
    gate(enabled(&app).await, Transport::locate)?
        .version()
        .await
}

/// The statuses seen in `project`'s recent issues, to choose a status
/// expectation from (#2034). `acli` has no command that lists a workflow's
/// statuses, so this is a sample, and says so when it hit its limit.
#[tauri::command]
pub async fn jira_project_statuses(
    app: AppHandle,
    project: String,
) -> Result<ProjectStatuses, JiraError> {
    Ok(gate(enabled(&app).await, Transport::locate)?
        .project_statuses(&project, super::cli::STATUS_SAMPLE)
        .await)
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
pub async fn jira_links(app: AppHandle) -> Result<LinkIndex, JiraError> {
    if !enabled(&app).await {
        return Err(JiraError::NotAsked);
    }
    let db = crate::commands::db_path(&app);
    let path = db.clone();
    let stored = crate::commands::off_main_thread(move || {
        open_db(&path).and_then(|c| settings::get::<Vec<String>>(&c, settings::keys::JIRA_PROJECTS))
    })
    .await
    .map_err(|e| JiraError::failed(e.to_string()))?;
    let mut error = None;
    let projects = match stored {
        Some(p) => p,
        None => {
            let list = Transport::locate()?.projects().await;
            let keys: Vec<String> = list.projects.iter().map(|p| p.key.clone()).collect();
            match list.error {
                // Not stored: an empty dictionary from a failed read would
                // hide every link until Settings listed projects again.
                Some(e) => error = Some(e),
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
    .map_err(|e| JiraError::failed(e.to_string()))?;
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
///
/// Rejects only when nothing could be asked (off, or no `acli`). A read
/// that was rate limited, timed out or refused part-way is `Ok`, with what
/// it found and the reason in `error`.
#[tauri::command]
pub async fn jira_issues(app: AppHandle, keys: Vec<String>) -> Result<IssueBatch, JiraError> {
    let t = gate(enabled(&app).await, Transport::locate);
    let site = site(&app).await;
    let db = crate::commands::db_path(&app);
    issues_with(t, &db, site.as_deref(), &keys).await
}

async fn issues_with(
    t: Result<Transport, JiraError>,
    db: &Path,
    site: Option<&str>,
    keys: &[String],
) -> Result<IssueBatch, JiraError> {
    Ok(super::read::issues(&t?, db, site, keys).await)
}

/// How many assigned issues one read returns. Past this the page says "at
/// least" rather than presenting a partial list as the whole.
const ASSIGNED_LIMIT: usize = 100;

/// Open work assigned to the signed-in user -- including issues with no
/// pull request yet, which key detection alone can never find.
#[tauri::command]
pub async fn jira_assigned(app: AppHandle) -> Result<IssueBatch, JiraError> {
    let t = gate(enabled(&app).await, Transport::locate);
    let site = site(&app).await;
    let db = crate::commands::db_path(&app);
    assigned_with(t, &db, site.as_deref()).await
}

async fn assigned_with(
    t: Result<Transport, JiraError>,
    db: &Path,
    site: Option<&str>,
) -> Result<IssueBatch, JiraError> {
    Ok(super::read::assigned(&t?, db, site, ASSIGNED_LIMIT).await)
}

/// Every pull request, worktree and branch that names `key` (#2041): the
/// database first, then -- when it holds no pull request, or `more` asks --
/// one GitHub search across the owners the user already works in, and one
/// GitLab search per project in the user's merge-request lists (#2040).
#[tauri::command]
pub async fn jira_find(
    app: AppHandle,
    key: String,
    more: bool,
) -> Result<super::lookup::KeyLookup, JiraError> {
    if !enabled(&app).await {
        return Err(JiraError::NotAsked);
    }
    if !super::cli::is_key(&key) {
        return Err(JiraError::failed(format!("{key} is not a Jira key")));
    }
    use tauri::Manager;
    let client = app.state::<crate::commands::GhClient>().0.clone();
    let glab = crate::gitlab::auth::find_glab();
    super::lookup::find(client, glab, &crate::commands::db_path(&app), &key, more)
        .await
        .map_err(JiraError::failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire spelling of every state, which `src/types/jira.ts` mirrors
    /// as `JiraError`. A rename here that is not made there breaks this.
    #[test]
    fn every_error_state_has_its_wire_spelling() {
        let j = |e: JiraError| serde_json::to_string(&e).unwrap();
        assert_eq!(j(JiraError::NotAsked), r#"{"state":"not_asked"}"#);
        assert_eq!(
            j(JiraError::NotInstalled {
                searched: vec!["d".into()]
            }),
            r#"{"state":"not_installed","searched":["d"]}"#
        );
        assert_eq!(j(JiraError::NotLoggedIn), r#"{"state":"not_logged_in"}"#);
        assert_eq!(
            j(JiraError::RateLimited {
                retry_after_secs: None
            }),
            r#"{"state":"rate_limited","retry_after_secs":null}"#
        );
        assert_eq!(
            j(JiraError::TimedOut { secs: 20 }),
            r#"{"state":"timed_out","secs":20}"#
        );
        assert_eq!(
            j(JiraError::failed("x")),
            r#"{"state":"failed","detail":"x"}"#
        );
    }

    /// "Not asked" reaches the phone as text, so its text must carry the
    /// marker the remote classifier and `src/lib/notAsked.ts` look for.
    #[test]
    fn not_asked_text_leads_with_the_marker() {
        assert!(JiraError::NotAsked
            .to_string()
            .starts_with(crate::commands::NOT_ASKED));
        for e in [
            JiraError::NotLoggedIn,
            JiraError::TimedOut { secs: 1 },
            JiraError::RateLimited {
                retry_after_secs: Some(3),
            },
        ] {
            assert!(!e.to_string().contains(crate::commands::NOT_ASKED), "{e}");
        }
    }

    #[test]
    fn off_never_looks_for_acli() {
        let got = gate(false, || panic!("located while switched off"));
        assert_eq!(got, Err(JiraError::NotAsked));
    }
}

/// Off spawns nothing (#2034): the same `*_with` functions the commands
/// call, against a stand-in `acli` that records every call it receives.
#[cfg(all(test, unix))]
mod off_spawns_nothing {
    use super::*;
    use crate::jira::cli::Acli;
    use std::os::unix::fs::PermissionsExt;

    fn recorder(dir: &Path) -> Transport {
        let bin = dir.join("acli");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh
echo \"$*\" >> '{}'
echo '[]'
",
                dir.join("calls").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        Transport::Acli(Acli::new(bin))
    }

    fn calls(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    async fn every_command(on: bool, dir: &Path) -> Vec<Option<JiraError>> {
        let db = dir.join("t.db");
        let keys = vec!["ABC-1".to_string()];
        let t = || gate(on, || Ok(recorder(dir)));
        let connection = connection_with(t()).await;
        assert_eq!(
            connection == JiraConnection::Disabled,
            !on,
            "{connection:?}"
        );
        vec![
            projects_with(t()).await.err(),
            issues_with(t(), &db, None, &keys).await.err(),
            assigned_with(t(), &db, None).await.err(),
        ]
    }

    #[tokio::test]
    async fn switched_off_every_command_is_not_asked_and_nothing_runs() {
        let dir = tempfile::tempdir().unwrap();
        let got = every_command(false, dir.path()).await;
        assert!(
            got.iter().all(|e| e == &Some(JiraError::NotAsked)),
            "{got:?}"
        );
        assert_eq!(calls(dir.path()), 0, "acli was spawned while Jira was off");
    }

    /// Proves the recorder records: the same calls with Jira on spawn it.
    /// Without this, a recorder that never wrote would pass the test above.
    #[tokio::test]
    async fn switched_on_the_same_commands_do_run() {
        let dir = tempfile::tempdir().unwrap();
        let got = every_command(true, dir.path()).await;
        assert!(got.iter().all(Option::is_none), "{got:?}");
        assert!(
            calls(dir.path()) >= 4,
            "connection, projects, issues, assigned"
        );
    }
}
