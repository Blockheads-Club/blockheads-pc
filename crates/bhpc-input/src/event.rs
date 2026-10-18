//! Event vocabulary: what the host (SDL) tells us, and what the guest expects to receive.

/// Android `KeyEvent.KEYCODE_BACK`.
pub const KEYCODE_BACK: i32 = 4;

/// Mouse buttons we distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Keyboard modifier state at the time of an event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// The keys the mapper treats specially; everything else is [`HostKey::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKey {
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    F11,
    /// A letter (case-insensitive) or ASCII digit.
    Char(char),
    Other,
}

/// An event coming from the host window system. Coordinates are window pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
        x: f32,
        y: f32,
        mods: Mods,
    },
    /// Vertical wheel / trackpad pinch amount in notches (positive = away from the user = zoom in).
    Wheel {
        dy: f32,
        x: f32,
        y: f32,
    },
    Key {
        key: HostKey,
        pressed: bool,
        repeat: bool,
        mods: Mods,
    },
    /// Committed text (SDL_TEXTINPUT).
    TextInput(String),
    FocusGained,
    FocusLost,
    Minimized,
    Restored,
    /// New window size (also fullscreen switches): the drawable in pixels (the size of the surface the guest
    /// sees) and the window in logical pixels (the unit of mouse coordinates; differs on high-DPI displays).
    Resized {
        width: u32,
        height: u32,
        logical_width: u32,
        logical_height: u32,
    },
    Quit,
}

/// What the host application itself must do (not for the guest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostAction {
    ToggleFullscreen,
    Quit,
    /// A guest text field took focus: call `SDL_StartTextInput`.
    StartTextInput,
    /// The text field lost focus: call `SDL_StopTextInput`.
    StopTextInput,
}

/// `com.apportable.Lifecycle` static natives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    WillResignActive,
    DidEnterBackground,
    WillEnterForeground,
    DidBecomeActive,
}

impl Lifecycle {
    /// Name of the `com/apportable/Lifecycle` static native (signature `()V`).
    pub fn method(self) -> &'static str {
        match self {
            Lifecycle::WillResignActive => "applicationWillResignActive",
            Lifecycle::DidEnterBackground => "applicationDidEnterBackground",
            Lifecycle::WillEnterForeground => "applicationWillEnterForeground",
            Lifecycle::DidBecomeActive => "applicationDidBecomeActive",
        }
    }
}

/// An event for the guest. Each variant maps to exactly one Java native (see [`GuestEvent::target`]);
/// the JNI layer decodes the variant into JNI arguments and calls the guest function.
///
/// `time_ms` is the trailing `jlong` argument of the `Window` natives. The Java signatures are
/// `nativeTouchesBegin(IFFJ)V`, `nativeTouchesMove([I[F[FJ)V`, `nativeKeyDown(IIJI)Z`; the `J` is
/// believed to be the event time (`MotionEvent.getEventTime`). Verify at first integration.
#[derive(Debug, Clone, PartialEq)]
pub enum GuestEvent {
    TouchBegin {
        id: i32,
        x: f32,
        y: f32,
        time_ms: i64,
    },
    TouchEnd {
        id: i32,
        x: f32,
        y: f32,
        time_ms: i64,
    },
    TouchMove {
        ids: Vec<i32>,
        xs: Vec<f32>,
        ys: Vec<f32>,
        time_ms: i64,
    },
    TouchCancel {
        ids: Vec<i32>,
        xs: Vec<f32>,
        ys: Vec<f32>,
        time_ms: i64,
    },
    /// `nativeKeyDown(keyCode, unicode, time, metaState)`; returns true if the game consumed it.
    KeyDown {
        key_code: i32,
        unicode: i32,
        meta: i32,
        time_ms: i64,
    },
    KeyUp {
        key_code: i32,
        unicode: i32,
        meta: i32,
        time_ms: i64,
    },
    /// `Window.nativeOnWindowFocusChanged(Z)V`.
    WindowFocus(bool),
    /// Surface size in pixels: drives `GLSurfaceView.nativeOnSurfaceChanged` and `VerdeActivity.nativeRootGeometryChanged`.
    SurfaceChanged {
        width: u32,
        height: u32,
    },
    Lifecycle(Lifecycle),
    /// `TextField.shouldChangeCharactersInRange(field, location, length, replacement)Z`
    /// (UTF-16 units). The return value says whether the edit may be applied.
    TextShouldChange {
        field: i32,
        location: u32,
        length: u32,
        replacement: String,
    },
    /// `TextField.textDidChange(field)V`, sent after an accepted edit.
    TextDidChange {
        field: i32,
    },
    /// `TextField.shouldReturn(field)Z`.
    TextShouldReturn {
        field: i32,
    },
}

impl GuestEvent {
    /// The same event with every touch position passed through `f` (other events are returned unchanged).
    pub fn map_points(&self, f: &dyn Fn(f32, f32) -> (f32, f32)) -> GuestEvent {
        let each =
            |xs: &[f32], ys: &[f32]| -> (Vec<f32>, Vec<f32>) { xs.iter().zip(ys).map(|(x, y)| f(*x, *y)).unzip() };
        match self {
            GuestEvent::TouchBegin { id, x, y, time_ms } => {
                let (x, y) = f(*x, *y);
                GuestEvent::TouchBegin { id: *id, x, y, time_ms: *time_ms }
            }
            GuestEvent::TouchEnd { id, x, y, time_ms } => {
                let (x, y) = f(*x, *y);
                GuestEvent::TouchEnd { id: *id, x, y, time_ms: *time_ms }
            }
            GuestEvent::TouchMove { ids, xs, ys, time_ms } => {
                let (xs, ys) = each(xs, ys);
                GuestEvent::TouchMove { ids: ids.clone(), xs, ys, time_ms: *time_ms }
            }
            GuestEvent::TouchCancel { ids, xs, ys, time_ms } => {
                let (xs, ys) = each(xs, ys);
                GuestEvent::TouchCancel { ids: ids.clone(), xs, ys, time_ms: *time_ms }
            }
            other => other.clone(),
        }
    }
}

/// Java target of a [`GuestEvent`]: class (JNI form), method name and JNI signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub class: &'static str,
    pub method: &'static str,
    pub signature: &'static str,
}

const WINDOW: &str = "com/apportable/ui/Window";
const TEXT_FIELD: &str = "com/apportable/ui/TextField";
const LIFECYCLE: &str = "com/apportable/Lifecycle";
const GL_VIEW: &str = "com/apportable/opengl/GLSurfaceView";
const ACTIVITY: &str = "com/apportable/activity/VerdeActivity";

const fn t(class: &'static str, method: &'static str, signature: &'static str) -> Target {
    Target { class, method, signature }
}

impl GuestEvent {
    /// The field handle of the `TextField` events (they go to the focused field object), `None` for the others.
    pub fn text_field(&self) -> Option<i32> {
        match self {
            GuestEvent::TextShouldChange { field, .. }
            | GuestEvent::TextDidChange { field }
            | GuestEvent::TextShouldReturn { field } => Some(*field),
            _ => None,
        }
    }

    /// The Java natives this event is delivered to, in call order. `SurfaceChanged` is the only
    /// event that fans out to two natives; the `TextField` natives are instance methods whose
    /// first int argument is the field handle, all others are static.
    pub fn targets(&self) -> Vec<Target> {
        match self {
            GuestEvent::TouchBegin { .. } => vec![t(WINDOW, "nativeTouchesBegin", "(IFFJ)V")],
            GuestEvent::TouchEnd { .. } => vec![t(WINDOW, "nativeTouchesEnd", "(IFFJ)V")],
            GuestEvent::TouchMove { .. } => vec![t(WINDOW, "nativeTouchesMove", "([I[F[FJ)V")],
            GuestEvent::TouchCancel { .. } => vec![t(WINDOW, "nativeTouchesCancel", "([I[F[FJ)V")],
            GuestEvent::KeyDown { .. } => vec![t(WINDOW, "nativeKeyDown", "(IIJI)Z")],
            GuestEvent::KeyUp { .. } => vec![t(WINDOW, "nativeKeyUp", "(IIJI)Z")],
            GuestEvent::WindowFocus(_) => vec![t(WINDOW, "nativeOnWindowFocusChanged", "(Z)V")],
            GuestEvent::SurfaceChanged { .. } => {
                vec![t(GL_VIEW, "nativeOnSurfaceChanged", "(II)V"), t(ACTIVITY, "nativeRootGeometryChanged", "()V")]
            }
            GuestEvent::Lifecycle(l) => vec![t(LIFECYCLE, l.method(), "()V")],
            GuestEvent::TextShouldChange { .. } => {
                vec![t(TEXT_FIELD, "shouldChangeCharactersInRange", "(IIILjava/lang/String;)Z")]
            }
            GuestEvent::TextDidChange { .. } => vec![t(TEXT_FIELD, "textDidChange", "(I)V")],
            GuestEvent::TextShouldReturn { .. } => vec![t(TEXT_FIELD, "shouldReturn", "(I)Z")],
        }
    }
}

/// Receives what the mapper produces. Implemented by the JNI layer (`bhpc-jni`/`bhpc-run`).
pub trait EventSink {
    /// Deliver `ev` to the guest. The return value is the Java `boolean` result for the natives
    /// that have one (`TextShouldChange`, `TextShouldReturn`, `KeyDown`, `KeyUp`); return `true`
    /// for all other events.
    fn deliver(&mut self, ev: &GuestEvent) -> bool;
    /// Something for the host application itself.
    fn host_action(&mut self, action: HostAction);
}

/// A sink that records everything (tests, `--dump-input`, and a null-object for headless runs).
#[derive(Debug, Default)]
pub struct Recorder {
    pub events: Vec<GuestEvent>,
    pub actions: Vec<HostAction>,
    /// Answer given to every `deliver` (default `true` via [`Recorder::new`]).
    pub accept: bool,
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder { accept: true, ..Recorder::default() }
    }
}

impl EventSink for Recorder {
    fn deliver(&mut self, ev: &GuestEvent) -> bool {
        self.events.push(ev.clone());
        self.accept
    }
    fn host_action(&mut self, action: HostAction) {
        self.actions.push(action);
    }
}
