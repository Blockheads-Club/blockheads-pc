//! The game inside the launcher window (Linux): a GTK drawing area laid over the webview (a `GtkOverlay` in the window's box).
//!
//! The game runs as `bhpc-run --embed FILE` (crash isolation, see `docs/launcher.md`, "Playing in the launcher
//! window"). It renders offscreen and publishes frames into a frame file; the newest one is shown once per display
//! frame, by a `GtkGLArea` under the input widget when GL works (`embed_gl`; off with `BHPC_EMBED_GL=0`), else painted
//! with cairo by the input widget itself and sends mouse, keyboard, text, wheel, focus and size to the runner as text lines
//! (`bhpc_launcher::embed::Cmd`). The webview keeps the whole window with the title bar and the control strip; the
//! widget is an overlay over the part below them, at the height the page reports (`layout`). All GTK work happens on the main thread.
//!
//! Other platforms have no view yet: `supported()` is false and the launcher starts the game in a window of its own.

/// True when this build can show the game inside the window.
pub fn supported() -> bool {
    imp::SUPPORTED && std::env::var("BHPC_EMBED").map_or(true, |v| v != "0")
}

pub use imp::{close, layout, open, prepare, reveal, set_fullscreen, Opts};

/// How close (logical pixels) to the window's side or bottom edge a press on the game view resizes the window.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const RESIZE_BORDER: f64 = 6.0;

/// The edge of a `width` x `height` game view (which spans the window's width and reaches its bottom) that
/// `(x, y)` is within `border` of: the sides, the bottom, or a bottom corner.
#[cfg(target_os = "linux")]
fn resize_edge(x: f64, y: f64, width: f64, height: f64, border: f64) -> Option<gtk::gdk::WindowEdge> {
    use gtk::gdk::WindowEdge as E;
    let (left, right, bottom) = (x < border, x >= width - border, y >= height - border);
    match (left, right, bottom) {
        (true, _, true) => Some(E::SouthWest),
        (_, true, true) => Some(E::SouthEast),
        (true, _, _) => Some(E::West),
        (_, true, _) => Some(E::East),
        (_, _, true) => Some(E::South),
        _ => None,
    }
}

/// Ctrl+V (by the letter the layout puts on the key) or Shift+Insert: the clipboard text is typed into the game (its
/// focused text field; without one the runner ignores text).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_paste(letter: Option<char>, insert: bool, ctrl: bool, shift: bool, alt: bool) -> bool {
    !alt && ((ctrl && letter.is_some_and(|c| c.eq_ignore_ascii_case(&'v'))) || (insert && shift && !ctrl))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{is_paste, resize_edge};

    #[test]
    fn paste_keys() {
        assert!(is_paste(Some('v'), false, true, false, false));
        assert!(is_paste(Some('V'), false, true, true, false));
        assert!(is_paste(None, true, false, true, false));
        assert!(!is_paste(Some('v'), false, false, false, false));
        assert!(!is_paste(Some('v'), false, true, false, true), "AltGr/Alt combinations are not paste");
        assert!(!is_paste(Some('c'), false, true, false, false));
        assert!(!is_paste(None, true, true, true, false));
        assert!(!is_paste(None, true, false, false, false));
    }
    use gtk::gdk::WindowEdge as E;

    #[test]
    fn edges_of_the_game_view() {
        let at = |x, y| resize_edge(x, y, 800.0, 600.0, 6.0);
        assert_eq!(at(400.0, 300.0), None);
        assert_eq!(at(400.0, 0.0), None, "the top is the title strip's");
        assert_eq!(at(2.0, 300.0), Some(E::West));
        assert_eq!(at(797.0, 300.0), Some(E::East));
        assert_eq!(at(400.0, 596.0), Some(E::South));
        assert_eq!(at(1.0, 599.0), Some(E::SouthWest));
        assert_eq!(at(799.0, 599.0), Some(E::SouthEast));
        assert_eq!(at(6.0, 593.9), None);
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use crate::embed_gl::{Dialect, GlPainter};
    use bhpc_input::{Input, Trigger};
    use bhpc_launcher::embed::{hid_from_evdev, Cmd, FrameReader, Layout, MOD_ALT, MOD_CTRL, MOD_SHIFT};
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
    const FADE: Duration = Duration::from_millis(150);
    /// If the page never says it is ready to hand over (`reveal`), the game shows this long after its first frame.
    const REVEAL_FALLBACK: Duration = Duration::from_secs(3);

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
        /// Holds the webview with the game view laid over its stage.
        overlay: gtk::Overlay,
        /// The GL picture under `area` (when GL is on).
        gl: Option<gtk::GLArea>,
        paint: Rc<RefCell<Paint>>,
    }

    struct Paint {
        /// When the game became visible: once its first frame arrived and the page let it (`reveal`, after its
        /// loading screen reached 100 %). Before it the view draws nothing (the loading screen shows through and gets
        /// the mouse), then it fades in over [`FADE`].
        first: Option<Instant>,
        /// When the first game frame arrived.
        arrived: Option<Instant>,
        /// The page is done with its loading screen (or [`REVEAL_FALLBACK`] passed).
        revealed: bool,
        reader: Option<FrameReader>,
        buf: Vec<u8>,
        surface: Option<ImageSurface>,
        /// The GL painter once its area is realized; `None` paints with cairo.
        gl: Option<GlPainter>,
        /// Sequence number of the newest frame the GL path has seen announced.
        gl_seq: u64,
        stats: Option<Stats>,
    }

    /// `BHPC_EMBED_STATS=1`: time spent showing frames, printed every 10 seconds.
    struct Stats {
        frames: u32,
        busy: Duration,
        since: Instant,
    }

    impl Stats {
        fn add(stats: &mut Option<Stats>, began: Instant, how: &str) {
            let Some(s) = stats.as_mut() else { return };
            s.frames += 1;
            s.busy += began.elapsed();
            let secs = s.since.elapsed().as_secs_f64();
            if secs >= 10.0 {
                let ms = s.busy.as_secs_f64() * 1000.0 / f64::from(s.frames.max(1));
                eprintln!(
                    "[embed] view: {how}, {:.1} frames/s shown, {ms:.2} ms per frame",
                    f64::from(s.frames) / secs
                );
                *s = Stats { frames: 0, busy: Duration::ZERO, since: Instant::now() };
            }
        }
    }

    thread_local! {
        static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
        /// The overlay that holds the webview, made once and kept: moving the webview in and out of it on every
        /// start made WebKit repaint from black (a black flash on the loading screen).
        static HOST: RefCell<Option<gtk::Overlay>> = const { RefCell::new(None) };
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
            let overlay = host_overlay(win)?;
            let area = gtk::DrawingArea::new();
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
            let stats = std::env::var("BHPC_EMBED_STATS").is_ok_and(|v| v == "1");
            let paint = Rc::new(RefCell::new(Paint {
                first: None,
                arrived: None,
                revealed: false,
                reader: Some(opts.frame),
                buf: Vec::new(),
                surface: None,
                gl: None,
                gl_seq: 0,
                stats: stats.then(|| Stats { frames: 0, busy: Duration::ZERO, since: Instant::now() }),
            }));
            let gl = std::env::var("BHPC_EMBED_GL").map_or(true, |v| v != "0").then(|| gl_area(&paint, &opts.input));
            wire_paint(&area, &paint, gl.clone());
            wire_input(&area, opts.input, opts.fullscreen_key, win.clone(), opts.app);
            area.set_halign(gtk::Align::Fill);
            area.set_valign(gtk::Align::Fill);
            // The webview keeps the whole window; the game view is laid over the part of it below the bars.
            if let Some(g) = &gl {
                overlay.add_overlay(g);
                // The picture never takes input (the input widget above it does).
                pass_through(g, true);
                g.connect_realize(|g| pass_through(g, true));
                // Invisible until the game is revealed: GTK may show a GL area black before its first render.
                g.set_opacity(0.0);
            }
            overlay.add_overlay(&area);
            // Until the first frame the page shows its loading screen and must get the mouse (Cancel).
            pass_through(&area, true);
            area.connect_realize(|w| {
                let p = w.parent().and_then(|o| o.downcast::<gtk::Overlay>().ok());
                pass_through(w, p.is_some_and(|o| o.is_overlay_pass_through(w)));
            });
            // Hidden until the page has reported where the stage starts (`layout`): the runner sizes its first frame
            // by the first size it hears, which must be the real one.
            VIEW.with(|v| *v.borrow_mut() = Some(View { area, overlay, gl, paint }));
            Ok(())
        })?
    }

    /// The GL picture: realized with a context of its own, it asks the runner for frames as read back and draws them
    /// on `render`. Any GL failure hands the picture back to cairo (and the runner back to the cairo layout).
    fn gl_area(paint: &Rc<RefCell<Paint>>, input: &Sender<String>) -> gtk::GLArea {
        let g = gtk::GLArea::new();
        g.set_has_alpha(true);
        g.set_auto_render(false);
        g.set_hexpand(true);
        g.set_vexpand(true);
        g.set_halign(gtk::Align::Fill);
        g.set_valign(gtk::Align::Fill);
        let (p, tx) = (paint.clone(), input.clone());
        g.connect_realize(move |g| {
            g.make_current();
            let made = match (g.error(), g.context()) {
                (None, Some(ctx)) => {
                    let dialect = if ctx.uses_es() {
                        Dialect::Es
                    } else if ctx.is_legacy() {
                        Dialect::Legacy
                    } else {
                        Dialect::Core
                    };
                    GlPainter::new(dialect)
                }
                (e, _) => Err(e.map_or("no GL context".into(), |e| e.to_string())),
            };
            match made {
                Ok(painter) => {
                    p.borrow_mut().gl = Some(painter);
                    send(&tx, Cmd::Layout(Layout::GlRgba));
                }
                Err(e) => {
                    eprintln!("in-window game view: GL unavailable ({e}); painting with cairo");
                    g.hide();
                }
            }
        });
        let p = paint.clone();
        g.connect_unrealize(move |g| {
            g.make_current();
            if let Some(painter) = p.borrow_mut().gl.take() {
                painter.destroy();
            }
        });
        let p = paint.clone();
        g.connect_render(move |g, _| {
            let began = Instant::now();
            let mut p = p.borrow_mut();
            let Paint { first, reader, gl, stats, .. } = &mut *p;
            let Some(painter) = gl.as_mut() else { return Propagation::Stop };
            let s = g.scale_factor();
            let view = (g.allocated_width() * s, g.allocated_height() * s);
            // Before the first frame the area is cleared to transparent: an undrawn GL area shows black over the
            // page's loading screen (Play again showed a black window instead of the loader).
            let Some(first) = *first else {
                painter.draw(view, 0.0);
                return Propagation::Stop;
            };
            let fresh = reader.as_mut().and_then(|r| r.read_with(|info, px| painter.upload(info, px)).ok().flatten());
            painter.draw(view, fade_alpha(first) as f32);
            if fresh.is_some() {
                Stats::add(stats, began, "GL texture");
            }
            Propagation::Stop
        });
        g
    }

    /// How far the game has faded in since its first frame (at once without animations, the desktop's setting).
    fn fade_alpha(first: Instant) -> f64 {
        let animate = gtk::Settings::default().is_some_and(|s| s.is_gtk_enable_animations());
        if animate {
            (first.elapsed().as_secs_f64() / FADE.as_secs_f64()).min(1.0)
        } else {
            1.0
        }
    }

    /// Lets clicks on an overlay widget go to the webview under it (`on`), or not. The overlay's own pass-through
    /// only covers the window it puts around the widget: the windows the widget makes itself when realized (the
    /// drawing area's, the GL area's input window) still caught every click, so the loading screen's Cancel never
    /// got one and the press moved the keyboard focus to the game view (Esc no longer reached the page either).
    fn pass_through(w: &impl IsA<gtk::Widget>, on: bool) {
        if let Some(o) = w.parent().and_then(|p| p.downcast::<gtk::Overlay>().ok()) {
            o.set_overlay_pass_through(w, on);
        }
        // A widget without a window of its own (the GL area) has its parent's: only the input windows under it are
        // its own, and the parent's (the overlay's child window) is already set above.
        if let Some(gw) = w.window() {
            if w.has_window() {
                gw.set_pass_through(on);
            }
            for c in gw.children() {
                c.set_pass_through(on);
            }
        }
    }

    /// A game frame arrived: the game shows once the page has also let it.
    fn frame_arrived(w: &gtk::DrawingArea, p: &mut Paint) {
        p.arrived.get_or_insert_with(Instant::now);
        show_if_ready(w, p);
    }

    /// The page never answered: show the game anyway.
    fn reveal_overdue(p: &Paint) -> bool {
        !p.revealed && p.arrived.is_some_and(|a| a.elapsed() >= REVEAL_FALLBACK)
    }

    /// First frame there and the page done: the view fades in and takes the mouse and the keyboard from the page.
    fn show_if_ready(w: &gtk::DrawingArea, p: &mut Paint) {
        let first = &mut p.first;
        if first.is_none() && p.revealed && p.arrived.is_some() {
            *first = Some(Instant::now());
            pass_through(w, false);
            w.grab_focus();
        }
    }

    fn wire_paint(area: &gtk::DrawingArea, paint: &Rc<RefCell<Paint>>, gl: Option<gtk::GLArea>) {
        let p = paint.clone();
        area.add_tick_callback(move |w, _| {
            let mut p = p.borrow_mut();
            if let (Some(g), true) = (&gl, p.gl.is_some()) {
                // GL: the area reads and uploads the frame itself when it renders.
                let seq = p.reader.as_ref().and_then(|r| r.seq().ok()).unwrap_or(0);
                if seq != 0 && seq != p.gl_seq {
                    p.gl_seq = seq;
                    frame_arrived(w, &mut p);
                    g.queue_render();
                } else if reveal_overdue(&p) {
                    p.revealed = true;
                    show_if_ready(w, &mut p);
                    g.set_opacity(1.0);
                    g.queue_render();
                } else if p.first.is_some_and(|f| f.elapsed() < FADE) {
                    g.queue_render();
                }
                return glib::ControlFlow::Continue;
            }
            let began = Instant::now();
            if reveal_overdue(&p) {
                p.revealed = true;
                show_if_ready(w, &mut p);
                w.queue_draw();
            }
            let mut arrived = false;
            let Paint { first, reader, buf, surface, stats, .. } = &mut *p;
            let fresh = reader.as_mut().and_then(|r| r.read(buf).ok().flatten());
            // A frame in the GL layout (sent before the runner heard that GL is off) is skipped; the next is cairo's.
            if let Some(info) = fresh.filter(|i| i.layout == Layout::Bgra) {
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
                arrived = true;
                w.queue_draw();
                Stats::add(stats, began, "cairo");
            } else if first.is_some_and(|f| f.elapsed() < FADE) {
                w.queue_draw();
            }
            if arrived {
                frame_arrived(w, &mut p);
            }
            glib::ControlFlow::Continue
        });
        let p = paint.clone();
        area.connect_draw(move |_, cr| {
            let p = p.borrow();
            // With GL the picture is the area underneath: this widget stays transparent.
            let (Some(first), false) = (p.first, p.gl.is_some()) else { return Propagation::Stop };
            let alpha = fade_alpha(first);
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
            show_resize_cursor(w, resize_edge_at(w, x, y));
            send(&t, Cmd::Move { x: (x * scale(w)) as f32, y: (y * scale(w)) as f32 });
            Propagation::Stop
        });
        let t = tx.clone();
        area.connect_button_press_event(move |w, ev| {
            // The view covers the window's side and bottom edges: a press there resizes the (frameless) window.
            let (x, y) = ev.position();
            if let (1, Some(edge)) = (ev.button(), resize_edge_at(w, x, y)) {
                if let Some(gw) = w.toplevel().and_then(|t| t.window()) {
                    let (rx, ry) = ev.root();
                    gw.begin_resize_drag(edge, 1, rx as i32, ry as i32, ev.time());
                    return Propagation::Stop;
                }
            }
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
            let letter = ev.keyval().to_lower().to_unicode();
            let insert = matches!(ev.keyval(), gdk::keys::constants::Insert | gdk::keys::constants::KP_Insert);
            if !repeat && super::is_paste(letter, insert, mods.ctrl, mods.shift, mods.alt) {
                let t = t.clone();
                gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).request_text(move |_, text| {
                    if let Some(text) = text.filter(|s| !s.is_empty()) {
                        send(&t, Cmd::Text(text.to_string()));
                    }
                });
                return Propagation::Stop;
            }
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

    /// The window edge under `(x, y)` of the view, when the window can be resized there (not fullscreen or maximized).
    fn resize_edge_at(w: &gtk::DrawingArea, x: f64, y: f64) -> Option<gdk::WindowEdge> {
        let state = w.toplevel().and_then(|t| t.window()).map(|gw| gw.state())?;
        if state.intersects(gdk::WindowState::FULLSCREEN | gdk::WindowState::MAXIMIZED) {
            return None;
        }
        let a = w.allocation();
        super::resize_edge(x, y, f64::from(a.width()), f64::from(a.height()), super::RESIZE_BORDER)
    }

    fn show_resize_cursor(w: &gtk::DrawingArea, edge: Option<gdk::WindowEdge>) {
        let Some(gw) = w.window() else { return };
        let name = edge.map(|e| match e {
            gdk::WindowEdge::West | gdk::WindowEdge::East => "ew-resize",
            gdk::WindowEdge::SouthWest => "sw-resize",
            gdk::WindowEdge::SouthEast => "se-resize",
            _ => "s-resize",
        });
        gw.set_cursor(name.and_then(|n| gdk::Cursor::from_name(&gw.display(), n)).as_ref());
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
                // The GL picture follows the input widget (unless GL failed: then it stays hidden).
                let gl = v.gl.as_ref().filter(|g| !g.is_realized() || v.paint.borrow().gl.is_some());
                if top < 0 {
                    v.area.hide();
                    if let Some(g) = gl {
                        g.hide();
                    }
                    return;
                }
                v.area.set_margin_top(top);
                if !v.area.is_visible() {
                    v.area.show();
                }
                if let Some(g) = gl {
                    g.set_margin_top(top);
                    if !g.is_visible() {
                        g.show();
                    }
                }
                // Before the first frame the page's loading screen keeps the keyboard (Esc cancels).
                if v.paint.borrow().first.is_some() && !v.area.has_focus() {
                    v.area.grab_focus();
                }
            });
        })
    }

    /// The page's loading screen has shown 100 %: the game may fade in over it.
    pub fn reveal(win: &WebviewWindow) -> Result<(), String> {
        on_main(win, move |_| {
            VIEW.with(|v| {
                let v = v.borrow();
                let Some(v) = v.as_ref() else { return };
                let mut p = v.paint.borrow_mut();
                p.revealed = true;
                show_if_ready(&v.area, &mut p);
                if let Some(g) = &v.gl {
                    g.set_opacity(1.0);
                    g.queue_render();
                }
                v.area.queue_draw();
            });
        })
    }

    /// The overlay around the webview: the first call moves the webview into it, later calls reuse it.
    fn host_overlay(win: &WebviewWindow) -> Result<gtk::Overlay, String> {
        if let Some(o) = HOST.with(|h| h.borrow().clone()) {
            return Ok(o);
        }
        let vbox = win.default_vbox().map_err(|e| e.to_string())?;
        let webview = vbox
            .children()
            .into_iter()
            .find(|c| c.type_().name().contains("WebView"))
            .ok_or("the webview is not in the window box")?;
        let overlay = gtk::Overlay::new();
        vbox.remove(&webview);
        overlay.add(&webview);
        vbox.pack_start(&overlay, true, true, 0);
        overlay.show();
        HOST.with(|h| *h.borrow_mut() = Some(overlay.clone()));
        Ok(overlay)
    }

    /// Moves the webview into its overlay before the window first shows, so the first game start doesn't flash.
    pub fn prepare(win: &WebviewWindow) -> Result<(), String> {
        on_main(win, |w| host_overlay(w).map(|_| ()))?
    }

    fn close_now() {
        VIEW.with(|v| {
            if let Some(v) = v.borrow_mut().take() {
                v.overlay.remove(&v.area);
                if let Some(g) = &v.gl {
                    v.overlay.remove(g);
                }
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
    pub fn reveal(_: &WebviewWindow) -> Result<(), String> {
        Ok(())
    }
    pub fn prepare(_: &WebviewWindow) -> Result<(), String> {
        Ok(())
    }
    pub fn set_fullscreen(win: &WebviewWindow, _: &AppHandle, on: bool) -> Result<(), String> {
        win.set_fullscreen(on).map_err(|e| e.to_string())
    }
    pub fn close(_: &WebviewWindow) -> Result<(), String> {
        Ok(())
    }
}
