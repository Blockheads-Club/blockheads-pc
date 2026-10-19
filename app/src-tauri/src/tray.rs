//! The system tray: closing the launcher window hides it there; the tray brings it back or quits.
//!
//! Quitting goes through the main window (`quit-requested`), which asks first while a game runs and then calls
//! `app_quit`. Without a tray (no StatusNotifier host, say) the close button takes that same path instead.

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, Window, WindowEvent, Wry};

/// The tray's menu entries (their texts follow the UI language) and whether the tray is up.
pub struct Tray {
    items: Option<(MenuItem<Wry>, MenuItem<Wry>)>,
    up: AtomicBool,
}

/// Puts the icon in the tray. Failing that the app still works; the close button then quits.
pub fn create(app: &AppHandle) {
    let built = (|| -> tauri::Result<_> {
        let show = MenuItem::with_id(app, "show", "Show Blockheads PC", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&show, &quit])?;
        let mut tray = TrayIconBuilder::with_id("main")
            .menu(&menu)
            .tooltip("Blockheads PC")
            .show_menu_on_left_click(false)
            .on_menu_event(|app, e| match e.id().as_ref() {
                "show" => show_main(app),
                "quit" => request_quit(app),
                _ => {}
            })
            .on_tray_icon_event(|tray, e| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    show_main(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        tray.build(app)?;
        Ok((show, quit))
    })();
    if let Err(e) = &built {
        eprintln!("tray icon failed: {e}");
    }
    let up = AtomicBool::new(built.is_ok());
    app.manage(Tray { items: built.ok(), up });
}

/// Brings the launcher window back from the tray (or from behind other windows).
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
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
    if let Some((s, q)) = &tray.items {
        let _ = s.set_text(show);
        let _ = q.set_text(quit);
    }
}
