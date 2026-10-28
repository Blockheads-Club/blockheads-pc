//! Voice chat seams of the client loader (the voice core itself is mod-sdk's `bhmod-voice`).
//!
//! - [`features`]: what this client can do for voice, for `HELLO.features`.
//! - [`TalkKeys`]: push-to-talk key events from the window's input thread to the mod host on the game's main thread.

use bhpc_input::VoiceMode;
use std::sync::{Mutex, PoisonError};

/// `HELLO.features`: the client sends voice frames as unsequenced ENet packets on the game connection.
pub const FEATURE_VOICE_UNSEQUENCED: &str = "voice-unsequenced";

/// The voice entries of `HELLO.features` for a client that can (`unsequenced`) or cannot send unsequenced packets.
pub fn features(unsequenced: bool) -> Vec<String> {
    if unsequenced {
        vec![FEATURE_VOICE_UNSEQUENCED.to_string()]
    } else {
        Vec::new()
    }
}

/// One push-to-talk key going down or up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalkKey {
    pub mode: VoiceMode,
    pub pressed: bool,
}

/// Push-to-talk events waiting for the main thread, plus which modes are held right now.
#[derive(Default)]
pub struct TalkKeys {
    inner: Mutex<(Vec<TalkKey>, Vec<VoiceMode>)>,
}

impl TalkKeys {
    /// The input thread saw a key go down or up (repeats are already filtered by the mapper).
    pub fn post(&self, mode: VoiceMode, pressed: bool) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.0.push(TalkKey { mode, pressed });
        g.1.retain(|m| *m != mode);
        if pressed {
            g.1.push(mode);
        }
    }

    /// The events since the last call, in order (main thread, once per frame).
    pub fn take(&self) -> Vec<TalkKey> {
        std::mem::take(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner).0)
    }

    /// Modes whose key is held, in press order.
    pub fn held(&self) -> Vec<VoiceMode> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).1.clone()
    }
}
