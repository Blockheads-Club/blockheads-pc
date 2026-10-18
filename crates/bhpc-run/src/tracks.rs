//! `android.media.AudioTrack` over the audio mixer: the table of live tracks behind [`bhpc_jni::AudioHooks`].

use bhpc_audio::{track, AudioTrack, Mixer};
use bhpc_bionic::{sched, Shared};
use bhpc_cpu::Cpu;
use bhpc_jni::{AudioHooks, TrackOp};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How long `write` keeps retrying a full track before it reports a stall (0 bytes).
const STALL: Duration = Duration::from_secs(2);
/// Pause between retries; the baton goes to other guest threads meanwhile.
const RETRY_NS: u64 = 2_000_000;

/// Live tracks by id.
pub struct Tracks {
    mixer: Mixer,
    st: Shared,
    inner: Mutex<(u32, HashMap<u32, AudioTrack>)>,
}

impl Tracks {
    pub fn new(mixer: Mixer, st: Shared) -> Tracks {
        Tracks { mixer, st, inner: Mutex::new((0, HashMap::new())) }
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
        if op == TrackOp::Release {
            let gone = self.inner.lock().unwrap_or_else(PoisonError::into_inner).1.remove(&id);
            return if gone.is_some() { 0 } else { -3 };
        }
        let r = self.with(id, |t| match op {
            TrackOp::Play => t.play(),
            TrackOp::Pause => t.pause(),
            TrackOp::Stop => t.stop(),
            TrackOp::Flush => t.flush(),
            TrackOp::Volume(v) => t.set_volume(v),
            TrackOp::Release => Ok(()),
        });
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
                Some(Ok(0)) if !data.is_empty() && begun.elapsed() < STALL => sched::sleep(cpu, &self.st, RETRY_NS),
                Some(Ok(n)) => return n as i32,
            }
        }
    }

    fn head_position(&self, id: u32) -> i32 {
        self.with(id, |t| t.playback_head_position()).and_then(Result::ok).map_or(0, |p| p as i32)
    }
}
