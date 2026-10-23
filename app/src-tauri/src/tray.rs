//! The system tray: closing the launcher window hides it there; a click on the icon brings it back (or hides it
//! again while it is open), the menu shows it or quits.
//!
//! Quitting goes through the main window (`quit-requested`), which asks first while a game runs and then calls
//! `app_quit`. Without a tray (no StatusNotifier host, say) the close button takes that same path instead.
//!
//! Linux talks StatusNotifierItem over D-Bus directly (`ksni`): Tauri's tray goes through appindicator there, which
//! never reports a left click and has no tooltip. Windows and macOS use Tauri's tray.

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, State, Window, WindowEvent};

const TITLE: &str = "Blockheads PC";

/// The tray handle (its menu texts follow the UI language) and whether the tray is up.
pub struct Tray {
    imp: Option<imp::Handle>,
    up: AtomicBool,
}

/// Puts the icon in the tray. Failing that the app still works; the close button then quits.
pub fn create(app: &AppHandle) {
    let built = imp::create(app);
    if let Err(e) = &built {
        eprintln!("tray icon failed: {e}");
    }
    let up = AtomicBool::new(built.is_ok());
    app.manage(Tray { imp: built.ok(), up });
}

/// Brings the launcher window back from the tray (or from behind other windows).
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// A left click on the icon: hides the launcher window while it is open, otherwise brings it back. Focus is no
/// test here, the click itself takes it from the window on Windows.
fn toggle_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    if w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false) {
        let _ = w.hide();
    } else {
        show_main(app);
    }
}

/// Quit, as the main window decides: it asks first while a game runs.
fn request_quit(app: &AppHandle) {
    show_main(app);
    let _ = app.emit_to("main", "quit-requested", ());
}

/// The launcher window's close button: hide to the tray, or without a tray ask the window to quit.
pub fn on_close(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else { return };
    if window.label() != "main" {
        return;
    }
    api.prevent_close();
    let app = window.app_handle();
    if app.try_state::<Tray>().is_some_and(|t| t.up.load(Ordering::SeqCst)) {
        let _ = window.hide();
    } else {
        request_quit(app);
    }
}

/// The menu texts in the UI language.
#[tauri::command]
pub fn tray_labels(tray: State<'_, Tray>, show: String, quit: String) {
    if let Some(h) = &tray.imp {
        imp::labels(h, show, quit);
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{request_quit, show_main, toggle_main, TITLE};
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;
    use ksni::{Category, Icon, MenuItem, ToolTip};
    use tauri::AppHandle;

    pub type Handle = ksni::blocking::Handle<Sni>;

    /// The StatusNotifierItem: a left click shows or hides the window, the menu shows it or quits.
    pub struct Sni {
        app: AppHandle,
        show: String,
        quit: String,
        icon: Vec<Icon>,
    }

    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            "blockheads-pc".into()
        }
        fn title(&self) -> String {
            TITLE.into()
        }
        fn category(&self) -> Category {
            Category::ApplicationStatus
        }
        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icon.clone()
        }
        fn tool_tip(&self) -> ToolTip {
            ToolTip { title: TITLE.into(), icon_pixmap: self.icon.clone(), ..Default::default() }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            toggle_main(&self.app);
        }
        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: self.show.clone(),
                    activate: Box::new(|t: &mut Sni| show_main(&t.app)),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: self.quit.clone(),
                    activate: Box::new(|t: &mut Sni| request_quit(&t.app)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    /// The window icon as StatusNotifierItem pixels: ARGB32, big-endian.
    fn argb(app: &AppHandle) -> Vec<Icon> {
        let Some(img) = app.default_window_icon() else { return Vec::new() };
        let data = img.rgba().chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
        vec![Icon { width: img.width() as i32, height: img.height() as i32, data }]
    }

    pub fn create(app: &AppHandle) -> Result<Handle, String> {
        let sni = Sni { app: app.clone(), show: format!("Show {TITLE}"), quit: "Quit".into(), icon: argb(app) };
        sni.spawn().map_err(|e| e.to_string())
    }

    pub fn labels(h: &Handle, show: String, quit: String) {
        h.update(|t| {
            t.show = show;
            t.quit = quit;
        });
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{request_quit, show_main, toggle_main, TITLE};
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{AppHandle, Wry};

    pub type Handle = (MenuItem<Wry>, MenuItem<Wry>);

    pub fn create(app: &AppHandle) -> tauri::Result<Handle> {
        let show = MenuItem::with_id(app, "show", format!("Show {TITLE}"), true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&show, &quit])?;
        let mut tray = TrayIconBuilder::with_id("main")
            .menu(&menu)
            .tooltip(TITLE)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, e| match e.id().as_ref() {
                "show" => show_main(app),
                "quit" => request_quit(app),
                _ => {}
            })
            .on_tray_icon_event(|tray, e| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    toggle_main(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        tray.build(app)?;
        Ok((show, quit))
    }

    pub fn labels(h: &Handle, show: String, quit: String) {
        let _ = h.0.set_text(show);
        let _ = h.1.set_text(quit);
    }
}
