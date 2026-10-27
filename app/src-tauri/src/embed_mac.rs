//! The game inside the launcher window on macOS: an `NSView` of our own laid over the webview, below the title bar and
//! the control strip (the part the page reports, `layout`). Opt-in for now (`BHPC_EMBED=1`): it has only been built,
//! never run on a Mac; without the flag the game opens in a window of its own as before.
//!
//! Same protocol as the GTK and Windows views (`embed.rs`, `embed_win.rs`): the runner (`bhpc-run --embed FILE`,
//! rendering into a hidden window of its own) publishes frames into the frame file in BGRA, top row first. A thread of
//! our own ([`frames`]) takes the newest one, wraps it in a `CGImage` (one copy, straight from the mapping of the file)
//! and hands it to the main thread, which makes it the contents of the view's layer: Core Animation scales it to the
//! view and composites it with the webview. The same thread posts a layout tick (placement, reveal, the size for the
//! runner). Mouse, wheel, keys (virtual key code to HID usage), text, focus and size go to the runner as
//! `bhpc_launcher::embed::Cmd` lines. All AppKit work happens on the main thread.

use bhpc_input::{Input, Trigger};
use bhpc_launcher::embed::{hid_from_mac_keycode, Cmd, FrameReader, Layout, MOD_ALT, MOD_CTRL, MOD_SHIFT};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSEvent, NSEventModifierFlags, NSPasteboard, NSPasteboardTypeString, NSResponder,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowOrderingMode,
};
use objc2_core_foundation::{CFData, CFRetained, CFType};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, WebviewWindow};

pub const SUPPORTED: bool = true;
/// Untested on real hardware: only with `BHPC_EMBED=1`.
pub const OPT_IN: bool = true;

/// If the page never says it is ready to hand over (`reveal`), the game shows this long after its first frame.
const REVEAL_FALLBACK: Duration = Duration::from_secs(3);
/// How long the size must stay put before the runner hears it (a window drag sends many).
const SIZE_SETTLE: Duration = Duration::from_millis(60);
/// Layout tick period (placement, reveal, size; the frames have their own pace).
const TICK: Duration = Duration::from_millis(50);
/// How often the frame thread looks for a new frame.
const FRAME_POLL: Duration = Duration::from_millis(2);
/// Wheel steps per pixel of a precise (trackpad) scroll: about one notch per 10 points.
const PRECISE_SCROLL: f64 = 0.1;

/// What the view needs from the launcher.
pub struct Opts {
    pub frame: FrameReader,
    /// Lines for the runner's stdin.
    pub input: Sender<String>,
    /// The fullscreen binding: the launcher handles it itself (the window is its own).
    pub fullscreen_key: Trigger,
    pub app: AppHandle,
}

/// A frame made into an image on the frame thread, waiting for the main thread.
struct Image(CFRetained<CGImage>);
// SAFETY: `CGImage` is immutable and thread-safe (Quartz documents it so); only the reference count crosses threads.
unsafe impl Send for Image {}

/// What the main thread and the frame thread share.
#[derive(Default)]
struct Shared {
    /// The newest image not yet shown.
    image: Mutex<Option<Image>>,
    /// A closure that shows `image` is queued on the main thread (no second one is posted meanwhile).
    posted: AtomicBool,
    /// The first frame has been made into an image.
    arrived: AtomicBool,
    /// The view is closing: the thread ends.
    stop: AtomicBool,
}

struct State {
    view: Retained<GameView>,
    window: Retained<NSWindow>,
    win: WebviewWindow,
    app: AppHandle,
    tx: Sender<String>,
    fullscreen_key: Trigger,
    shared: Arc<Shared>,
    /// Where the stage starts (points below the content view's top); `None` until the page said it, or while a
    /// dialog of the page covers the stage.
    top: Option<f64>,
    /// Size last sent to the runner (pixels), and the one waiting for [`SIZE_SETTLE`] since when.
    sent: (u32, u32),
    pending: Option<((u32, u32), Instant)>,
    arrived: Option<Instant>,
    revealed: bool,
    /// Once shown, the view takes the mouse and the keyboard (before that the page's loading screen has them).
    shown: bool,
    /// Key codes held down (to tell repeats and modifier releases).
    held: HashSet<u16>,
    /// Keys the launcher took (the fullscreen binding): their release is not the game's either.
    swallowed: HashSet<u16>,
}

thread_local! {
    static VIEW: RefCell<Option<State>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; `GameView` has no ivars and no `Drop`.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "BlockheadsPcGameView"]
    struct GameView;

    impl GameView {
        /// Top-left origin, like the runner's pixels.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// The first click into an inactive window already reaches the game.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            with_state(|s| send(&s.tx, Cmd::Focus(true)));
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            with_state(|s| send(&s.tx, Cmd::Focus(false)));
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            button(self, event, 0, true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            button(self, event, 0, false);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            button(self, event, 2, true);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            button(self, event, 2, false);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            button(self, event, other_button(event), true);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            button(self, event, other_button(event), false);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            moved(self, event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            moved(self, event);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            moved(self, event);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            moved(self, event);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            wheel(self, event);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            key(event, true);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            key(event, false);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            modifier(event);
        }
    }
);

fn send(tx: &Sender<String>, cmd: Cmd) {
    let _ = tx.send(cmd.encode());
}

/// Runs `f` with the view's state, unless there is no view or it is in use further up the stack.
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    VIEW.with(|v| v.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Makes the view the window's first responder, with no borrow of the state held (AppKit calls
/// `becomeFirstResponder` right away, which tells the runner).
fn take_keyboard() {
    let pair = VIEW.with(|v| v.borrow().as_ref().map(|s| (s.window.clone(), s.view.clone())));
    if let Some((window, view)) = pair {
        window.makeFirstResponder(Some(&view));
    }
}

/// Runs `f` on the main thread and returns its result (synchronous Tauri commands already run there).
fn on_main<T: Send + 'static>(
    win: &WebviewWindow,
    f: impl FnOnce(&WebviewWindow, MainThreadMarker) -> T + Send + 'static,
) -> Result<T, String> {
    if let Some(mtm) = MainThreadMarker::new() {
        return Ok(f(win, mtm));
    }
    let (tx, rx) = channel();
    let w = win.clone();
    win.run_on_main_thread(move || {
        let mtm = MainThreadMarker::new().expect("run_on_main_thread runs on the main thread");
        let _ = tx.send(f(&w, mtm));
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())
}

fn mods_of(event: &NSEvent) -> (u8, bhpc_input::Mods) {
    let f = event.modifierFlags();
    let m = bhpc_input::Mods {
        ctrl: f.contains(NSEventModifierFlags::Control),
        shift: f.contains(NSEventModifierFlags::Shift),
        alt: f.contains(NSEventModifierFlags::Option),
    };
    let bits =
        (if m.ctrl { MOD_CTRL } else { 0 }) | (if m.shift { MOD_SHIFT } else { 0 }) | (if m.alt { MOD_ALT } else { 0 });
    (bits, m)
}

fn other_button(event: &NSEvent) -> u8 {
    // AppKit counts 0 left, 1 right, 2 middle, then the extra buttons; the runner counts 0 left, 1 middle, 2 right.
    match event.buttonNumber() {
        2 => 1,
        n => n.clamp(3, 255) as u8,
    }
}

/// The event's position in the runner's pixels: the size it was last told (right after a resize the layer still
/// stretches the old frames over the new size, and a press must land where it shows).
fn runner_point(view: &GameView, s: &State, event: &NSEvent) -> (f32, f32) {
    let p = view.convertPoint_fromView(event.locationInWindow(), None);
    let b = view.bounds();
    let (w, h) = (b.size.width.max(1.0), b.size.height.max(1.0));
    let (rw, rh) = if s.sent.0 > 0 && s.sent.1 > 0 {
        (f64::from(s.sent.0), f64::from(s.sent.1))
    } else {
        let scale = s.window.backingScaleFactor();
        (w * scale, h * scale)
    };
    ((p.x * rw / w) as f32, (p.y * rh / h) as f32)
}

fn button(view: &GameView, event: &NSEvent, button: u8, pressed: bool) {
    with_state(|s| {
        let (x, y) = runner_point(view, s, event);
        send(&s.tx, Cmd::Button { button, pressed, x, y, mods: mods_of(event).0 });
    });
    if pressed {
        take_keyboard();
    }
}

fn moved(view: &GameView, event: &NSEvent) {
    with_state(|s| {
        let (x, y) = runner_point(view, s, event);
        send(&s.tx, Cmd::Move { x, y });
    });
}

fn wheel(view: &GameView, event: &NSEvent) {
    // AppKit counts up (content moves down) and left as positive, like the mapper; trackpads report points.
    let step = if event.hasPreciseScrollingDeltas() { PRECISE_SCROLL } else { 1.0 };
    let (dx, dy) = ((event.scrollingDeltaX() * step) as f32, (event.scrollingDeltaY() * step) as f32);
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    with_state(|s| {
        let (x, y) = runner_point(view, s, event);
        send(&s.tx, Cmd::Wheel { dx, dy, x, y, mods: mods_of(event).0 });
    });
}

/// Text a key press types, without the characters AppKit gives function and arrow keys (private use area) and
/// control characters (Backspace, Enter, Tab reach the game as keys).
fn typed(event: &NSEvent) -> Option<String> {
    let text = event.characters()?.to_string();
    let plain = |c: char| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c);
    (!text.is_empty() && text.chars().all(plain)).then_some(text)
}

fn key(event: &NSEvent, pressed: bool) {
    let code = event.keyCode();
    let (bits, mods) = mods_of(event);
    let command = event.modifierFlags().contains(NSEventModifierFlags::Command);
    let toggle = with_state(|s| {
        let hid = hid_from_mac_keycode(code)?;
        if !pressed {
            s.held.remove(&code);
            if !s.swallowed.remove(&code) {
                send(&s.tx, Cmd::Key { hid, pressed: false, repeat: false, mods: bits });
            }
            return None;
        }
        let repeat = event.isARepeat() || !s.held.insert(code);
        if s.fullscreen_key.fires(Input::Key(bhpc_input::keymap::host_key_from_hid(hid)), mods) {
            s.swallowed.insert(code);
            return (!repeat).then(|| (s.win.clone(), s.app.clone()));
        }
        if command {
            // Command shortcuts belong to the launcher (its menu); Command+V types the clipboard into the game.
            let letter = event.charactersIgnoringModifiers().map(|t| t.to_string());
            if !repeat && letter.as_deref().is_some_and(|l| l.eq_ignore_ascii_case("v")) {
                if let Some(text) = clipboard_text().filter(|t| !t.is_empty()) {
                    send(&s.tx, Cmd::Text(text));
                }
            }
            s.held.remove(&code);
            return None;
        }
        send(&s.tx, Cmd::Key { hid, pressed: true, repeat, mods: bits });
        if let Some(text) = typed(event).filter(|_| !mods.ctrl) {
            send(&s.tx, Cmd::Text(text));
        }
        None
    })
    .flatten();
    if let Some((win, app)) = toggle {
        let on = !win.is_fullscreen().unwrap_or(false);
        set_fullscreen_now(&win, &app, on);
    }
}

/// Shift, Control, Option and Caps Lock come as `flagsChanged:`, a press or a release of the key by key code.
fn modifier(event: &NSEvent) {
    let code = event.keyCode();
    let Some(hid) = hid_from_mac_keycode(code) else { return };
    let bits = mods_of(event).0;
    with_state(|s| {
        let pressed = s.held.insert(code);
        if !pressed {
            s.held.remove(&code);
        }
        send(&s.tx, Cmd::Key { hid, pressed, repeat: false, mods: bits });
    });
}

fn clipboard_text() -> Option<String> {
    // SAFETY: a static string constant of AppKit.
    let kind = unsafe { NSPasteboardTypeString };
    NSPasteboard::generalPasteboard().stringForType(kind).map(|t| t.to_string())
}

/// Put text the game copied on the desktop clipboard.
pub fn copy_text(text: &str) {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    // SAFETY: a static string constant of AppKit.
    let kind = unsafe { NSPasteboardTypeString };
    board.setString_forType(&NSString::from_str(text), kind);
}

/// A BGRA frame (top row first) as an image Core Animation can show: the bytes are copied once, into the data the
/// image owns.
fn image_of(width: u32, height: u32, pixels: &[u8]) -> Option<Image> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return None;
    }
    let data = CFData::from_bytes(&pixels[..w * h * 4]);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    // BGRA in memory is a little-endian 32-bit ARGB word; the alpha byte is ignored (the game is opaque).
    let info = CGBitmapInfo(CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::NoneSkipFirst.0);
    // SAFETY: the provider holds `w * h * 4` bytes, 8 bits per channel, 4 bytes per pixel, rows of `w * 4` bytes.
    let image = unsafe {
        CGImage::new(
            w,
            h,
            8,
            32,
            w * 4,
            Some(&space),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }?;
    Some(Image(image))
}

/// The frame thread: the newest frame as an image for the main thread, and the layout tick.
fn frames(mut reader: FrameReader, shared: Arc<Shared>, win: WebviewWindow) {
    let mut last_tick = Instant::now();
    while !shared.stop.load(Ordering::SeqCst) {
        let mut made = None;
        let read = reader.read_with(|info, pixels| {
            made = if info.layout == Layout::Bgra { image_of(info.width, info.height, pixels) } else { None };
        });
        if let (Ok(Some(_)), Some(image)) = (read, made) {
            *shared.image.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(image);
            shared.arrived.store(true, Ordering::SeqCst);
            if !shared.posted.swap(true, Ordering::SeqCst) {
                let sh = shared.clone();
                let _ = win.run_on_main_thread(move || show(&sh));
            }
        }
        if last_tick.elapsed() >= TICK {
            last_tick = Instant::now();
            let sh = shared.clone();
            let _ = win.run_on_main_thread(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    tick(&sh, mtm);
                }
            });
        }
        std::thread::sleep(FRAME_POLL);
    }
    reader.remove();
}

/// Main thread: the newest image becomes the layer's contents.
fn show(shared: &Arc<Shared>) {
    shared.posted.store(false, Ordering::SeqCst);
    let image = shared.image.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    let Some(Image(image)) = image else { return };
    with_state(|s| {
        if !Arc::ptr_eq(&s.shared, shared) {
            return;
        }
        if let Some(layer) = s.view.layer() {
            let cf: &CFType = &image;
            let contents: &AnyObject = cf.as_ref();
            // SAFETY: a `CGImage` is a valid layer contents object. The view's own (AppKit-managed) layer does not
            // animate content changes.
            unsafe { layer.setContents(Some(contents)) };
        }
    });
}

/// Where the view belongs in the content view (flipped or not), or `None` while it should be hidden.
fn target(s: &State) -> Option<NSRect> {
    let top = s.top?;
    let content = s.window.contentView()?;
    let b = content.bounds();
    let h = (b.size.height - top).max(1.0);
    let y = if content.isFlipped() { top } else { 0.0 };
    Some(NSRect::new(NSPoint::new(0.0, y), NSSize::new(b.size.width, h)))
}

/// Moves the view to the stage, shows or hides it, tells the runner a new size once it settled and gives the view
/// the keyboard when it first shows (true: the caller calls [`take_keyboard`] once the state is free again).
fn place(s: &mut State) -> bool {
    let rect = target(s);
    if let Some(r) = rect {
        if r != s.view.frame() {
            s.view.setFrame(r);
        }
        let scale = s.window.backingScaleFactor();
        let size = ((r.size.width * scale).round() as u32, (r.size.height * scale).round() as u32);
        match s.pending {
            _ if size == s.sent => s.pending = None,
            Some((p, _)) if p == size => {}
            _ => s.pending = Some((size, Instant::now())),
        }
        if let Some(((width, height), since)) = s.pending {
            if since.elapsed() >= SIZE_SETTLE {
                s.pending = None;
                s.sent = (width, height);
                send(&s.tx, Cmd::Size { width, height });
            }
        }
    }
    let visible = rect.is_some() && s.revealed && s.arrived.is_some();
    if visible == s.view.isHidden() {
        s.view.setHidden(!visible);
    }
    let first_show = visible && !s.shown;
    s.shown |= first_show;
    first_show
}

/// Layout tick: follow the window's size, show the game once it is ready.
fn tick(shared: &Arc<Shared>, _: MainThreadMarker) {
    let focus = with_state(|s| {
        if !Arc::ptr_eq(&s.shared, shared) {
            return false;
        }
        if s.arrived.is_none() && shared.arrived.load(Ordering::SeqCst) {
            s.arrived = Some(Instant::now());
        }
        if !s.revealed && s.arrived.is_some_and(|a| a.elapsed() >= REVEAL_FALLBACK) {
            s.revealed = true;
        }
        place(s)
    });
    if focus == Some(true) {
        take_keyboard();
    }
}

fn ns_window(win: &WebviewWindow) -> Result<Retained<NSWindow>, String> {
    let ptr = win.ns_window().map_err(|e| e.to_string())?.cast::<NSWindow>();
    // SAFETY: Tauri hands out the window's live `NSWindow`; retaining it keeps it alive while the view uses it.
    unsafe { Retained::retain(ptr) }.ok_or_else(|| "the launcher window has no NSWindow".to_string())
}

/// Makes the game view (hidden until the page has said where the stage starts and the first frame is there).
pub fn open(win: &WebviewWindow, opts: Opts) -> Result<(), String> {
    on_main(win, move |win, mtm| {
        close_now();
        let window = ns_window(win)?;
        let content = window.contentView().ok_or("the launcher window has no content view")?;
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
        // SAFETY: plain NSView initialisation on the main thread.
        let view: Retained<GameView> = unsafe { msg_send![mtm.alloc::<GameView>(), initWithFrame: frame] };
        view.setWantsLayer(true);
        view.setHidden(true);
        // Follow the window's size at once (the top margin stays: the stage starts below the strip).
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // SAFETY: a tracking area owned by the view (it only sends `mouseMoved:` to it) for as long as it is visible.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                mtm.alloc::<NSTrackingArea>(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::ActiveInKeyWindow
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&view),
                None,
            )
        };
        view.addTrackingArea(&area);
        content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, None);
        // Frames as the view shows them (BGRA, top row first): the runner converts the read-back pixels.
        send(&opts.input, Cmd::Layout(Layout::Bgra));
        let shared = Arc::new(Shared::default());
        let (sh, w) = (shared.clone(), win.clone());
        let reader = opts.frame;
        std::thread::Builder::new()
            .name("game-view".into())
            .spawn(move || frames(reader, sh, w))
            .map_err(|e| format!("cannot start the game view ({e})"))?;
        VIEW.with(|v| {
            *v.borrow_mut() = Some(State {
                view,
                window,
                win: win.clone(),
                app: opts.app,
                tx: opts.input,
                fullscreen_key: opts.fullscreen_key,
                shared,
                top: None,
                sent: (0, 0),
                pending: None,
                arrived: None,
                revealed: false,
                shown: false,
                held: HashSet::new(),
                swallowed: HashSet::new(),
            })
        });
        Ok(())
    })?
}

fn set_fullscreen_now(win: &WebviewWindow, app: &AppHandle, on: bool) {
    if win.set_fullscreen(on).is_ok() {
        let _ = app.emit_to("main", "embed-fullscreen", on);
    }
}

/// Switch the window to or from fullscreen and tell the page.
pub fn set_fullscreen(win: &WebviewWindow, app: &AppHandle, on: bool) -> Result<(), String> {
    let app = app.clone();
    on_main(win, move |w, _| set_fullscreen_now(w, &app, on))
}

/// Where the stage starts: the game view covers the window below `top` logical pixels (`0`: all of it, fullscreen);
/// a negative `top` hides it so that the page (a dialog) is visible.
pub fn layout(win: &WebviewWindow, top: i32) -> Result<(), String> {
    on_main(win, move |_, _| {
        let focus = with_state(|s| {
            // Logical pixels are points on macOS.
            s.top = (top >= 0).then_some(f64::from(top));
            place(s)
        });
        if focus == Some(true) {
            take_keyboard();
        }
    })
}

/// The page's loading screen has shown 100 %: the game may show over it.
pub fn reveal(win: &WebviewWindow) -> Result<(), String> {
    on_main(win, move |_, _| {
        let focus = with_state(|s| {
            s.revealed = true;
            place(s)
        });
        if focus == Some(true) {
            take_keyboard();
        }
    })
}

/// The page's stage took the keyboard focus (the webview took it from the view): give it back once the game shows.
pub fn focus(win: &WebviewWindow) -> Result<(), String> {
    on_main(win, move |_, _| {
        if with_state(|s| s.shown) == Some(true) {
            take_keyboard();
        }
    })
}

/// Nothing to prepare: the view is made per game.
pub fn prepare(_: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

fn close_now() {
    let Some(s) = VIEW.with(|v| v.borrow_mut().take()) else { return };
    s.shared.stop.store(true, Ordering::SeqCst);
    if s.window.firstResponder().is_some_and(|r| std::ptr::eq(&*r, &***s.view)) {
        s.window.makeFirstResponder(None);
    }
    s.view.removeFromSuperview();
}

/// Takes the game view away and gives the webview the whole window again.
pub fn close(win: &WebviewWindow) -> Result<(), String> {
    on_main(win, |_, _| close_now())
}
