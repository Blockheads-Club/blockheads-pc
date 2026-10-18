//! A host model of `android.media.MediaPlayer` playing into a mixer stream.
//!
//! The player does no decoding on its caller's thread beyond `prepare`/`seekTo`; instead
//! [`MediaPlayer::pump`] keeps about one second of decoded audio queued in the mixer and detects
//! the end of playback. Call it from any thread you like, or use [`MediaPlayer::spawn_pump`].
//! Listener callbacks run on whichever thread called the method that produced them (the pump
//! thread for completion and errors); `bhpc-jni` re-dispatches them to the guest.

use super::decode::Decoder;
use super::source::SliceSource;
use super::MusicError;
use crate::mixer::{Mixer, PlayState, StreamId};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;
use symphonia::core::io::MediaSource;

/// `MediaPlayer.MEDIA_ERROR_UNKNOWN`.
pub const MEDIA_ERROR_UNKNOWN: i32 = 1;

/// Android's player states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Initialized,
    Prepared,
    Started,
    Paused,
    Stopped,
    PlaybackCompleted,
    Error,
    End,
}

/// What the `onXxx` listener methods of the game's `MediaPlayer` bridge receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEvent {
    Prepared,
    Completion,
    SeekComplete,
    Error { what: i32, extra: i32 },
}

type Listener = Arc<dyn Fn(PlayerEvent) + Send + Sync>;

struct Inner {
    mixer: Mixer,
    state: PlayerState,
    pending: Option<(Box<dyn MediaSource>, Option<String>)>,
    decoder: Option<Decoder>,
    stream: Option<StreamId>,
    /// The decoder has no more packets (and we are not looping).
    eof: bool,
    looping: bool,
    /// Where the stream's frame 0 sits in the track, in milliseconds.
    base_ms: u32,
    volume: (f32, f32),
    listener: Option<Listener>,
}

impl Inner {
    fn drop_stream(&mut self) {
        if let Some(id) = self.stream.take() {
            self.mixer.destroy_stream(id);
        }
    }

    fn gain(&self) -> f32 {
        (self.volume.0 + self.volume.1) / 2.0
    }

    fn fail(&mut self, events: &mut Vec<PlayerEvent>) {
        self.state = PlayerState::Error;
        if let Some(id) = self.stream {
            let _ = self.mixer.set_state(id, PlayState::Paused);
        }
        events.push(PlayerEvent::Error { what: MEDIA_ERROR_UNKNOWN, extra: 0 });
    }

    /// Top the mixer queue up to about a second of audio. Only called with a stream and decoder.
    fn refill(&mut self) -> Result<(), MusicError> {
        let id = self.stream.expect("refill needs a stream");
        let dec = self.decoder.as_mut().expect("refill needs a decoder");
        let target = dec.rate() as usize;
        while !self.eof && self.mixer.queued_frames(id).unwrap_or(target) < target {
            match dec.next_frames()? {
                Some(frames) => {
                    let _ = self.mixer.enqueue(id, frames);
                }
                // Wrap around; the next pump carries on from the start (and does not spin on an
                // empty stream).
                None if self.looping => {
                    dec.seek_ms(0)?;
                    break;
                }
                None => self.eof = true,
            }
        }
        Ok(())
    }

    /// Seek the decoder and restart the mixer stream there, keeping its play state. Only called with
    /// a stream and decoder.
    fn seek(&mut self, ms: u32) -> Result<(), MusicError> {
        let id = self.stream.expect("seek needs a stream");
        self.decoder.as_mut().expect("seek needs a decoder").seek_ms(ms)?;
        let was = self.mixer.state(id).unwrap_or(PlayState::Stopped);
        let _ = self.mixer.clear(id);
        let _ = self.mixer.set_state(id, PlayState::Stopped);
        let _ = self.mixer.set_state(id, was);
        self.base_ms = ms;
        self.eof = false;
        self.refill()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.drop_stream();
    }
}

/// The player. Cloning shares the same player.
#[derive(Clone)]
pub struct MediaPlayer {
    inner: Arc<Mutex<Inner>>,
}

impl MediaPlayer {
    /// A new player in the `Idle` state, playing into `mixer`.
    pub fn new(mixer: Mixer) -> MediaPlayer {
        MediaPlayer {
            inner: Arc::new(Mutex::new(Inner {
                mixer,
                state: PlayerState::Idle,
                pending: None,
                decoder: None,
                stream: None,
                eof: false,
                looping: false,
                base_ms: 0,
                volume: (1.0, 1.0),
                listener: None,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `f` under the lock, then deliver the events it produced outside the lock.
    fn run<T>(&self, f: impl FnOnce(&mut Inner, &mut Vec<PlayerEvent>) -> T) -> T {
        let mut events = Vec::new();
        let (out, listener) = {
            let mut g = self.lock();
            let out = f(&mut g, &mut events);
            (out, g.listener.clone())
        };
        for e in events {
            if let Some(l) = &listener {
                l(e);
            }
        }
        out
    }

    /// Install the callback for `onPrepared`, `onCompletion`, `onSeekComplete` and `onError`.
    pub fn set_listener(&self, l: impl Fn(PlayerEvent) + Send + Sync + 'static) {
        self.lock().listener = Some(Arc::new(l));
    }

    pub fn state(&self) -> PlayerState {
        self.lock().state
    }

    /// The mixer stream carrying the music, once prepared.
    pub fn stream_id(&self) -> Option<StreamId> {
        self.lock().stream
    }

    // ---------- setDataSource ----------

    /// `setDataSource` from any seekable byte source. `ext` is an optional extension hint.
    pub fn set_data_source_reader(&self, src: Box<dyn MediaSource>, ext: Option<&str>) -> Result<(), MusicError> {
        self.run(|s, _| {
            if s.state != PlayerState::Idle {
                return Err(MusicError::IllegalState("setDataSource outside Idle"));
            }
            s.pending = Some((src, ext.map(str::to_owned)));
            s.state = PlayerState::Initialized;
            Ok(())
        })
    }

    /// `setDataSource(path)`.
    pub fn set_data_source_path(&self, path: &Path) -> Result<(), MusicError> {
        let f = File::open(path).map_err(|e| MusicError::Io(format!("{}: {e}", path.display())))?;
        let ext = path.extension().and_then(|e| e.to_str());
        self.set_data_source_reader(Box::new(f), ext)
    }

    /// `setDataSource(fd, offset, length)`. A negative `length` means "to the end of the file".
    pub fn set_data_source_fd(&self, file: File, offset: u64, length: i64) -> Result<(), MusicError> {
        let src = SliceSource::new(file, offset, u64::try_from(length).ok())
            .map_err(|e| MusicError::Io(format!("fd: {e}")))?;
        self.set_data_source_reader(Box::new(src), None)
    }

    /// `setDataSource` from memory.
    pub fn set_data_source_bytes(&self, bytes: Vec<u8>) -> Result<(), MusicError> {
        let src = SliceSource::new(std::io::Cursor::new(bytes), 0, None).expect("a cursor cannot fail to seek");
        self.set_data_source_reader(Box::new(src), None)
    }

    // ---------- prepare / transport ----------

    /// `prepare()`: open the stream and queue the first second of audio.
    pub fn prepare(&self) -> Result<(), MusicError> {
        self.run(|s, ev| {
            if !matches!(s.state, PlayerState::Initialized | PlayerState::Stopped) {
                return Err(MusicError::IllegalState("prepare outside Initialized/Stopped"));
            }
            match s.prepare_inner() {
                Ok(()) => {
                    s.state = PlayerState::Prepared;
                    Ok(())
                }
                Err(e) => {
                    s.fail(ev);
                    Err(e)
                }
            }
        })
    }

    /// `prepareAsync()` for hosts that deliver `onPrepared` themselves: prepares synchronously and
    /// emits [`PlayerEvent::Prepared`] (or an error event) to the listener.
    pub fn prepare_async(&self) -> Result<(), MusicError> {
        let r = self.prepare();
        if r.is_ok() {
            self.run(|_, ev| ev.push(PlayerEvent::Prepared));
        }
        r
    }

    pub fn start(&self) -> Result<(), MusicError> {
        self.run(|s, _| {
            match s.state {
                PlayerState::Started => return Ok(()),
                PlayerState::Prepared | PlayerState::Paused => {}
                PlayerState::PlaybackCompleted => s.seek(0)?,
                _ => return Err(MusicError::IllegalState("start before prepare")),
            }
            let id = s.stream.expect("prepared players have a stream");
            let _ = s.mixer.set_state(id, PlayState::Playing);
            s.state = PlayerState::Started;
            Ok(())
        })
    }

    pub fn pause(&self) -> Result<(), MusicError> {
        self.run(|s, _| match s.state {
            PlayerState::Paused => Ok(()),
            PlayerState::Started => {
                let id = s.stream.expect("started players have a stream");
                let _ = s.mixer.set_state(id, PlayState::Paused);
                s.state = PlayerState::Paused;
                Ok(())
            }
            _ => Err(MusicError::IllegalState("pause while not started")),
        })
    }

    /// `stop()`: the player must be prepared again before it can start.
    pub fn stop(&self) -> Result<(), MusicError> {
        self.run(|s, _| match s.state {
            PlayerState::Prepared | PlayerState::Started | PlayerState::Paused | PlayerState::PlaybackCompleted => {
                let id = s.stream.expect("prepared players have a stream");
                let _ = s.mixer.clear(id);
                let _ = s.mixer.set_state(id, PlayState::Stopped);
                s.state = PlayerState::Stopped;
                Ok(())
            }
            PlayerState::Stopped => Ok(()),
            _ => Err(MusicError::IllegalState("stop before prepare")),
        })
    }

    /// `seekTo(msec)`; emits [`PlayerEvent::SeekComplete`].
    pub fn seek_to(&self, ms: u32) -> Result<(), MusicError> {
        self.run(|s, ev| {
            if !matches!(
                s.state,
                PlayerState::Prepared | PlayerState::Started | PlayerState::Paused | PlayerState::PlaybackCompleted
            ) {
                return Err(MusicError::IllegalState("seekTo before prepare"));
            }
            let was_done = s.state == PlayerState::PlaybackCompleted;
            if let Err(e) = s.seek(ms) {
                s.fail(ev);
                return Err(e);
            }
            if was_done {
                s.state = PlayerState::Paused;
            }
            ev.push(PlayerEvent::SeekComplete);
            Ok(())
        })
    }

    pub fn set_looping(&self, looping: bool) {
        let mut s = self.lock();
        s.looping = looping;
        if looping {
            // A stream already at its end may be revived by the next pump.
            s.eof = s.eof && s.state == PlayerState::PlaybackCompleted;
        }
    }

    pub fn is_looping(&self) -> bool {
        self.lock().looping
    }

    /// `setVolume(left, right)`. The mixer has one gain per stream, so the two are averaged.
    pub fn set_volume(&self, left: f32, right: f32) {
        let mut s = self.lock();
        s.volume = (left.max(0.0), right.max(0.0));
        if let Some(id) = s.stream {
            let _ = s.mixer.set_gain(id, s.gain());
        }
    }

    pub fn is_playing(&self) -> bool {
        self.lock().state == PlayerState::Started
    }

    /// `getDuration()` in milliseconds, -1 when unknown or not prepared.
    pub fn duration_ms(&self) -> i32 {
        let s = self.lock();
        s.decoder.as_ref().and_then(|d| d.duration_ms()).map_or(-1, |d| d as i32)
    }

    /// `getCurrentPosition()` in milliseconds.
    pub fn position_ms(&self) -> i32 {
        let s = self.lock();
        let (Some(id), Some(dec)) = (s.stream, s.decoder.as_ref()) else {
            return 0;
        };
        let dur = dec.duration_ms();
        if s.state == PlayerState::PlaybackCompleted {
            return dur.unwrap_or(0) as i32;
        }
        let pos = s.base_ms as u64 + s.mixer.position_ms(id).unwrap_or(0) as u64;
        let looping = s.looping;
        let wrap = |d: u64| if looping { pos % d } else { pos.min(d) };
        dur.filter(|&d| d > 0).map_or(pos, |d| wrap(d as u64)) as i32
    }

    /// `reset()`: back to `Idle`, dropping the data source.
    pub fn reset(&self) {
        self.run(|s, _| {
            if s.state != PlayerState::End {
                s.teardown();
                s.state = PlayerState::Idle;
            }
        });
    }

    /// `release()`: free everything; the player is unusable afterwards.
    pub fn release(&self) {
        self.run(|s, _| {
            s.teardown();
            s.state = PlayerState::End;
        });
    }

    // ---------- the pump ----------

    /// Keep the mixer fed and notice the end of the track. Cheap when there is nothing to do.
    pub fn pump(&self) {
        self.run(|s, ev| {
            if !matches!(s.state, PlayerState::Prepared | PlayerState::Started | PlayerState::Paused) {
                return;
            }
            if s.refill().is_err() {
                s.fail(ev);
                return;
            }
            let id = s.stream.expect("prepared players have a stream");
            if s.state == PlayerState::Started && s.eof && s.mixer.queued_frames(id).unwrap_or(0) == 0 {
                let _ = s.mixer.set_state(id, PlayState::Stopped);
                s.state = PlayerState::PlaybackCompleted;
                ev.push(PlayerEvent::Completion);
            }
        });
    }

    /// Run [`pump`](Self::pump) every `interval` on a thread until the guard is dropped.
    pub fn spawn_pump(&self, interval: Duration) -> PumpGuard {
        let stop = Arc::new(AtomicBool::new(false));
        let (p, flag) = (self.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                p.pump();
                std::thread::sleep(interval);
            }
        });
        PumpGuard { stop, handle: Some(handle) }
    }
}

impl Inner {
    fn prepare_inner(&mut self) -> Result<(), MusicError> {
        if self.decoder.is_none() {
            let (src, ext) = self.pending.take().ok_or(MusicError::IllegalState("no data source"))?;
            let dec = Decoder::open(src, ext.as_deref())?;
            self.stream = Some(self.mixer.create_stream(dec.rate(), usize::MAX));
            self.decoder = Some(dec);
            let (id, gain) = (self.stream.expect("just created"), self.gain());
            let _ = self.mixer.set_gain(id, gain);
        }
        // Rewind (a re-prepare after stop) and prefill.
        let dec = self.decoder.as_mut().expect("opened above");
        dec.seek_ms(0)?;
        let id = self.stream.expect("created with the decoder");
        let _ = self.mixer.clear(id);
        let _ = self.mixer.set_state(id, PlayState::Stopped);
        self.base_ms = 0;
        self.eof = false;
        self.refill()
    }

    fn teardown(&mut self) {
        self.drop_stream();
        self.decoder = None;
        self.pending = None;
        self.eof = false;
        self.base_ms = 0;
    }
}

/// Stops the pump thread when dropped.
pub struct PumpGuard {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for PumpGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
