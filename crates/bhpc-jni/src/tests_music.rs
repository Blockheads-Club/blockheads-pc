//! `com.apportable.media.AudioPlayer` (the game's music) over the audio hooks.

use crate::tests::Env;
use crate::*;
use std::sync::{Arc, Mutex};

/// Records every music call; `ok.mp4` (absolute) opens as player 7.
struct RecordingMusic(Mutex<Vec<String>>);

impl AudioHooks for RecordingMusic {
    fn min_buffer_size(&self, _rate: i32, _ch: i32, _enc: i32) -> i32 {
        0
    }
    fn create(&self, _rate: i32, _ch: i32, _enc: i32, _buf: i32) -> Option<u32> {
        None
    }
    fn control(&self, _id: u32, _op: TrackOp) -> i32 {
        0
    }
    fn write(&self, _cpu: &mut bhpc_cpu::Cpu, _id: u32, _data: &[u8]) -> i32 {
        0
    }
    fn head_position(&self, _id: u32) -> i32 {
        0
    }
    fn music_open(&self, path: &str, asset: Option<Vec<u8>>) -> Option<u32> {
        let asset = asset.map(|a| String::from_utf8(a).unwrap());
        self.0.lock().unwrap().push(format!("open {path} {asset:?}"));
        (asset.is_some() || path == "/ok.mp4").then_some(7)
    }
    fn music(&self, id: u32, op: MusicOp) -> i32 {
        self.0.lock().unwrap().push(format!("{id} {op:?}"));
        42
    }
    fn music_suspend(&self, suspend: bool) {
        self.0.lock().unwrap().push(format!("suspend {suspend}"));
    }
}

fn int(v: JResult) -> i32 {
    v.unwrap().as_int()
}

const CLASS: &str = "com/apportable/media/AudioPlayer";

#[test]
fn audio_player_delegates_to_the_music_hooks() {
    let rec = Arc::new(RecordingMusic(Mutex::new(Vec::new())));
    let r2 = rec.clone();
    let mut e = Env::with(move |c| c.audio = r2);
    let p = e.new_obj(CLASS, "(Landroid/content/Context;)V", &[Value::Ref(None)]);
    let open = |e: &mut Env, path: &str| {
        let s = e.vm.new_string(path);
        int(e.call(p, "initAudio", "(Ljava/lang/String;)I", &[Value::Ref(Some(s))]))
    };
    assert_eq!(open(&mut e, "hello.txt"), 7, "a relative path is an APK asset");
    assert_eq!(open(&mut e, "/ok.mp4"), 7, "an absolute path is left to the host");
    assert_eq!(open(&mut e, "missing.mp4"), -1);
    assert!(e.log_text().contains("AudioPlayer.initAudio(\"missing.mp4\") -> None"));
    let id = Value::Int(7);
    for m in ["prepareAudio", "startAudio", "pauseAudio", "releaseAudio"] {
        e.call(p, m, "(I)V", &[id]).unwrap();
    }
    for m in ["getDuration", "getCurrentPosition", "isAudioPlaying"] {
        assert_eq!(int(e.call(p, m, "(I)I", &[id])), 42, "{m}");
    }
    e.call(p, "seekTo", "(II)V", &[Value::Int(1500), id]).unwrap();
    e.call(p, "setNumberOfLoops", "(II)V", &[Value::Int(-1), id]).unwrap();
    e.call(p, "setVolume", "(FFI)V", &[Value::Float(0.25), Value::Float(0.5), id]).unwrap();
    // An id the player never handed out (`initAudio` failed) does not reach the host.
    assert_eq!(int(e.call(p, "isAudioPlaying", "(I)I", &[Value::Int(-1)])), -1);
    e.scall(CLASS, "lifecycleSuspend", "()V", &[]).unwrap();
    e.scall(CLASS, "lifecycleResume", "()V", &[]).unwrap();
    let log = rec.0.lock().unwrap().join("|");
    assert_eq!(
        log,
        [
            "open hello.txt Some(\"hello assets\")",
            "open /ok.mp4 None",
            "open missing.mp4 None",
            "7 Prepare",
            "7 Start",
            "7 Pause",
            "7 Release",
            "7 Duration",
            "7 Position",
            "7 IsPlaying",
            "7 SeekTo(1500)",
            "7 Loops(-1)",
            "7 Volume(0.25, 0.5)",
            "suspend true",
            "suspend false",
        ]
        .join("|")
    );
}

#[test]
fn without_a_music_back_end_nothing_opens() {
    let mut e = Env::new();
    let p = e.new_obj(CLASS, "(Landroid/content/Context;)V", &[Value::Ref(None)]);
    let s = e.vm.new_string("hello.txt");
    assert_eq!(int(e.call(p, "initAudio", "(Ljava/lang/String;)I", &[Value::Ref(Some(s))])), -1);
    assert_eq!(int(e.call(p, "isAudioPlaying", "(I)I", &[Value::Int(1)])), -1);
    e.scall(CLASS, "lifecycleSuspend", "()V", &[]).unwrap();
}
