//! Extend Tauri's standard menus without losing native editing/window items.
#[cfg(not(target_os = "macos"))]
use tauri::menu::{AboutMetadata, PredefinedMenuItem};
use tauri::menu::{Menu, MenuItem, MenuItemKind};
use tauri_plugin_opener::OpenerExt;

#[cfg(not(target_os = "macos"))]
const REPOSITORY: &str = "https://github.com/StormKiln/headstate";
const ISSUES: &str = "https://github.com/StormKiln/headstate/issues";

pub fn setup(app: &tauri::AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    for item in menu.items()? {
        if let MenuItemKind::Submenu(submenu) = item {
            if submenu.id().as_ref() == "__tauri_help_menu__" {
                submenu.append(&MenuItem::with_id(
                    app,
                    "project-issues",
                    "Report a Bug or Request a Feature…",
                    true,
                    None::<&str>,
                )?)?;
            }
            // The standard About item is first in the app menu on macOS,
            // and first in Help on other platforms.
            for child in submenu.items()? {
                if let MenuItemKind::Predefined(predefined) = &child {
                    if is_about_label(&predefined.text()?) {
                        submenu.remove(&child)?;
                        #[cfg(target_os = "macos")]
                        submenu.insert(
                            &MenuItem::with_id(
                                app,
                                "project-about",
                                "About Headstate",
                                true,
                                None::<&str>,
                            )?,
                            0,
                        )?;
                        #[cfg(not(target_os = "macos"))]
                        submenu.insert(
                            &PredefinedMenuItem::about(app, None, Some(about_metadata(app)))?,
                            0,
                        )?;
                        break;
                    }
                }
            }
        }
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| match event.id().as_ref() {
        "project-issues" => {
            if let Err(error) = app.opener().open_url(ISSUES, None::<&str>) {
                log::warn!("could not open project issues: {error}");
            }
        }
        #[cfg(target_os = "macos")]
        "project-about" => {
            let _ = app.run_on_main_thread(|| unsafe {
                crate::notification_navigation::headstate_about()
            });
        }
        _ => {}
    });
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn about_metadata(app: &tauri::AppHandle) -> AboutMetadata<'static> {
    AboutMetadata {
        name: Some("Headstate".into()),
        version: Some(app.package_info().version.to_string()),
        website: Some(REPOSITORY.into()),
        website_label: Some("Headstate on GitHub".into()),
        ..Default::default()
    }
}

// Windows retains the accelerator marker; Linux may normalize it to &.
fn is_about_label(label: &str) -> bool {
    label.trim_start_matches(['&', '_']).starts_with("About")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_native_about_mnemonics() {
        for label in ["About", "About Headstate", "&About", "_About"] {
            assert!(is_about_label(label), "{label}");
        }
        for label in ["Help", "Services", "Hide Headstate"] {
            assert!(!is_about_label(label), "{label}");
        }
    }
}
