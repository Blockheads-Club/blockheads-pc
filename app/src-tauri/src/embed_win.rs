//! The game inside the launcher window on Windows: a child window laid over the webview, below the title bar and the
//! control strip (the part the page reports, `layout`).
//!
//! Same protocol as the GTK view (`embed.rs`): the runner (`bhpc-run --embed FILE`, rendering into a hidden window of
//! its own) publishes frames into the frame file; a render thread of our own ([`render`]) takes the newest one as
//! soon as it is there and shows it through a D3D11 flip-model swap chain ([`Gpu`]), uploading it straight from a
//! mapping of the frame file in the layout the runner reads back (no CPU conversion). Not on the UI thread: a
//! `WM_TIMER` there comes only when no input is queued, so the picture stalled while the mouse moved or clicked. GDI would not do: WebView2 draws with DirectComposition,
//! which covers whatever GDI paints into a sibling child window whatever the z-order (the game showed black). Mouse, wheel, keys (scan code to HID usage), text (`WM_CHAR`, so input
//! methods work), focus and size go to the runner as `bhpc_launcher::embed::Cmd` lines. All of it runs on the window's
//! thread (Tauri's main thread).

use bhpc_input::{Input, Trigger};
use bhpc_launcher::embed::{hid_from_scancode, Cmd, FrameInfo, FrameReader, Layout, MOD_ALT, MOD_CTRL, MOD_SHIFT};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, WebviewWindow};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, ScreenToClient, PAINTSTRUCT};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, MapVirtualKeyW, ReleaseCapture, SetCapture, SetFocus, MAPVK_VK_TO_CHAR, VK_CONTROL, VK_F4,
    VK_INSERT, VK_MENU, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const SUPPORTED: bool = true;
pub const OPT_IN: bool = false;

/// If the page never says it is ready to hand over (`reveal`), the game shows this long after its first frame.
const REVEAL_FALLBACK: Duration = Duration::from_secs(3);
/// How long the size must stay put before the runner hears it (a window drag sends many).
const SIZE_SETTLE: u32 = 60;
/// Layout timer period (placement, reveal, focus; the frames have their own thread).
const TICK: u32 = 50;
/// The least time between two pictures while the window is being resized by its edge.
const SIZING_FRAME: Duration = Duration::from_millis(33);
/// How often the render thread looks for a new frame.
const FRAME_POLL: Duration = Duration::from_millis(1);
const TIMER_FRAME: usize = 1;
const TIMER_SIZE: usize = 2;
const CLASS: &str = "BlockheadsPcGameView";
const SUBCLASS_ID: usize = 0xb10c;

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
    hwnd: HWND,
    state: Rc<RefCell<State>>,
}

struct State {
    parent: HWND,
    win: WebviewWindow,
    app: AppHandle,
    tx: Sender<String>,
    fullscreen_key: Trigger,
    /// Shared with the render thread.
    shared: Arc<Shared>,
    /// Where the stage starts (physical pixels below the window's top); `None` until the page said it, or while a
    /// dialog of the page covers the stage.
    top: Option<i32>,
    /// The child's rectangle in the parent (x, y, w, h), as last placed.
    placed: (i32, i32, i32, i32),
    /// Size last sent to the runner, and the one waiting for [`SIZE_SETTLE`].
    sent: (u32, u32),
    pending: (u32, u32),
    /// When the first game frame was shown, and whether the page let it show (`reveal`).
    arrived: Option<Instant>,
    /// The view had the keyboard when the window was deactivated: it takes it back on return (Tauri gives it to the
    /// webview whenever the window is activated).
    refocus: bool,
    revealed: bool,
    /// Once shown, the view takes the mouse and the keyboard (before that the page's loading screen has them).
    shown: bool,
    held: HashSet<u32>,
    /// Keys the launcher took (the fullscreen binding): their release is not the game's either.
    swallowed: HashSet<u32>,
    /// A UTF-16 high surrogate waiting for its partner (`WM_CHAR` sends characters outside the BMP in two halves).
    surrogate: Option<u16>,
    buttons: u8,
}

/// What the UI thread and the render thread share.
#[derive(Default)]
struct Shared {
    /// Client size of the view in pixels (`width << 32 | height`), set by the UI thread.
    size: AtomicU64,
    /// The first frame has been shown.
    arrived: AtomicBool,
    /// Draw again (resized, uncovered) even without a new frame.
    redraw: AtomicBool,
    /// The user is dragging the launcher window's edge (between `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`): the
    /// picture keeps its swap chain size (DXGI stretches it) and is drawn at most [`SIZING_FRAME`] apart, and the
    /// runner hears the size once the drag ends. Resizing the swap chain and presenting 120 times a second while
    /// WebView2 relaid the page out made the title bar lag the window edge (buttons cut off, flashing).
    sizing: AtomicBool,
    /// The view is closing: the thread ends.
    stop: AtomicBool,
}

impl Shared {
    fn set_size(&self, (w, h): (u32, u32)) {
        if self.size.swap(u64::from(w) << 32 | u64::from(h), Ordering::SeqCst) != u64::from(w) << 32 | u64::from(h) {
            self.redraw.store(true, Ordering::SeqCst);
        }
    }

    fn size(&self) -> (u32, u32) {
        let v = self.size.load(Ordering::SeqCst);
        ((v >> 32) as u32, v as u32)
    }
}

thread_local! {
    static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn parent_of(win: &WebviewWindow) -> Result<HWND, String> {
    Ok(win.hwnd().map_err(|e| e.to_string())?.0 as HWND)
}

/// Runs `f` on the window's thread and returns its result (synchronous Tauri commands already run there).
fn on_main<T: Send + 'static>(
    win: &WebviewWindow,
    f: impl FnOnce(&WebviewWindow) -> T + Send + 'static,
) -> Result<T, String> {
    let parent = parent_of(win)?;
    // SAFETY: plain queries on a live window handle.
    if unsafe { GetWindowThreadProcessId(parent, std::ptr::null_mut()) == GetCurrentThreadId() } {
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

fn down(vk: u16) -> bool {
    // SAFETY: reads the thread's keyboard state.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

fn mods_now() -> (u8, bhpc_input::Mods) {
    let m = bhpc_input::Mods { ctrl: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU) };
    let bits =
        (if m.ctrl { MOD_CTRL } else { 0 }) | (if m.shift { MOD_SHIFT } else { 0 }) | (if m.alt { MOD_ALT } else { 0 });
    (bits, m)
}

fn register_class() -> Result<(), String> {
    let name = wide(CLASS);
    // SAFETY: a window class with a static window procedure; registering it twice fails harmlessly.
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            // Redraw at the new size while the window is resized.
            style: CS_DBLCLKS | CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&wc) == 0 && windows_sys::Win32::Foundation::GetLastError() != 1410 {
            // 1410: ERROR_CLASS_ALREADY_EXISTS (a second game start).
            return Err(format!("cannot register the game view class ({})", std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

/// Makes the game view (hidden until the page has said where the stage starts and the first frame is there).
pub fn open(win: &WebviewWindow, opts: Opts) -> Result<(), String> {
    on_main(win, move |win| {
        close_now();
        register_class()?;
        let parent = parent_of(win)?;
        let class = wide(CLASS);
        // SAFETY: a child of the launcher's live top-level window, made on that window's thread.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                parent,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(format!("cannot create the game view ({})", std::io::Error::last_os_error()));
        }
        let shared = Arc::new(Shared::default());
        shared.set_size(client_size(hwnd));
        if let Err(e) = render(hwnd, opts.frame, shared.clone()) {
            // SAFETY: the window made above, never shown.
            unsafe { DestroyWindow(hwnd) };
            return Err(format!("no Direct3D 11 for the game view ({e})"));
        }
        // Frames as read back (RGBA, bottom row first): the shader converts them.
        send(&opts.input, Cmd::Layout(Layout::GlRgba));
        let state = Rc::new(RefCell::new(State {
            parent,
            win: win.clone(),
            app: opts.app,
            tx: opts.input,
            fullscreen_key: opts.fullscreen_key,
            shared,
            top: None,
            placed: (0, 0, 0, 0),
            sent: (0, 0),
            pending: (0, 0),
            arrived: None,
            refocus: false,
            revealed: false,
            shown: false,
            held: HashSet::new(),
            swallowed: HashSet::new(),
            surrogate: None,
            buttons: 0,
        }));
        // SAFETY: the pointer stays valid while the window lives: `VIEW` keeps the `Rc` until after `DestroyWindow`.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&state) as isize);
            SetTimer(hwnd, TIMER_FRAME, TICK, None);
            // Follow the launcher window's size at once, not on the next layout tick.
            SetWindowSubclass(parent, Some(parent_proc), SUBCLASS_ID, 0);
            transitions(parent, false);
            let _ = win.as_ref().window().set_background_color(Some(tauri::window::Color(0x1f, 0x1a, 0x17, 0xff)));
            see_through(parent, false);
        }
        VIEW.with(|v| *v.borrow_mut() = Some(View { hwnd, state }));
        Ok(())
    })?
}

/// The state of `hwnd` (the window procedure's view of it).
///
/// # Safety
/// `hwnd` must be a game view made by [`open`] that is still alive.
unsafe fn state_of<'a>(hwnd: HWND) -> Option<&'a RefCell<State>> {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State>;
    p.as_ref()
}

/// Where the child belongs and whether it should be visible now.
fn target(s: &State) -> Option<(i32, i32, i32, i32)> {
    let top = s.top?;
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: reads the client rectangle of the live parent window.
    unsafe { GetClientRect(s.parent, &mut r) };
    Some((0, top, (r.right - r.left).max(1), (r.bottom - r.top - top).max(1)))
}

/// Moves the child to the stage, shows or hides it, tells the runner a new size (after it settles) and gives it the
/// keyboard when it first shows. Called with no borrow of the state held (the window calls re-enter the procedure).
fn place(hwnd: HWND, cell: &RefCell<State>) {
    let (rect, visible, first_show, first_place) = {
        let Ok(mut s) = cell.try_borrow_mut() else { return };
        let rect = target(&s);
        if let Some((_, _, w, h)) = rect {
            let size = (w as u32, h as u32);
            if size != s.sent && size != s.pending && !s.shared.sizing.load(Ordering::SeqCst) {
                s.pending = size;
                // SAFETY: (re)arms a timer on the live view.
                unsafe { SetTimer(hwnd, TIMER_SIZE, SIZE_SETTLE, None) };
            }
        }
        let ready = s.revealed && s.arrived.is_some();
        let visible = rect.is_some() && ready;
        let first_show = visible && !s.shown;
        if first_show {
            s.shown = true;
        }
        let first_place = s.placed == (0, 0, 0, 0);
        if rect.is_some() {
            s.placed = rect.unwrap_or_default();
        }
        (rect, visible, first_show, first_place)
    };
    // SAFETY: window calls on the live view, on its own thread.
    unsafe {
        if let Some((x, y, w, h)) = rect {
            // Above the webview once; moving it later leaves the z-order alone (a z-order change on every resize
            // made DWM compose the webview again, a flash of the title bar).
            let order = if first_place { SWP_NOACTIVATE } else { SWP_NOACTIVATE | SWP_NOZORDER };
            SetWindowPos(hwnd, HWND_TOP, x, y, w, h, order);
        }
        let now = IsWindowVisible(hwnd) != 0;
        if visible != now {
            ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
            if visible {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
        }
        if first_show {
            SetFocus(hwnd);
        }
    }
}

/// Layout tick: follow the parent's size, show the game once it is ready, take the keyboard back after the window
/// was activated again.
fn tick(hwnd: HWND, cell: &RefCell<State>) {
    let (moved, refocus) = {
        let Ok(mut s) = cell.try_borrow_mut() else { return };
        if s.arrived.is_none() && s.shared.arrived.load(Ordering::SeqCst) {
            s.arrived = Some(Instant::now());
        }
        if !s.revealed && s.arrived.is_some_and(|a| a.elapsed() >= REVEAL_FALLBACK) {
            s.revealed = true;
        }
        // SAFETY: the focus of this (the window's) thread; null while another application is active.
        let focus = unsafe { GetFocus() };
        let refocus = s.refocus && !focus.is_null() && focus != hwnd;
        if refocus {
            s.refocus = false;
        }
        let want = target(&s);
        (want.is_some_and(|r| r != s.placed) || (s.revealed && s.arrived.is_some() && !s.shown), refocus)
    };
    if moved {
        place(hwnd, cell);
    }
    if refocus {
        // SAFETY: the live view, on its own thread.
        unsafe { SetFocus(hwnd) };
    }
}

fn paint(hwnd: HWND, cell: &RefCell<State>) {
    // SAFETY: the usual BeginPaint/EndPaint pair on the live view.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        BeginPaint(hwnd, &mut ps);
        EndPaint(hwnd, &ps);
    }
    // A repaint (resized, shown again): the render thread draws the newest frame at the window's size.
    if let Ok(s) = cell.try_borrow() {
        s.shared.set_size(client_size(hwnd));
        s.shared.redraw.store(true, Ordering::SeqCst);
    }
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: the live view.
    unsafe { GetClientRect(hwnd, &mut r) };
    ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)
}

/// The parent's resize hit-test code for a press at `(x, y)` of the view, which spans the window's width and reaches
/// its bottom: the sides, the bottom or a bottom corner of a window that is neither maximized nor fullscreen.
fn resize_hit(hwnd: HWND, cell: &RefCell<State>, x: i32, y: i32) -> Option<u32> {
    let s = cell.try_borrow().ok()?;
    // SAFETY: queries on the live parent.
    if unsafe { IsZoomed(s.parent) } != 0 || s.win.is_fullscreen().unwrap_or(false) {
        return None;
    }
    let border = (super::RESIZE_BORDER * s.win.scale_factor().unwrap_or(1.0)).round() as i32;
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: the live view.
    unsafe { GetClientRect(hwnd, &mut r) };
    let (left, right, bottom) = (x < border, x >= r.right - border, y >= r.bottom - border);
    Some(match (left, right, bottom) {
        (true, _, true) => HTBOTTOMLEFT,
        (_, true, true) => HTBOTTOMRIGHT,
        (true, _, _) => HTLEFT,
        (_, true, _) => HTRIGHT,
        (_, _, true) => HTBOTTOM,
        _ => return None,
    })
}

/// A point of the view in the runner's pixels: the size it was last told. Right after a resize the view still
/// stretches the old frames over the new size, and a press must land where it shows (in the new part it was lost).
fn to_runner(hwnd: HWND, s: &State, x: i32, y: i32) -> (f32, f32) {
    let (w, h) = client_size(hwnd);
    let (rw, rh) = if s.sent.0 > 0 && s.sent.1 > 0 { s.sent } else { (w, h) };
    (x as f32 * rw as f32 / w as f32, y as f32 * rh as f32 / h as f32)
}

fn xy(lparam: LPARAM) -> (i32, i32) {
    (i32::from((lparam & 0xffff) as u16 as i16), i32::from(((lparam >> 16) & 0xffff) as u16 as i16))
}

fn button(hwnd: HWND, cell: &RefCell<State>, b: u8, pressed: bool, lparam: LPARAM) {
    let (x, y) = xy(lparam);
    if pressed && b == 0 {
        if let Some(hit) = resize_hit(hwnd, cell, x, y) {
            let parent = cell.borrow().parent;
            // SAFETY: hands the press to the frameless parent as a press on its border: Windows resizes it.
            unsafe {
                ReleaseCapture();
                SendMessageW(parent, WM_NCLBUTTONDOWN, hit as WPARAM, 0);
            }
            return;
        }
    }
    let (capture, release) = {
        let Ok(mut s) = cell.try_borrow_mut() else { return };
        let before = s.buttons;
        if pressed {
            s.buttons |= 1 << b.min(7);
        } else {
            s.buttons &= !(1 << b.min(7));
        }
        let (x, y) = to_runner(hwnd, &s, x, y);
        send(&s.tx, Cmd::Button { button: b, pressed, x, y, mods: mods_now().0 });
        (before == 0 && s.buttons != 0, before != 0 && s.buttons == 0)
    };
    // SAFETY: the live view, on its own thread.
    unsafe {
        if pressed {
            SetFocus(hwnd);
        }
        if capture {
            SetCapture(hwnd);
        } else if release {
            ReleaseCapture();
        }
    }
}

fn key(hwnd: HWND, cell: &RefCell<State>, msg: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
    let pressed = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let code = ((lparam >> 16) & 0xff) as u32;
    let extended = lparam & (1 << 24) != 0;
    let id = code | if extended { 0x100 } else { 0 };
    let vk = wparam as u16;
    let (bits, mods) = mods_now();
    if pressed && vk == VK_F4 && mods.alt {
        // Alt+F4 closes the launcher as anywhere else.
        let parent = cell.borrow().parent;
        // SAFETY: posts to the live parent.
        unsafe { PostMessageW(parent, WM_SYSCOMMAND, SC_CLOSE as WPARAM, 0) };
        return true;
    }
    let Some(hid) = hid_from_scancode(code, extended) else { return true };
    let Ok(mut s) = cell.try_borrow_mut() else { return true };
    if !pressed {
        s.held.remove(&id);
        if !s.swallowed.remove(&id) {
            send(&s.tx, Cmd::Key { hid, pressed: false, repeat: false, mods: bits });
        }
        return true;
    }
    let repeat = !s.held.insert(id);
    if s.fullscreen_key.fires(Input::Key(bhpc_input::keymap::host_key_from_hid(hid)), mods) {
        s.swallowed.insert(id);
        if !repeat {
            let (win, app) = (s.win.clone(), s.app.clone());
            drop(s);
            let on = !win.is_fullscreen().unwrap_or(false);
            set_fullscreen_now(&win, &app, on);
        }
        return true;
    }
    send(&s.tx, Cmd::Key { hid, pressed: true, repeat, mods: bits });
    // SAFETY: a table lookup of the current keyboard layout.
    let letter =
        char::from_u32(unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_CHAR) } & 0xffff).filter(|c| *c != '\0');
    if !repeat && super::is_paste(letter, vk == VK_INSERT, mods.ctrl, mods.shift, mods.alt) {
        if let Some(text) = clipboard_text(hwnd).filter(|t| !t.is_empty()) {
            send(&s.tx, Cmd::Text(text));
        }
    }
    true
}

fn char_in(cell: &RefCell<State>, unit: u16) {
    let Ok(mut s) = cell.try_borrow_mut() else { return };
    let text = match (s.surrogate.take(), unit) {
        (_, 0xd800..=0xdbff) => {
            s.surrogate = Some(unit);
            return;
        }
        (Some(hi), 0xdc00..=0xdfff) => String::from_utf16(&[hi, unit]).ok(),
        (_, u) => char::from_u32(u32::from(u)).map(String::from),
    };
    // Control characters (Backspace, Enter, Ctrl+letter) reach the game as keys.
    if let Some(t) = text.filter(|t| !t.chars().any(char::is_control)) {
        send(&s.tx, Cmd::Text(t));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(cell) = state_of(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_TIMER if wparam == TIMER_FRAME => tick(hwnd, cell),
        WM_TIMER if wparam == TIMER_SIZE => {
            KillTimer(hwnd, TIMER_SIZE);
            if let Ok(mut s) = cell.try_borrow_mut() {
                let (width, height) = s.pending;
                if (width, height) != s.sent {
                    s.sent = (width, height);
                    send(&s.tx, Cmd::Size { width, height });
                }
            }
        }
        WM_PAINT => paint(hwnd, cell),
        WM_ERASEBKGND => return 1,
        WM_SETCURSOR => {
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            ScreenToClient(hwnd, &mut p);
            let cursor = match resize_hit(hwnd, cell, p.x, p.y) {
                Some(HTLEFT | HTRIGHT) => IDC_SIZEWE,
                Some(HTBOTTOM) => IDC_SIZENS,
                Some(HTBOTTOMLEFT) => IDC_SIZENESW,
                Some(_) => IDC_SIZENWSE,
                None => IDC_ARROW,
            };
            SetCursor(LoadCursorW(std::ptr::null_mut(), cursor));
            return 1;
        }
        WM_MOUSEMOVE => {
            let (x, y) = xy(lparam);
            if let Ok(s) = cell.try_borrow() {
                let (x, y) = to_runner(hwnd, &s, x, y);
                send(&s.tx, Cmd::Move { x, y });
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => button(hwnd, cell, 0, true, lparam),
        WM_LBUTTONUP => button(hwnd, cell, 0, false, lparam),
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => button(hwnd, cell, 1, true, lparam),
        WM_MBUTTONUP => button(hwnd, cell, 1, false, lparam),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => button(hwnd, cell, 2, true, lparam),
        WM_RBUTTONUP => button(hwnd, cell, 2, false, lparam),
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = f32::from(((wparam >> 16) & 0xffff) as u16 as i16) / 120.0;
            let (sx, sy) = xy(lparam);
            let mut p = POINT { x: sx, y: sy };
            ScreenToClient(hwnd, &mut p);
            // Windows counts up (away from the user) and right as positive; the mapper counts up and left.
            let (dx, dy) = if msg == WM_MOUSEWHEEL { (0.0, delta) } else { (-delta, 0.0) };
            if let Ok(s) = cell.try_borrow() {
                let (x, y) = to_runner(hwnd, &s, p.x, p.y);
                send(&s.tx, Cmd::Wheel { dx, dy, x, y, mods: mods_now().0 });
            }
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            if key(hwnd, cell, msg, wparam, lparam) {
                return 0;
            }
        }
        // Alt+letter would beep (no menu here); the key itself already went to the game.
        WM_SYSCHAR => return 0,
        WM_CHAR => char_in(cell, wparam as u16),
        WM_SETFOCUS | WM_KILLFOCUS => {
            if let Ok(mut s) = cell.try_borrow_mut() {
                // Losing the keyboard to nothing (another application) comes back with the window; to the webview
                // (a click on the strip) it does not.
                s.refocus = msg == WM_KILLFOCUS && wparam == 0;
                send(&s.tx, Cmd::Focus(msg == WM_SETFOCUS));
            }
        }
        WM_SIZE => {
            if let Ok(s) = cell.try_borrow() {
                s.shared.set_size(client_size(hwnd));
            }
        }
        WM_CAPTURECHANGED => {
            if let Ok(mut s) = cell.try_borrow_mut() {
                s.buttons = 0;
            }
        }
        _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
    }
    0
}

fn clipboard_text(owner: HWND) -> Option<String> {
    // SAFETY: the clipboard is opened, read through a locked handle and closed again on this thread.
    unsafe {
        if OpenClipboard(owner) == 0 {
            return None;
        }
        let h = GetClipboardData(u32::from(CF_UNICODETEXT));
        let p = if h.is_null() { std::ptr::null_mut() } else { GlobalLock(h) as *const u16 };
        let text = (!p.is_null()).then(|| {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        });
        if !p.is_null() {
            GlobalUnlock(h);
        }
        CloseClipboard();
        text
    }
}

/// Put text the game copied on the desktop clipboard (the window's thread).
pub fn copy_text(text: &str) {
    let units = wide(text);
    // SAFETY: a movable global block filled while locked and handed to the clipboard, which owns it from then on.
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
                GlobalUnlock(h);
                SetClipboardData(u32::from(CF_UNICODETEXT), h);
            }
        }
        CloseClipboard();
    }
}

thread_local! {
    /// The window was maximized when it went fullscreen: it is maximized again when it leaves.
    static WAS_MAXIMIZED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn set_fullscreen_now(win: &WebviewWindow, app: &AppHandle, on: bool) {
    // A maximized window stays in the work area (the taskbar shows) when tao makes it fullscreen: restore it first.
    if on && win.is_maximized().unwrap_or(false) && !win.is_fullscreen().unwrap_or(false) {
        WAS_MAXIMIZED.set(true);
        let _ = win.unmaximize();
    }
    let done = win.set_fullscreen(on).is_ok();
    if !on && WAS_MAXIMIZED.replace(false) {
        let _ = win.maximize();
    }
    if done {
        let _ = app.emit_to("main", "embed-fullscreen", on);
    }
}

/// Switch the window to or from fullscreen and tell the page.
pub fn set_fullscreen(win: &WebviewWindow, app: &AppHandle, on: bool) -> Result<(), String> {
    let app = app.clone();
    on_main(win, move |w| set_fullscreen_now(w, &app, on))
}

fn with_view(f: impl FnOnce(HWND, &RefCell<State>)) {
    let view = VIEW.with(|v| v.borrow().as_ref().map(|v| (v.hwnd, v.state.clone())));
    if let Some((hwnd, state)) = view {
        f(hwnd, &state);
    }
}

/// Where the stage starts: the game view covers the window below `top` logical pixels (`0`: all of it, fullscreen);
/// a negative `top` hides it so that the page (a dialog) is visible.
pub fn layout(win: &WebviewWindow, top: i32) -> Result<(), String> {
    on_main(win, move |win| {
        let scale = win.scale_factor().unwrap_or(1.0);
        with_view(|hwnd, cell| {
            if let Ok(mut s) = cell.try_borrow_mut() {
                s.top = (top >= 0).then(|| (f64::from(top) * scale).round() as i32);
            }
            place(hwnd, cell);
        });
    })
}

/// The page's loading screen has shown 100 %: the game may show over it.
pub fn reveal(win: &WebviewWindow) -> Result<(), String> {
    on_main(win, move |_| {
        with_view(|hwnd, cell| {
            if let Ok(mut s) = cell.try_borrow_mut() {
                s.revealed = true;
            }
            place(hwnd, cell);
        });
    })
}

/// The page's stage took the keyboard focus (WebView2 took it from the view): give it back once the game shows.
pub fn focus(win: &WebviewWindow) -> Result<(), String> {
    on_main(win, move |_| {
        with_view(|hwnd, cell| {
            if cell.try_borrow().is_ok_and(|s| s.shown) {
                // SAFETY: the live view, on its own thread.
                unsafe { SetFocus(hwnd) };
            }
        });
    })
}

/// Nothing to prepare: the child window is made per game.
pub fn prepare(_: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

/// The launcher window's procedure, in front of Tauri's: a size change moves the game view along right away.
unsafe extern "system" fn parent_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    let r = DefSubclassProc(hwnd, msg, wparam, lparam);
    match msg {
        WM_SIZE => with_view(place),
        WM_ENTERSIZEMOVE | WM_EXITSIZEMOVE => with_view(|view, cell| {
            let ended = msg == WM_EXITSIZEMOVE;
            if let Ok(s) = cell.try_borrow() {
                s.shared.sizing.store(!ended, Ordering::SeqCst);
                s.shared.redraw.store(true, Ordering::SeqCst);
            }
            if ended {
                // The size the drag ended at goes to the runner now.
                place(view, cell);
            }
        }),
        _ => {}
    }
    r
}

/// Turns the window's maximize / restore / minimize animations on or off. While the game shows they are off: DWM
/// animates the webview's DirectComposition content and the game's swap chain apart, so the title bar and the strip
/// vanished and came back (the game's picture stood over them) on every maximize and restore.
fn transitions(window: HWND, on: bool) {
    let off: i32 = i32::from(!on);
    // SAFETY: a documented window attribute with a 4-byte BOOL value, on a live window.
    unsafe {
        DwmSetWindowAttribute(
            window,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            std::ptr::addr_of!(off).cast(),
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Makes the launcher window see-through (how tao makes a transparent window: blur-behind with an empty region) or
/// opaque. While the game shows it is opaque with the chrome's colour behind the webview: WebView2 leaves a frame or
/// two empty when the window is maximized or restored (and once, about a second after the first maximize, a whole
/// frame), and a see-through window showed the desktop there, the title bar and the strip flashing away.
fn see_through(window: HWND, on: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmEnableBlurBehindWindow, DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND,
    };
    use windows_sys::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject};
    // SAFETY: plain DWM and GDI calls on a live window; the region is deleted after use.
    unsafe {
        let region = if on { CreateRectRgn(0, 0, -1, -1) } else { std::ptr::null_mut() };
        let bb = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | if on { DWM_BB_BLURREGION } else { 0 },
            fEnable: i32::from(on),
            hRgnBlur: region,
            fTransitionOnMaximized: 0,
        };
        DwmEnableBlurBehindWindow(window, &bb);
        if !region.is_null() {
            DeleteObject(region);
        }
        InvalidateRect(window, std::ptr::null(), 1);
    }
}

fn close_now() {
    let Some(view) = VIEW.with(|v| v.borrow_mut().take()) else { return };
    let parent = view.state.borrow().parent;
    // SAFETY: removes the subclass `open` installed on the live parent (a no-op when it is not there).
    unsafe { RemoveWindowSubclass(parent, Some(parent_proc), SUBCLASS_ID) };
    transitions(parent, true);
    let window = view.state.borrow().win.clone();
    see_through(parent, true);
    let _ = window.as_ref().window().set_background_color(None);
    // SAFETY: destroys the view on its own thread; the state (`view.state`) outlives the messages this sends.
    unsafe {
        KillTimer(view.hwnd, TIMER_FRAME);
        KillTimer(view.hwnd, TIMER_SIZE);
        SetWindowLongPtrW(view.hwnd, GWLP_USERDATA, 0);
        DestroyWindow(view.hwnd);
    }
    // The render thread ends on its own (it deletes the frame file); waiting for it here could deadlock with DXGI,
    // which may send messages to the window this thread owns.
    view.state.borrow().shared.stop.store(true, Ordering::SeqCst);
}

/// Takes the game view away and gives the webview the whole window again.
pub fn close(win: &WebviewWindow) -> Result<(), String> {
    on_main(win, |_| close_now())
}

/// The picture: each frame is uploaded into a texture as the runner read it back (RGBA, bottom row first: no CPU
/// conversion on either side, like the GTK view's GL path) and drawn as a full-window triangle into a flip-model swap
/// chain of the window's size. The shader flips the rows; the texture format does the channel order.
struct Gpu {
    dev: ID3D11Device,
    ctx: ID3D11DeviceContext,
    chain: IDXGISwapChain1,
    /// Render target of the back buffer and its size (made again after a resize).
    target: Option<(ID3D11RenderTargetView, (u32, u32))>,
    /// The frame texture, its size and layout.
    frame: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, (u32, u32), Layout)>,
    vs_flip: ID3D11VertexShader,
    vs_top: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
}

use windows::core::{Interface, PCSTR};
use windows::Win32::Foundation::{HMODULE, HWND as WHWND};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    ID3DBlob, D3D11_SRV_DIMENSION_TEXTURE2D, D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, DXGI_MWA_NO_ALT_ENTER, DXGI_MWA_NO_WINDOW_CHANGES, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

const SHADERS: &str = r"
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
V corner(uint id) {
    V v;
    float2 p = float2((id << 1) & 2, id & 2);
    v.pos = float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
    v.uv = p;
    return v;
}
V vs_top(uint id : SV_VertexID) { return corner(id); }
V vs_flip(uint id : SV_VertexID) { V v = corner(id); v.uv.y = 1 - v.uv.y; return v; }
Texture2D frame : register(t0);
SamplerState linear_clamp : register(s0);
float4 ps(V v) : SV_Target { return float4(frame.Sample(linear_clamp, v.uv).rgb, 1); }
";

fn compile(entry: &str, target: &str) -> windows::core::Result<Vec<u8>> {
    let (entry, target) = (format!("{entry}\0"), format!("{target}\0"));
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: compiles the static source above; the blobs are read before they are released.
    unsafe {
        let r = D3DCompile(
            SHADERS.as_ptr().cast(),
            SHADERS.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry.as_ptr()),
            PCSTR(target.as_ptr()),
            0,
            0,
            &mut code,
            Some(&mut errors),
        );
        if let Some(e) = errors.filter(|_| r.is_err()) {
            let msg = std::slice::from_raw_parts(e.GetBufferPointer() as *const u8, e.GetBufferSize());
            eprintln!("in-window game view: shader {entry}: {}", String::from_utf8_lossy(msg));
        }
        r?;
        let code = code.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        Ok(std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize()).to_vec())
    }
}

impl Gpu {
    fn new(hwnd: HWND) -> windows::core::Result<Gpu> {
        let device = |kind: D3D_DRIVER_TYPE| {
            let (mut dev, mut ctx): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
            // SAFETY: plain device creation with out-pointers to locals.
            unsafe {
                D3D11CreateDevice(
                    None,
                    kind,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut dev),
                    None,
                    Some(&mut ctx),
                )
            }
            .map(|()| (dev, ctx))
        };
        // WARP (software) when there is no usable GPU driver.
        let (dev, ctx) = device(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| device(D3D_DRIVER_TYPE_WARP))?;
        let fail = || windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL);
        let (Some(dev), Some(ctx)) = (dev, ctx) else { return Err(fail()) };
        // SAFETY: COM calls on live objects; the window is ours and alive.
        unsafe {
            let factory: IDXGIFactory2 = dev.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 1,
                Height: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let chain = factory.CreateSwapChainForHwnd(&dev, WHWND(hwnd), &desc, None, None)?;
            // The launcher handles fullscreen itself.
            factory.MakeWindowAssociation(WHWND(hwnd), DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES)?;
            let (mut vs_flip, mut vs_top, mut ps, mut sampler) = (None, None, None, None);
            dev.CreateVertexShader(&compile("vs_flip", "vs_4_0")?, None, Some(&mut vs_flip))?;
            dev.CreateVertexShader(&compile("vs_top", "vs_4_0")?, None, Some(&mut vs_top))?;
            dev.CreatePixelShader(&compile("ps", "ps_4_0")?, None, Some(&mut ps))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            dev.CreateSamplerState(&sd, Some(&mut sampler))?;
            Ok(Gpu {
                dev,
                ctx,
                chain,
                target: None,
                frame: None,
                vs_flip: vs_flip.ok_or_else(fail)?,
                vs_top: vs_top.ok_or_else(fail)?,
                ps: ps.ok_or_else(fail)?,
                sampler: sampler.ok_or_else(fail)?,
            })
        }
    }

    /// Copies a frame into the frame texture (made again when the size or the layout changes).
    fn upload(&mut self, info: FrameInfo, px: &[u8]) -> windows::core::Result<()> {
        let size = (info.width, info.height);
        if px.len() != size.0 as usize * size.1 as usize * 4 {
            return Ok(());
        }
        // SAFETY: COM calls on live objects; `px` holds exactly one frame of `size`.
        unsafe {
            if !self.frame.as_ref().is_some_and(|f| f.2 == size && f.3 == info.layout) {
                self.frame = None;
                let format: DXGI_FORMAT =
                    if info.layout == Layout::Bgra { DXGI_FORMAT_B8G8R8A8_UNORM } else { DXGI_FORMAT_R8G8B8A8_UNORM };
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: size.0,
                    Height: size.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..Default::default()
                };
                let mut tex = None;
                self.dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
                let tex = tex.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
                let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 },
                    },
                };
                let mut srv = None;
                self.dev.CreateShaderResourceView(&tex, Some(&srv_desc), Some(&mut srv))?;
                let srv = srv.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
                self.frame = Some((tex, srv, size, info.layout));
            }
            let tex = &self.frame.as_ref().expect("made above").0;
            self.ctx.UpdateSubresource(tex, 0, None, px.as_ptr().cast(), size.0 * 4, 0);
        }
        Ok(())
    }

    /// Size of the swap chain's buffers, once made.
    fn target_size(&self) -> Option<(u32, u32)> {
        self.target.as_ref().map(|t| t.1)
    }

    /// Draws the frame texture over the whole `view` (the window's client size) and presents it.
    fn draw(&mut self, view: (u32, u32)) -> windows::core::Result<()> {
        let Some((_, srv, _, layout)) = self.frame.as_ref() else { return Ok(()) };
        let view = (view.0.max(1), view.1.max(1));
        // SAFETY: COM calls on live objects owned by `self`.
        unsafe {
            if !self.target.as_ref().is_some_and(|t| t.1 == view) {
                // The back buffers can only be resized once nothing refers to them.
                self.target = None;
                self.ctx.OMSetRenderTargets(None, None);
                self.chain.ResizeBuffers(0, view.0, view.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
                let back: ID3D11Texture2D = self.chain.GetBuffer(0)?;
                let mut rtv = None;
                self.dev.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
                let rtv = rtv.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
                self.target = Some((rtv, view));
            }
            let rtv = self.target.as_ref().expect("made above").0.clone();
            self.ctx.OMSetRenderTargets(Some(&[Some(rtv)]), None);
            self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: view.0 as f32,
                Height: view.1 as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            self.ctx.IASetInputLayout(None);
            self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let vs = if *layout == Layout::GlRgba { &self.vs_flip } else { &self.vs_top };
            self.ctx.VSSetShader(vs, None);
            self.ctx.PSSetShader(&self.ps, None);
            self.ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
            self.ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.ctx.Draw(3, 0);
            // No vsync wait: the frame timer sets the pace and must not block the launcher's UI thread.
            self.chain.Present(0, DXGI_PRESENT(0)).ok()
        }
    }
}

/// Starts the render thread of `hwnd`: it makes the D3D11 device (an error comes back from here), then shows each new
/// frame of `reader` as soon as the runner published it, until `shared.stop`.
fn render(hwnd: HWND, mut reader: FrameReader, shared: Arc<Shared>) -> Result<(), String> {
    let (ready_tx, ready_rx) = channel();
    let hwnd_addr = hwnd as usize;
    std::thread::Builder::new()
        .name("game-view".into())
        .spawn(move || {
            let hwnd = hwnd_addr as HWND;
            let mut gpu = match Gpu::new(hwnd) {
                Ok(g) => {
                    let _ = ready_tx.send(Ok(()));
                    g
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let timer = Waiter::new();
            let mut stats = std::env::var("BHPC_EMBED_STATS")
                .is_ok_and(|v| v == "1")
                .then(|| (0u32, Duration::ZERO, Instant::now()));
            let mut drawn = Instant::now();
            while !shared.stop.load(Ordering::SeqCst) {
                let began = Instant::now();
                let sizing = shared.sizing.load(Ordering::SeqCst);
                if sizing && drawn.elapsed() < SIZING_FRAME {
                    timer.wait(FRAME_POLL);
                    continue;
                }
                let mut uploaded = Ok(());
                // Straight from the mapping into the texture; a frame the runner overwrote meanwhile is not drawn.
                let fresh = reader.read_with(|info, px| uploaded = gpu.upload(info, px)).ok().flatten();
                let redraw = shared.redraw.swap(false, Ordering::SeqCst);
                if fresh.is_some() || redraw {
                    drawn = Instant::now();
                    let size = if sizing { gpu.target_size().unwrap_or(shared.size()) } else { shared.size() };
                    if let Err(e) = uploaded.and_then(|()| gpu.draw(size)) {
                        eprintln!("in-window game view: cannot show a frame ({e})");
                    }
                }
                if fresh.is_some() {
                    shared.arrived.store(true, Ordering::SeqCst);
                    if let Some((n, busy, since)) = stats.as_mut() {
                        *n += 1;
                        *busy += began.elapsed();
                        let secs = since.elapsed().as_secs_f64();
                        if secs >= 10.0 {
                            let ms = busy.as_secs_f64() * 1000.0 / f64::from((*n).max(1));
                            eprintln!(
                                "[embed] view: D3D11, {:.1} frames/s shown, {ms:.2} ms per frame",
                                f64::from(*n) / secs
                            );
                            stats = Some((0, Duration::ZERO, Instant::now()));
                        }
                    }
                }
                timer.wait(FRAME_POLL);
            }
            reader.remove();
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|e| e.to_string())?
}

/// Sleeps of about a millisecond (a plain sleep lasts a whole system tick, 15.6 ms, unless the process raised the
/// timer resolution): a high-resolution waitable timer where Windows has one (10 1803 and later).
struct Waiter(windows_sys::Win32::Foundation::HANDLE);

impl Waiter {
    fn new() -> Waiter {
        use windows_sys::Win32::System::Threading::{
            CreateWaitableTimerExW, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
        };
        // SAFETY: an unnamed timer owned by this value; null (older Windows) falls back to `sleep`.
        Waiter(unsafe {
            CreateWaitableTimerExW(
                std::ptr::null(),
                std::ptr::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        })
    }

    fn wait(&self, d: Duration) {
        use windows_sys::Win32::System::Threading::{SetWaitableTimer, WaitForSingleObject, INFINITE};
        if self.0.is_null() {
            std::thread::sleep(d);
            return;
        }
        // Negative: relative, in 100 ns units.
        let due = -((d.as_nanos() / 100) as i64);
        // SAFETY: a timer handle owned by `self`.
        unsafe {
            if SetWaitableTimer(self.0, &due, 0, None, std::ptr::null(), 0) == 0 {
                std::thread::sleep(d);
                return;
            }
            WaitForSingleObject(self.0, INFINITE);
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: closes the handle `new` made.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
        }
    }
}
