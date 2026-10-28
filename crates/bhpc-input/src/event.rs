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
    /// A function key, `F(11)` is F11.
    F(u8),
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
        mods: Mods,
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
    /// A game controller.
    Pad(crate::pad::PadEvent),
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
    /// Hide or show the game's own on-screen buttons (its HUD).
    ToggleGameButtons,
    /// Zoom the game's camera one step in / out, or to its next zoom level.
    ZoomIn,
    ZoomOut,
    ZoomCycle,
    /// The wheel in a world (see [`crate::InputMapper::set_world_zoom`]): zoom `steps` whole notches (positive = closer)
    /// around `at`, in content pixels (rounded).
    WheelZoom {
        steps: i32,
        at: (i32, i32),
    },
    /// Open (or close) the game's pause menu.
    Pause,
    /// Open the game's chat (like Minecraft's T).
    OpenChat,
    /// A push-to-talk key went down (`pressed`) or up; for the voice chat host (key repeat is filtered out).
    Voice {
        mode: VoiceMode,
        pressed: bool,
    },
}

/// The voice chat talk modes that have a push-to-talk key of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VoiceMode {
    /// Players nearby.
    Proximity,
    /// The player's channel (team).
    Channel,
    /// Everyone on the server.
    Global,
    /// Admin broadcast.
    Broadcast,
}

impl VoiceMode {
    /// All modes, in the order of [`Bindings::voice`].
    pub const ALL: [VoiceMode; 4] = [VoiceMode::Proximity, VoiceMode::Channel, VoiceMode::Global, VoiceMode::Broadcast];

    /// Lower-case name (`proximity`, `channel`, `global`, `broadcast`), as the voice protocol and settings use.
    pub fn name(self) -> &'static str {
        match self {
            VoiceMode::Proximity => "proximity",
            VoiceMode::Channel => "channel",
            VoiceMode::Global => "global",
            VoiceMode::Broadcast => "broadcast",
        }
    }
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
const GL_VIEW: &str = "com/apportable/gl/GLSurfaceView";
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
    /// The current text of the focused field as the guest holds it (`None`: unknown), read after each edit so the
    /// host's copy follows what the game did with it.
    fn field_text(&mut self) -> Option<String> {
        None
    }
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

/// The thing a binding is made of: a mouse button, the wheel or a key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    Mouse(MouseButton),
    Wheel,
    Key(HostKey),
}

/// Modifiers plus an input. It fires when the input happens with at least these modifiers held, and of several
/// matching bindings the one with the most modifiers wins (so `Ctrl+Left` beats `Left`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trigger {
    pub mods: Mods,
    pub input: Input,
}

impl Trigger {
    /// A trigger without modifiers.
    pub fn plain(input: Input) -> Trigger {
        Trigger { mods: Mods::default(), input }
    }

    /// Number of modifiers the trigger asks for.
    pub fn weight(&self) -> usize {
        usize::from(self.mods.ctrl) + usize::from(self.mods.shift) + usize::from(self.mods.alt)
    }

    /// True when `input` happened with (at least) the trigger's modifiers held.
    pub fn fires(&self, input: Input, held: Mods) -> bool {
        self.input == input
            && (held.ctrl || !self.mods.ctrl)
            && (held.shift || !self.mods.shift)
            && (held.alt || !self.mods.alt)
    }
}

/// What each mapped action is bound to. The default is the hard-wired desktop mapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bindings {
    /// One-finger touch / drag.
    pub touch: Trigger,
    /// Two-finger pan.
    pub pan: Trigger,
    /// Wheel pinch (zoom).
    pub zoom: Trigger,
    /// Drag pinch.
    pub pinch: Trigger,
    /// Android back key.
    pub back: Trigger,
    /// Toggle fullscreen (a host action, not for the guest).
    pub fullscreen: Trigger,
    /// Hide / show the game's on-screen buttons (a host action; ignored while a text field has focus).
    pub hide_ui: Trigger,
    /// Open the chat (a host action; ignored while a text field has focus).
    pub chat: Trigger,
    /// Push-to-talk per [`VoiceMode`] (in [`VoiceMode::ALL`] order; host actions, ignored while a text field has focus).
    pub voice: [Trigger; 4],
}

impl Default for Bindings {
    fn default() -> Bindings {
        Bindings {
            touch: Trigger::plain(Input::Mouse(MouseButton::Left)),
            pan: Trigger::plain(Input::Mouse(MouseButton::Right)),
            zoom: Trigger::plain(Input::Wheel),
            pinch: Trigger { mods: Mods { ctrl: true, ..Mods::default() }, input: Input::Mouse(MouseButton::Left) },
            back: Trigger::plain(Input::Key(HostKey::Escape)),
            fullscreen: Trigger::plain(Input::Key(HostKey::F(11))),
            hide_ui: Trigger::plain(Input::Key(HostKey::Char('h'))),
            chat: Trigger::plain(Input::Key(HostKey::Char('t'))),
            // V, B, G as planned; broadcast is J because H already hides the game's buttons.
            voice: ['v', 'b', 'g', 'j'].map(|c| Trigger::plain(Input::Key(HostKey::Char(c)))),
        }
    }
}
