//! Input mapping (PLAN 1.7): host mouse, keyboard and window events in, guest events out.
//!
//! The crate is pure: no SDL, no JNI, no guest memory. The host feeds [`HostEvent`]s to an
//! [`InputMapper`]; the mapper emits [`GuestEvent`]s into an [`EventSink`]. Each `GuestEvent` names
//! the Java native it belongs to ([`GuestEvent::targets`]) so the JNI layer can turn it into a
//! guest call without further knowledge of the mapping.
//!
//! | Host input | Guest event |
//! |---|---|
//! | left button down / drag / up | `Window.nativeTouchesBegin` / `Move` / `End`, pointer 0 |
//! | right button drag | two-finger pan (pointers 0 and 1 move together) |
//! | wheel | two-finger pinch around the cursor; ends after an idle timeout ([`InputMapper::tick`]) |
//! | Ctrl + left drag | pinch around the press point (right/up = spread) |
//! | Esc | `Window.nativeKeyDown/Up` with `KEYCODE_BACK` |
//! | other keys | `Window.nativeKeyDown/Up` with Android key codes, unless a text field has focus |
//! | text while a field has focus | `TextField.shouldChangeCharactersInRange`, `textDidChange`, `shouldReturn` |
//! | focus / minimise / restore | `nativeOnWindowFocusChanged` and the `Lifecycle` natives |
//! | resize | `SurfaceChanged` (`nativeOnSurfaceChanged` + `nativeRootGeometryChanged`) |
//! | F11 / window close | [`HostAction::ToggleFullscreen`] / [`HostAction::Quit`] |
//!
//! The long press of the game is a held left button; it needs no mapping.

pub mod event;
pub mod keymap;
mod mapper;
pub mod text;

pub use event::{
    EventSink, GuestEvent, HostAction, HostEvent, HostKey, Lifecycle, Mods, MouseButton, Recorder, Target, KEYCODE_BACK,
};
pub use mapper::{Config, InputMapper};

#[cfg(test)]
mod tests;
