//! The game inside the launcher window (Linux): a GTK drawing area laid over the webview (a `GtkOverlay` in the window's box).
//!
//! The game runs as `bhpc-run --embed FILE` (crash isolation, see `docs/launcher.md`, "Playing in the launcher
//! window"). It renders offscreen and publishes frames into a frame file; this widget paints the newest one with cairo
//! once per display frame and sends mouse, keyboard, text, wheel, focus and size to the runner as text lines
//! (`bhpc_launcher::embed::Cmd`). The webview keeps the whole window with the title bar and the control strip; the
//! widget is an overlay over the part below them, at the height the page reports (`layout`). All GTK work happens on the main thread.
//!
//! Other platforms have no view yet: `supported()` is false and the launcher starts the game in a window of its own.

/// True when this build can show the game inside the window.
pub fn supported() -> bool {
    imp::SUPPORTED && std::env::var("BHPC_EMBED").map_or(true, |v| v != "0")
}

pub use imp::{close, layout, open, set_fullscreen, Opts};

#[cfg(target_os = "linux")]
mod imp {
    use bhpc_input::{Input, Trigger};
    use bhpc_launcher::embed::{hid_from_evdev, Cmd, FrameReader, MOD_ALT, MOD_CTRL, MOD_SHIFT};
    use gtk::cairo::{Format, ImageSurface};
    use gtk::gdk::{self, EventMask, ModifierType, ScrollDirection};
    use gtk::glib::{self, Propagation};
    use gtk::prelude::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;
    use std::rc::Rc;
    use std::sync::mpsc::{channel, Sender};
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Emitter, WebviewWindow};

    pub const SUPPORTED: bool = true;

    /// How long the game fades in over the loading screen.
    const FADE: Duration = Duration::from_millis(240);

    /// What the view needs from the launcher.
    pub struct Opts {
        pub frame: FrameReader,
        /// Lines for the runner's stdin.
        pub input: Sender<String>,
        /// The fullscreen binding: the launcher handles it itself (the window is its own).
        pub fullscreen_key: Trigger,
        pub app: AppHandle,
    }

    struct View {
        area: gtk::DrawingArea,
        webview: gtk::Widget,
        vbox: gtk::Box,
        /// Holds the webview with the game view laid over its stage.
        overlay: gtk::Overlay,
        paint: Rc<RefCell<Paint>>,
    }

    struct Paint {
        /// When the first game frame arrived: before it the view draws nothing (the page's loading screen shows
        /// through and gets the mouse), then it fades in over [`FADE`].
        first: Option<Instant>,
        reader: Option<FrameReader>,
        buf: Vec<u8>,
        surface: Option<ImageSurface>,
    }

    thread_local! {
        static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
    }

    /// Runs `f` on the GTK main thread and returns its result.
    fn on_main<T: Send + 'static>(
        win: &WebviewWindow,
        f: impl FnOnce(&WebviewWindow) -> T + Send + 'static,
    ) -> Result<T, String> {
        if gtk::is_initialized_main_thread() {
            return Ok(f(win));
        }
        let (tx, rx) = channel();
        let w = win.clone();
        win.run_on_main_thread(move || {
            let _ = tx.send(f(&w));
        })
        .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())
    }

    fn send(tx: &Sender<String>, cmd: Cmd) {
        let _ = tx.send(cmd.encode());
    }

    fn mods_of(state: ModifierType) -> u8 {
        let bit = |m: ModifierType, b: u8| if state.contains(m) { b } else { 0 };
        bit(ModifierType::CONTROL_MASK, MOD_CTRL)
            | bit(ModifierType::SHIFT_MASK, MOD_SHIFT)
            | bit(ModifierType::MOD1_MASK, MOD_ALT)
    }

    /// Shows the game view under the webview. The webview keeps its full size until `layout` is called.
    pub fn open(win: &WebviewWindow, opts: Opts) -> Result<(), String> {
        on_main(win, move |win| {
            close_now();
            let vbox = win.default_vbox().map_err(|e| e.to_string())?;
            let area = gtk::DrawingArea::new();
            let webview = vbox
                .children()
                .into_iter()
                .find(|c| c.type_().name().contains("WebView"))
                .ok_or("the webview is not in the window box")?;
            area.set_can_focus(true);
            area.set_hexpand(true);
            area.set_vexpand(true);
            area.add_events(
                EventMask::POINTER_MOTION_MASK
                    | EventMask::BUTTON_PRESS_MASK
                    | EventMask::BUTTON_RELEASE_MASK
                    | EventMask::SCROLL_MASK
                    | EventMask::SMOOTH_SCROLL_MASK
                    | EventMask::KEY_PRESS_MASK
                    | EventMask::KEY_RELEASE_MASK
                    | EventMask::FOCUS_CHANGE_MASK,
            );
            let paint =
                Rc::new(RefCell::new(Paint { first: None, reader: Some(opts.frame), buf: Vec::new(), surface: None }));
            wire_paint(&area, &paint);
            wire_input(&area, opts.input, opts.fullscreen_key, win.clone(), opts.app);
            area.set_halign(gtk::Align::Fill);
            area.set_valign(gtk::Align::Fill);
            // The webview keeps the whole window; the game view is laid over the part of it below the bars.
            let overlay = gtk::Overlay::new();
            vbox.remove(&webview);
            overlay.add(&webview);
            overlay.add_overlay(&area);
            // Until the first frame the page shows its loading screen and must get the mouse (Cancel).
            overlay.set_overlay_pass_through(&area, true);
            vbox.pack_start(&overlay, true, true, 0);
            overlay.show();
            // Hidden until the page has reported where the stage starts (`layout`): the runner sizes its first frame
            // by the first size it hears, which must be the real one.
            VIEW.with(|v| *v.borrow_mut() = Some(View { area, webview, vbox, overlay, paint }));
            Ok(())
        })?
    }

    fn wire_paint(area: &gtk::DrawingArea, paint: &Rc<RefCell<Paint>>) {
        let p = paint.clone();
        area.add_tick_callback(move |w, _| {
            let mut p = p.borrow_mut();
            let Paint { first, reader, buf, surface } = &mut *p;
            if let Some(info) = reader.as_mut().and_then(|r| r.read(buf).ok().flatten()) {
                let (iw, ih) = (info.width as i32, info.height as i32);
                let fits = surface.as_ref().is_some_and(|s| s.width() == iw && s.height() == ih);
                if !fits {
                    *surface = ImageSurface::create(Format::ARgb32, iw, ih).ok();
                    if let Some(s) = surface.as_ref() {
                        s.set_device_scale(f64::from(w.scale_factor()), f64::from(w.scale_factor()));
                    }
                }
                if let Some(Ok(mut data)) = surface.as_mut().map(|s| s.data()) {
                    if data.len() == buf.len() {
                        data.copy_from_slice(buf);
                    }
                }
                if first.is_none() {
                    *first = Some(Instant::now());
                    if let Some(o) = w.parent().and_then(|p| p.downcast::<gtk::Overlay>().ok()) {
                        o.set_overlay_pass_through(w, false);
                    }
                    w.grab_focus();
                }
                w.queue_draw();
            } else if first.is_some_and(|f| f.elapsed() < FADE) {
                w.queue_draw();
            }
            glib::ControlFlow::Continue
        });
        let p = paint.clone();
        area.connect_draw(move |_, cr| {
            let p = p.borrow();
            let Some(first) = p.first else { return Propagation::Stop };
            // Without animations (the desktop's setting) the game appears at once.
            let animate = gtk::Settings::default().is_some_and(|s| s.is_gtk_enable_animations());
            let alpha = if animate { (first.elapsed().as_secs_f64() / FADE.as_secs_f64()).min(1.0) } else { 1.0 };
            cr.set_source_rgba(0.0, 0.0, 0.0, alpha);
            let _ = cr.paint();
            if let Some(s) = p.surface.as_ref() {
                let _ = cr.set_source_surface(s, 0.0, 0.0);
                let _ = cr.paint_with_alpha(alpha);
            }
            Propagation::Stop
        });
    }

    fn wire_input(area: &gtk::DrawingArea, tx: Sender<String>, fs: Trigger, win: WebviewWindow, app: AppHandle) {
        let scale = |w: &gtk::DrawingArea| f64::from(w.scale_factor());
        // The size settles over a few allocations (window resize, webview shrinking): tell the runner the last one.
        let t = tx.clone();
        let latest: Rc<Cell<(u32, u32)>> = Rc::default();
        let pending: Rc<Cell<bool>> = Rc::default();
        area.connect_size_allocate(move |w, a| {
            let s = w.scale_factor();
            latest.set(((a.width() * s).max(1) as u32, (a.height() * s).max(1) as u32));
            if !pending.replace(true) {
                let (t, latest, pending) = (t.clone(), latest.clone(), pending.clone());
                glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                    pending.set(false);
                    let (width, height) = latest.get();
                    send(&t, Cmd::Size { width, height });
                });
            }
        });
        let t = tx.clone();
        area.connect_motion_notify_event(move |w, ev| {
            let (x, y) = ev.position();
            send(&t, Cmd::Move { x: (x * scale(w)) as f32, y: (y * scale(w)) as f32 });
            Propagation::Stop
        });
        let t = tx.clone();
        area.connect_button_press_event(move |w, ev| {
            w.grab_focus();
            button(&t, w, ev, true)
        });
        let t = tx.clone();
        area.connect_button_release_event(move |w, ev| button(&t, w, ev, false));
        let t = tx.clone();
        area.connect_scroll_event(move |w, ev| {
            let (x, y) = ev.position();
            let (dx, dy) = match ev.direction() {
                ScrollDirection::Up => (0.0, -1.0),
                ScrollDirection::Down => (0.0, 1.0),
                ScrollDirection::Left => (-1.0, 0.0),
                ScrollDirection::Right => (1.0, 0.0),
                ScrollDirection::Smooth => ev.delta(),
                _ => (0.0, 0.0),
            };
            // GTK counts down as positive, the mapper counts away from the user (up) as positive.
            send(
                &t,
                Cmd::Wheel {
                    dx: -dx as f32,
                    dy: -dy as f32,
                    x: (x * scale(w)) as f32,
                    y: (y * scale(w)) as f32,
                    mods: mods_of(ev.state()),
                },
            );
            Propagation::Stop
        });

        let im = gtk::IMMulticontext::new();
        let t = tx.clone();
        im.connect_commit(move |_, text| send(&t, Cmd::Text(text.to_string())));
        let imc = im.clone();
        area.connect_realize(move |w| imc.set_client_window(w.window().as_ref()));
        let (t, imc) = (tx.clone(), im.clone());
        area.connect_focus_in_event(move |_, _| {
            imc.focus_in();
            send(&t, Cmd::Focus(true));
            Propagation::Proceed
        });
        let (t, imc) = (tx.clone(), im.clone());
        area.connect_focus_out_event(move |_, _| {
            imc.focus_out();
            send(&t, Cmd::Focus(false));
            Propagation::Proceed
        });

        let held: Rc<RefCell<HashSet<u16>>> = Rc::default();
        let swallowed: Rc<RefCell<HashSet<u16>>> = Rc::default();
        let (t, h, sw) = (tx.clone(), held.clone(), swallowed.clone());
        let imc = im.clone();
        area.connect_key_press_event(move |_, ev| {
            let code = ev.hardware_keycode();
            let Some(hid) = hid_from_evdev(u32::from(code).saturating_sub(8)) else {
                return Propagation::Stop;
            };
            let state = ev.state();
            let mods = bhpc_input::Mods {
                ctrl: state.contains(ModifierType::CONTROL_MASK),
                shift: state.contains(ModifierType::SHIFT_MASK),
                alt: state.contains(ModifierType::MOD1_MASK),
            };
            let repeat = !h.borrow_mut().insert(code);
            if fs.fires(Input::Key(bhpc_input::keymap::host_key_from_hid(hid)), mods) {
                sw.borrow_mut().insert(code);
                if !repeat {
                    toggle_fullscreen(&win, &app);
                }
                return Propagation::Stop;
            }
            send(&t, Cmd::Key { hid, pressed: true, repeat, mods: mods_of(state) });
            if imc.filter_keypress(ev) {
                return Propagation::Stop;
            }
            // The input method did not take the key: a printable one is text.
            if let Some(c) = ev.keyval().to_unicode().filter(|c| !c.is_control() && !mods.ctrl && !mods.alt) {
                send(&t, Cmd::Text(c.to_string()));
            }
            Propagation::Stop
        });
        let t = tx;
        area.connect_key_release_event(move |_, ev| {
            let code = ev.hardware_keycode();
            held.borrow_mut().remove(&code);
            if swallowed.borrow_mut().remove(&code) {
                return Propagation::Stop;
            }
            if let Some(hid) = hid_from_evdev(u32::from(code).saturating_sub(8)) {
                send(&t, Cmd::Key { hid, pressed: false, repeat: false, mods: mods_of(ev.state()) });
            }
            Propagation::Stop
        });
    }

    fn button(tx: &Sender<String>, w: &gtk::DrawingArea, ev: &gdk::EventButton, pressed: bool) -> Propagation {
        // Double and triple clicks arrive as extra events after the plain press.
        if pressed && ev.event_type() != gdk::EventType::ButtonPress {
            return Propagation::Stop;
        }
        let (x, y) = ev.position();
        let s = f64::from(w.scale_factor());
        let b = match ev.button() {
            1 => 0,
            2 => 1,
            3 => 2,
            n => n as u8,
        };
        send(tx, Cmd::Button { button: b, pressed, x: (x * s) as f32, y: (y * s) as f32, mods: mods_of(ev.state()) });
        Propagation::Stop
    }

    fn toggle_fullscreen(win: &WebviewWindow, app: &AppHandle) {
        let on = !win.is_fullscreen().unwrap_or(false);
        set_fullscreen_now(win, app, on);
    }

    fn set_fullscreen_now(win: &WebviewWindow, app: &AppHandle, on: bool) {
        if win.set_fullscreen(on).is_ok() {
            let _ = app.emit_to("main", "embed-fullscreen", on);
        }
    }

    /// Switch the window to or from fullscreen and tell the page.
    pub fn set_fullscreen(win: &WebviewWindow, app: &AppHandle, on: bool) -> Result<(), String> {
        let app = app.clone();
        on_main(win, move |w| set_fullscreen_now(w, &app, on))
    }

    /// Where the stage starts: the game view covers the window below `top` pixels (`0`: all of it, fullscreen); a
    /// negative `top` hides it so that the page (a dialog) is visible.
    pub fn layout(win: &WebviewWindow, top: i32) -> Result<(), String> {
        on_main(win, move |_| {
            VIEW.with(|v| {
                let v = v.borrow();
                let Some(v) = v.as_ref() else { return };
                if top < 0 {
                    v.area.hide();
                    return;
                }
                v.area.set_margin_top(top);
                if !v.area.is_visible() {
                    v.area.show();
                }
                // Before the first frame the page's loading screen keeps the keyboard (Esc cancels).
                if v.paint.borrow().first.is_some() && !v.area.has_focus() {
                    v.area.grab_focus();
                }
            });
        })
    }

    fn close_now() {
        VIEW.with(|v| {
            if let Some(v) = v.borrow_mut().take() {
                v.overlay.remove(&v.webview);
                v.overlay.remove(&v.area);
                v.vbox.remove(&v.overlay);
                v.vbox.pack_start(&v.webview, true, true, 0);
                v.webview.show();
                if let Some(r) = v.paint.borrow_mut().reader.take() {
                    r.remove();
                }
            }
        });
    }

    /// Takes the game view away and gives the webview the whole window again.
    pub fn close(win: &WebviewWindow) -> Result<(), String> {
        on_main(win, |_| close_now())
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use bhpc_input::Trigger;
    use bhpc_launcher::embed::FrameReader;
    use std::sync::mpsc::Sender;
    use tauri::{AppHandle, WebviewWindow};

    pub const SUPPORTED: bool = false;

    pub struct Opts {
        pub frame: FrameReader,
        pub input: Sender<String>,
        pub fullscreen_key: Trigger,
        pub app: AppHandle,
    }

    pub fn open(_: &WebviewWindow, _: Opts) -> Result<(), String> {
        Err("the game view is not available on this platform yet".into())
    }
    pub fn layout(_: &WebviewWindow, _: i32) -> Result<(), String> {
        Ok(())
    }
    pub fn set_fullscreen(win: &WebviewWindow, _: &AppHandle, on: bool) -> Result<(), String> {
        win.set_fullscreen(on).map_err(|e| e.to_string())
    }
    pub fn close(_: &WebviewWindow) -> Result<(), String> {
        Ok(())
    }
}
