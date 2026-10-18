//! `AudioTrack` back end over the mixer.
use crate::new_env;
use crate::tracks::Tracks;
use bhpc_audio::Mixer;
use bhpc_jni::{AudioHooks, TrackOp};
use std::time::Duration;

fn rig() -> (Box<bhpc_cpu::Cpu>, Tracks, Mixer) {
    let (cpu, st) = new_env(Box::new(std::io::sink()), None).unwrap();
    let mixer = Mixer::new(44100);
    (cpu, Tracks::new(mixer.clone(), st).with_stall(Duration::from_millis(30)), mixer)
}

#[test]
fn tracks_play_pause_and_release() {
    let (mut cpu, t, mixer) = rig();
    assert_eq!(t.min_buffer_size(44100, 3, 2), 7056);
    assert_eq!(t.min_buffer_size(1, 3, 2), -2, "bad rate");
    assert_eq!(t.create(1, 3, 2, 100), None, "bad rate");
    let id = t.create(44100, 3, 2, 1 << 16).unwrap();
    assert_eq!(t.control(id, TrackOp::Play), 0);
    assert_eq!(t.write(&mut cpu, id, &vec![1u8; 4000]), 4000);
    assert_eq!(t.write(&mut cpu, id, &[]), 0, "nothing to queue");
    let mut out = vec![0i16; 2000];
    mixer.fill(&mut out);
    assert!(t.head_position(id) > 0, "the mixer consumed frames");
    for op in [TrackOp::Pause, TrackOp::Play, TrackOp::Flush, TrackOp::Volume(0.5), TrackOp::Stop] {
        assert_eq!(t.control(id, op), 0);
    }
    assert_eq!(t.control(77, TrackOp::Play), -3, "unknown track");
    assert_eq!(t.write(&mut cpu, 77, &[0; 4]), -3);
    assert_eq!(t.head_position(77), 0);
    assert_eq!(t.control(id, TrackOp::Release), 0);
    assert_eq!(t.control(id, TrackOp::Release), -3);
    assert_eq!(t.write(&mut cpu, id, &[0; 4]), -3, "released");
}

#[test]
fn a_full_track_stalls_after_the_retry_window() {
    let (mut cpu, t, _mixer) = rig();
    // 8 frames of room, nothing drains it: a 4000 byte write cannot fit and gives up after the window.
    let id = t.create(44100, 3, 2, 32).unwrap();
    assert_eq!(t.write(&mut cpu, id, &vec![0u8; 4000]), 4000, "an empty queue always accepts one buffer");
    let begun = std::time::Instant::now();
    assert_eq!(t.write(&mut cpu, id, &vec![0u8; 4000]), 0);
    assert!(begun.elapsed() >= Duration::from_millis(30));
}
