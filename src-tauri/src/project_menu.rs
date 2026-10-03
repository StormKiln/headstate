//! Extend Tauri's standard menus without losing native editing/window items.
#[cfg(not(target_os = "macos"))]
use tauri::menu::{AboutMetadata, PredefinedMenuItem};
use tauri::menu::{Menu, MenuItem, MenuItemKind};
#[cfg(target_os = "linux")]
use tauri::{menu::WINDOW_SUBMENU_ID, Manager};
use tauri_plugin_opener::OpenerExt;

#[cfg(not(target_os = "macos"))]
const REPOSITORY: &str = "https://github.com/StormKiln/headstate";
const ISSUES: &str = "https://github.com/StormKiln/headstate/issues";

#[cfg(any(target_os = "linux", test))]
const LINUX_WINDOW_ITEMS: [(&str, &str); 3] = [
    ("project-window-minimize", "Minimize"),
    ("project-window-maximize", "Maximize / Restore"),
    ("project-window-close", "Close"),
];

#[cfg(any(target_os = "linux", test))]
fn linux_window_items() -> [(&'static str, &'static str); 3] {
    LINUX_WINDOW_ITEMS
}

#[cfg(any(target_os = "linux", test))]
trait WindowActions {
    fn minimize(&mut self) -> tauri::Result<()>;
    fn is_maximized(&self) -> tauri::Result<bool>;
    fn maximize(&mut self) -> tauri::Result<()>;
    fn unmaximize(&mut self) -> tauri::Result<()>;
    fn close(&mut self) -> tauri::Result<()>;
}

#[cfg(any(target_os = "linux", test))]
fn dispatch_window_action(id: &str, window: &mut impl WindowActions) -> tauri::Result<bool> {
    match id {
        "project-window-minimize" => window.minimize()?,
        "project-window-maximize" => {
            if window.is_maximized()? {
                window.unmaximize()?;
            } else {
                window.maximize()?;
            }
        }
        "project-window-close" => window.close()?,
        _ => return Ok(false),
    }
    Ok(true)
}

#[cfg(target_os = "linux")]
impl WindowActions for tauri::WebviewWindow {
    fn minimize(&mut self) -> tauri::Result<()> {
        tauri::WebviewWindow::minimize(self)
    }

    fn is_maximized(&self) -> tauri::Result<bool> {
        tauri::WebviewWindow::is_maximized(self)
    }

    fn maximize(&mut self) -> tauri::Result<()> {
        tauri::WebviewWindow::maximize(self)
    }

    fn unmaximize(&mut self) -> tauri::Result<()> {
        tauri::WebviewWindow::unmaximize(self)
    }

    fn close(&mut self) -> tauri::Result<()> {
        tauri::WebviewWindow::close(self)
    }
}

pub fn setup(app: &tauri::AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    for item in menu.items()? {
        if let MenuItemKind::Submenu(submenu) = item {
            #[cfg(target_os = "linux")]
            if submenu.id().as_ref() == WINDOW_SUBMENU_ID {
                for child in submenu.items()? {
                    submenu.remove(&child)?;
                }
                for (id, label) in linux_window_items() {
                    submenu.append(&MenuItem::with_id(app, id, label, true, None::<&str>)?)?;
                }
            }
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
        #[cfg(target_os = "linux")]
        id if id.starts_with("project-window-") => {
            if let Some(mut window) = app.get_webview_window("main") {
                if let Err(error) = dispatch_window_action(id, &mut window) {
                    log::warn!("could not perform Window menu action: {error}");
                }
            }
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

    #[derive(Default)]
    struct FakeWindow {
        maximized: bool,
        calls: Vec<&'static str>,
    }

    impl WindowActions for FakeWindow {
        fn minimize(&mut self) -> tauri::Result<()> {
            self.calls.push("minimize");
            Ok(())
        }

        fn is_maximized(&self) -> tauri::Result<bool> {
            Ok(self.maximized)
        }

        fn maximize(&mut self) -> tauri::Result<()> {
            self.calls.push("maximize");
            self.maximized = true;
            Ok(())
        }

        fn unmaximize(&mut self) -> tauri::Result<()> {
            self.calls.push("unmaximize");
            self.maximized = false;
            Ok(())
        }

        fn close(&mut self) -> tauri::Result<()> {
            self.calls.push("close");
            Ok(())
        }
    }

    #[test]
    fn linux_window_menu_uses_supported_ordinary_items() {
        assert_eq!(
            linux_window_items(),
            [
                ("project-window-minimize", "Minimize"),
                ("project-window-maximize", "Maximize / Restore"),
                ("project-window-close", "Close"),
            ]
        );
    }

    #[test]
    fn window_actions_minimize_toggle_and_close_the_existing_window() {
        let mut window = FakeWindow::default();
        for id in [
            "project-window-minimize",
            "project-window-maximize",
            "project-window-maximize",
            "project-window-close",
        ] {
            assert!(dispatch_window_action(id, &mut window).unwrap());
        }
        assert_eq!(
            window.calls,
            ["minimize", "maximize", "unmaximize", "close"]
        );
        assert!(!dispatch_window_action("project-issues", &mut window).unwrap());
    }
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

#[cfg(all(test, target_os = "linux"))]
mod linux_native_tests {
    use gtk::prelude::*;
    use muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    fn native_window_children(items: &[&dyn muda::IsMenuItem]) -> Vec<String> {
        gtk::init().expect("GTK test requires Xvfb");
        let submenu = Submenu::with_id_and_items("window", "Window", true, items).unwrap();
        let menu = Menu::with_items(&[&submenu]).unwrap();
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&container);
        menu.init_for_gtk_window(&window, Some(&container)).unwrap();
        // This Muda accessor takes `self` by value. Keep the original owner
        // alive: dropping the last Menu immediately destroys its GTK items,
        // which would make the native menubar look empty before inspection.
        let menubar = menu.clone().gtk_menubar_for_gtk_window(&window).unwrap();
        let root = menubar
            .children()
            .remove(0)
            .downcast::<gtk::MenuItem>()
            .unwrap();
        let popup = root.submenu().unwrap().downcast::<gtk::Menu>().unwrap();
        popup
            .children()
            .into_iter()
            .map(|widget| {
                widget
                    .downcast::<gtk::MenuItem>()
                    .unwrap()
                    .label()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn gtk_drops_predefined_window_actions_but_renders_ordinary_actions() {
        let minimize = PredefinedMenuItem::minimize(None);
        let maximize = PredefinedMenuItem::maximize(None);
        let close = PredefinedMenuItem::close_window(None);
        assert!(native_window_children(&[&minimize, &maximize, &close]).is_empty());

        let ordinary: Vec<MenuItem> = super::linux_window_items()
            .into_iter()
            .map(|(id, label)| MenuItem::with_id(id, label, true, None))
            .collect();
        let refs: Vec<&dyn muda::IsMenuItem> = ordinary
            .iter()
            .map(|item| item as &dyn muda::IsMenuItem)
            .collect();
        assert_eq!(
            native_window_children(&refs),
            ["Minimize", "Maximize / Restore", "Close"]
        );
    }
}
