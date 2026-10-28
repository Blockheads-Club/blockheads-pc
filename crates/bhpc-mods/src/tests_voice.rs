//! Voice seams: HELLO features and push-to-talk events.

use crate::voice::*;
use bhpc_input::VoiceMode;

#[test]
fn unsequenced_feature() {
    assert_eq!(features(true), vec!["voice-unsequenced".to_string()]);
    assert!(features(false).is_empty());
}

#[test]
fn talk_keys_queue_and_track_what_is_held() {
    let k = TalkKeys::default();
    k.post(VoiceMode::Proximity, true);
    k.post(VoiceMode::Global, true);
    k.post(VoiceMode::Proximity, false);
    assert_eq!(k.held(), vec![VoiceMode::Global]);
    let ev = k.take();
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[2], TalkKey { mode: VoiceMode::Proximity, pressed: false });
    assert!(k.take().is_empty());
    k.post(VoiceMode::Global, false);
    assert!(k.held().is_empty());
}
