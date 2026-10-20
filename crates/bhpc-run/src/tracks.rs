//! `android.media.AudioTrack` over the audio mixer: the table of live tracks behind [`bhpc_jni::AudioHooks`], which
//! also serves the music players ([`Music`]).

use crate::music::Music;
use bhpc_audio::{track, AudioTrack, Mixer};
use bhpc_bionic::{lock, sched, Shared};
use bhpc_cpu::Cpu;
use bhpc_jni::{AudioHooks, MusicOp, TrackOp};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// Default for how long `write` keeps retrying a full track before it reports a stall (0 bytes).
const STALL: Duration = Duration::from_secs(2);
/// Pause between retries; the baton goes to other guest threads meanwhile.
const RETRY_NS: u64 = 2_000_000;

/// Live tracks by id.
pub struct Tracks {
    mixer: Mixer,
    st: Shared,
    stall: Duration,
    inner: Mutex<(u32, HashMap<u32, AudioTrack>)>,
    music: Music,
}

impl Tracks {
    pub fn new(mixer: Mixer, st: Shared) -> Tracks {
        // Music files are read through the guest's file system, like every other path the game names.
        let music = Music::new(mixer.clone(), lock(&st).vfs.fs().clone());
        Tracks { mixer, st, stall: STALL, inner: Mutex::new((0, HashMap::new())), music }
    }

    /// Change how long a full track is retried before `write` gives up.
    pub fn with_stall(mut self, stall: Duration) -> Tracks {
        self.stall = stall;
        self
    }

    fn with<R>(&self, id: u32, f: impl FnOnce(&mut AudioTrack) -> R) -> Option<R> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).1.get_mut(&id).map(f)
    }
}

impl AudioHooks for Tracks {
    fn min_buffer_size(&self, rate: i32, channel_config: i32, encoding: i32) -> i32 {
        track::min_buffer_size(rate, channel_config, encoding).unwrap_or(-2)
    }

    fn create(&self, rate: i32, channel_config: i32, encoding: i32, buffer_bytes: i32) -> Option<u32> {
        let mut t = AudioTrack::new(&self.mixer, rate, channel_config, encoding, buffer_bytes).ok()?;
        // Writes never block inside the mixer: `write` below retries so the baton can move on meanwhile.
        t.set_write_timeout(Duration::ZERO);
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.0 += 1;
        let id = g.0;
        g.1.insert(id, t);
        Some(id)
    }

    fn control(&self, id: u32, op: TrackOp) -> i32 {
        let r = match op {
            TrackOp::Release => {
                let gone = self.inner.lock().unwrap_or_else(PoisonError::into_inner).1.remove(&id);
                return if gone.is_some() { 0 } else { -3 };
            }
            TrackOp::Play => self.with(id, |t| t.play()),
            TrackOp::Pause => self.with(id, |t| t.pause()),
            TrackOp::Stop => self.with(id, |t| t.stop()),
            TrackOp::Flush => self.with(id, |t| t.flush()),
            TrackOp::Volume(v) => self.with(id, |t| t.set_volume(v)),
        };
        match r {
            Some(Ok(())) => 0,
            _ => -3,
        }
    }

    fn write(&self, cpu: &mut Cpu, id: u32, data: &[u8]) -> i32 {
        let begun = std::time::Instant::now();
        loop {
            match self.with(id, |t| t.write(data)) {
                None | Some(Err(_)) => return -3,
                Some(Ok(0)) if !data.is_empty() && begun.elapsed() < self.stall => {
                    sched::sleep(cpu, &self.st, RETRY_NS)
                }
                Some(Ok(n)) => return n as i32,
            }
        }
    }

    fn head_position(&self, id: u32) -> i32 {
        self.with(id, |t| t.playback_head_position()).and_then(Result::ok).map_or(0, |p| p as i32)
    }

    fn music_open(&self, path: &str, asset: Option<Vec<u8>>) -> Option<u32> {
        self.music.open(path, asset)
    }

    fn music(&self, id: u32, op: MusicOp) -> i32 {
        self.music.op(id, op)
    }

    fn music_suspend(&self, suspend: bool) {
        self.music.suspend(suspend)
    }
}
