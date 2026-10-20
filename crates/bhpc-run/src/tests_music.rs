//! The game's music players (`AudioPlayer`) over the mixer.
use crate::music::Music;
use crate::new_env;
use crate::tracks::Tracks;
use bhpc_audio::Mixer;
use bhpc_jni::{AudioHooks, MusicOp};
use bhpc_vfs::memfs::{MemFs, Node};
use std::sync::Arc;
use std::time::{Duration, Instant};

const M4A: &[u8] = include_bytes!("../../bhpc-audio/src/music_fixtures/tone.m4a");

fn music() -> (Music, Mixer) {
    let mut fs = MemFs::new();
    fs.insert("/apk/tone.m4a", Node::Bytes(M4A.into()));
    fs.insert("/apk/empty.m4a", Node::Bytes(Arc::from(&[][..])));
    let mixer = Mixer::new(44100);
    (Music::new(mixer.clone(), Arc::new(fs)), mixer)
}

/// Peak of the next `ms` of mixed output.
fn peak(mixer: &Mixer, ms: usize) -> i16 {
    let mut out = vec![0i16; 44100 * ms / 1000 * 2];
    mixer.fill(&mut out);
    out.iter().map(|s| s.saturating_abs()).max().unwrap_or(0)
}

#[test]
fn opens_assets_and_guest_paths() {
    let (m, _mixer) = music();
    assert_eq!(m.open("GameResources/x.mp4", Some(M4A.to_vec())), Some(1), "asset bytes");
    assert_eq!(m.open("/apk/tone.m4a", None), Some(2), "guest file");
    assert_eq!(m.open("file:///apk/tone.m4a", Some(Vec::new())), Some(3), "an empty asset falls back to the path");
    assert_eq!(m.open("/apk/empty.m4a", None), None, "empty file");
    assert_eq!(m.open("/apk/missing.m4a", None), None);
}

#[test]
fn plays_pauses_and_releases() {
    let (m, mixer) = music();
    let id = m.open("/apk/tone.m4a", None).unwrap();
    assert_eq!(m.op(id, MusicOp::Start), -1, "not prepared yet");
    assert_eq!(m.op(id, MusicOp::IsPlaying), 0);
    for op in [MusicOp::Loops(999), MusicOp::Volume(1.0, 1.0), MusicOp::Prepare, MusicOp::Start] {
        assert_eq!(m.op(id, op), 0, "{op:?}");
    }
    assert_eq!(m.op(id, MusicOp::IsPlaying), 1);
    assert_eq!(m.op(id, MusicOp::SeekTo(-5)), 0, "clamped to the start");
    assert!(m.op(id, MusicOp::Duration) > 0);
    assert!(peak(&mixer, 100) > 1000, "the tone reaches the mixer");
    assert!(m.op(id, MusicOp::Position) >= 0);
    assert_eq!(m.op(id, MusicOp::Loops(-1)), 0);
    assert_eq!(m.op(id, MusicOp::Pause), 0);
    assert_eq!(m.op(id, MusicOp::IsPlaying), 0);
    assert_eq!(m.op(id, MusicOp::Release), 0);
    assert_eq!(m.op(id, MusicOp::Release), -1, "already gone");
    assert_eq!(m.op(99, MusicOp::Duration), -1, "unknown player");
}

#[test]
fn a_finished_track_stops_on_its_own() {
    let (m, mixer) = music();
    let id = m.open("/apk/tone.m4a", None).unwrap();
    m.op(id, MusicOp::Prepare);
    m.op(id, MusicOp::Start);
    let begun = Instant::now();
    // The pump thread notices the end once the mixer has played everything.
    while m.op(id, MusicOp::IsPlaying) == 1 && begun.elapsed() < Duration::from_secs(10) {
        peak(&mixer, 200);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(m.op(id, MusicOp::IsPlaying), 0);
}

#[test]
fn suspend_pauses_and_resume_restarts_what_played() {
    let (m, _mixer) = music();
    let (a, b, c) = (
        m.open("/apk/tone.m4a", None).unwrap(),
        m.open("/apk/tone.m4a", None).unwrap(),
        m.open("/apk/tone.m4a", None).unwrap(),
    );
    for id in [a, b, c] {
        m.op(id, MusicOp::Prepare);
    }
    m.op(a, MusicOp::Start);
    m.op(c, MusicOp::Start);
    m.suspend(true);
    assert_eq!([a, b, c].map(|id| m.op(id, MusicOp::IsPlaying)), [0, 0, 0]);
    m.op(c, MusicOp::Release);
    m.suspend(false);
    assert_eq!(m.op(a, MusicOp::IsPlaying), 1, "resumed");
    assert_eq!(m.op(b, MusicOp::IsPlaying), 0, "was not playing");
}

#[test]
fn the_audio_hooks_serve_music_too() {
    let (_cpu, st) = new_env(Box::new(std::io::sink()), None).unwrap();
    let t = Tracks::new(Mixer::new(44100), st);
    let id = t.music_open("GameResources/x.mp4", Some(M4A.to_vec())).unwrap();
    assert_eq!(t.music(id, MusicOp::Prepare), 0);
    assert_eq!(t.music(id, MusicOp::Start), 0);
    t.music_suspend(true);
    assert_eq!(t.music(id, MusicOp::IsPlaying), 0);
    t.music_suspend(false);
    assert_eq!(t.music(id, MusicOp::IsPlaying), 1);
    assert_eq!(t.music_open("/nowhere.mp4", None), None);
}
