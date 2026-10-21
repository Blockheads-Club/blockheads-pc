//! The device-independent mixer: any number of queued-PCM streams summed into one stereo `i16`
//! output at the device rate. The audio device (SDL, cpal, a WAV writer, a test) just calls
//! [`Mixer::fill`] whenever it wants samples; it never touches guest memory or guest code.

use crate::pcm::Frame;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub type StreamId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Paused,
    Playing,
}

/// Something that happened inside [`Mixer::fill`] that the guest-side code must react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerEvent {
    /// A queued buffer was played to its end.
    BufferDone(StreamId),
    /// The last queued buffer of a playing stream finished.
    QueueEmpty(StreamId),
    /// Nothing happened; somebody wants the consumer to look at its own state.
    Wake,
}

/// A blocking queue of [`MixerEvent`]s.
#[derive(Default)]
pub struct EventQueue {
    q: Mutex<VecDeque<MixerEvent>>,
    cv: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl EventQueue {
    pub fn push(&self, ev: MixerEvent) {
        lock(&self.q).push_back(ev);
        self.cv.notify_one();
    }

    pub fn try_pop(&self) -> Option<MixerEvent> {
        lock(&self.q).pop_front()
    }

    /// Wait up to `timeout` for the next event.
    pub fn wait(&self, timeout: Duration) -> Option<MixerEvent> {
        let mut g = lock(&self.q);
        if g.is_empty() {
            g = self.cv.wait_timeout(g, timeout).unwrap_or_else(PoisonError::into_inner).0;
        }
        g.pop_front()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixError {
    NoSuchStream,
    /// The stream already holds `max_buffers` buffers.
    QueueFull,
    /// Zero frames.
    Empty,
}

impl fmt::Display for MixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MixError::NoSuchStream => "no such stream",
            MixError::QueueFull => "buffer queue is full",
            MixError::Empty => "empty buffer",
        })
    }
}

impl std::error::Error for MixError {}

/// Counters, for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Output frames produced by [`Mixer::fill`].
    pub frames_out: u64,
    /// `fill` calls in which a playing stream ran out of data.
    pub underruns: u64,
}

struct Buffer {
    frames: Vec<Frame>,
}

struct Stream {
    rate: u32,
    max_buffers: usize,
    state: PlayState,
    queue: VecDeque<Buffer>,
    /// Frame index inside the front buffer.
    pos: usize,
    /// Fractional source position (0 when the rates are equal).
    frac: f64,
    gain: f32,
    played_index: u32,
    played_frames: u64,
}

impl Stream {
    fn queued_frames(&self) -> usize {
        self.queue.iter().map(|b| b.frames.len()).sum::<usize>() - self.pos
    }
}

struct Inner {
    streams: HashMap<StreamId, Stream>,
    next_id: StreamId,
    master: f32,
    /// [`Mixer::set_held`]: `fill` gives silence and every stream keeps its place.
    held: bool,
    stats: Stats,
}

struct Shared {
    out_rate: u32,
    inner: Mutex<Inner>,
    events: EventQueue,
    /// Signalled whenever a stream consumed or lost data.
    space: Condvar,
}

/// The mixer. Cloning is cheap and shares the same mixer.
#[derive(Clone)]
pub struct Mixer {
    shared: Arc<Shared>,
}

impl Mixer {
    /// A mixer producing stereo audio at `out_rate` Hz.
    pub fn new(out_rate: u32) -> Mixer {
        Mixer {
            shared: Arc::new(Shared {
                out_rate,
                inner: Mutex::new(Inner {
                    streams: HashMap::new(),
                    next_id: 1,
                    master: 1.0,
                    held: false,
                    stats: Stats::default(),
                }),
                events: EventQueue::default(),
                space: Condvar::new(),
            }),
        }
    }

    pub fn out_rate(&self) -> u32 {
        self.shared.out_rate
    }

    pub fn events(&self) -> &EventQueue {
        &self.shared.events
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        lock(&self.shared.inner)
    }

    fn with<R>(&self, id: StreamId, f: impl FnOnce(&mut Stream) -> R) -> Result<R, MixError> {
        let mut g = self.inner();
        g.streams.get_mut(&id).map(f).ok_or(MixError::NoSuchStream)
    }

    pub fn stats(&self) -> Stats {
        self.inner().stats
    }

    pub fn stream_count(&self) -> usize {
        self.inner().streams.len()
    }

    /// Master volume (linear, 1.0 = unity).
    pub fn set_master(&self, gain: f32) {
        self.inner().master = gain;
    }

    /// Hold the output: while held, [`Mixer::fill`] gives silence without playing anything, so streams keep
    /// their data and position (a song started meanwhile starts from its beginning once released).
    pub fn set_held(&self, held: bool) {
        self.inner().held = held;
    }

    pub fn held(&self) -> bool {
        self.inner().held
    }

    // ---------- streams ----------

    /// A stopped stream whose buffers are sampled at `rate` Hz and which holds at most
    /// `max_buffers` queued buffers.
    pub fn create_stream(&self, rate: u32, max_buffers: usize) -> StreamId {
        let mut g = self.inner();
        let id = g.next_id;
        g.next_id += 1;
        g.streams.insert(
            id,
            Stream {
                rate,
                max_buffers,
                state: PlayState::Stopped,
                queue: VecDeque::new(),
                pos: 0,
                frac: 0.0,
                gain: 1.0,
                played_index: 0,
                played_frames: 0,
            },
        );
        id
    }

    /// Remove a stream; it produces no events afterwards. False if it did not exist.
    pub fn destroy_stream(&self, id: StreamId) -> bool {
        let gone = self.inner().streams.remove(&id).is_some();
        self.shared.space.notify_all();
        gone
    }

    pub fn enqueue(&self, id: StreamId, frames: Vec<Frame>) -> Result<(), MixError> {
        if frames.is_empty() {
            return Err(MixError::Empty);
        }
        self.with(id, |s| {
            if s.queue.len() >= s.max_buffers {
                return Err(MixError::QueueFull);
            }
            s.queue.push_back(Buffer { frames });
            Ok(())
        })?
    }

    /// Drop every queued buffer (no events) and reset the play index.
    pub fn clear(&self, id: StreamId) -> Result<(), MixError> {
        self.with(id, |s| {
            s.queue.clear();
            s.pos = 0;
            s.frac = 0.0;
            s.played_index = 0;
        })?;
        self.shared.space.notify_all();
        Ok(())
    }

    /// `Stopped` rewinds the current buffer and the position but keeps the queue.
    pub fn set_state(&self, id: StreamId, state: PlayState) -> Result<(), MixError> {
        self.with(id, |s| {
            s.state = state;
            if state == PlayState::Stopped {
                s.pos = 0;
                s.frac = 0.0;
                s.played_frames = 0;
            }
        })
    }

    pub fn state(&self, id: StreamId) -> Result<PlayState, MixError> {
        self.with(id, |s| s.state)
    }

    /// `(queued buffers, buffers played since creation or the last clear)`.
    pub fn queue_state(&self, id: StreamId) -> Result<(u32, u32), MixError> {
        self.with(id, |s| (s.queue.len() as u32, s.played_index))
    }

    /// Source frames still waiting to be played.
    pub fn queued_frames(&self, id: StreamId) -> Result<usize, MixError> {
        self.with(id, |s| s.queued_frames())
    }

    /// Source frames consumed since the stream was last stopped.
    pub fn played_frames(&self, id: StreamId) -> Result<u64, MixError> {
        self.with(id, |s| s.played_frames)
    }

    /// Playback position in milliseconds since the stream was last stopped.
    pub fn position_ms(&self, id: StreamId) -> Result<u32, MixError> {
        self.with(id, |s| (s.played_frames * 1000 / s.rate as u64) as u32)
    }

    /// Linear gain of one stream.
    pub fn set_gain(&self, id: StreamId, gain: f32) -> Result<(), MixError> {
        self.with(id, |s| s.gain = gain)
    }

    /// Block until the stream has at most `max_frames` queued, it vanished, or `timeout` passed.
    /// Returns whether the condition holds.
    pub fn wait_for_space(&self, id: StreamId, max_frames: usize, timeout: Duration) -> bool {
        let g = self.inner();
        let ready = |i: &mut Inner| i.streams.get(&id).is_none_or(|s| s.queued_frames() <= max_frames);
        let (mut g, res) =
            self.shared.space.wait_timeout_while(g, timeout, |i| !ready(i)).unwrap_or_else(PoisonError::into_inner);
        !res.timed_out() || ready(&mut g)
    }

    // ---------- the device side ----------

    /// Mix `out.len() / 2` stereo frames (interleaved L R) into `out`, overwriting it. Streams that
    /// are not playing contribute nothing; a playing stream without data contributes silence.
    pub fn fill(&self, out: &mut [i16]) {
        let rate = self.shared.out_rate as f64;
        let mut g = self.inner();
        if g.held {
            out.fill(0);
            return;
        }
        let master = g.master;
        let mut acc = vec![[0f32; 2]; out.len() / 2];
        let mut underruns = 0;
        let mut freed = false;
        for (&id, s) in g.streams.iter_mut().filter(|(_, s)| s.state == PlayState::Playing) {
            let step = s.rate as f64 / rate;
            let mut starved = false;
            for a in acc.iter_mut() {
                let Some(front) = s.queue.front() else {
                    starved = true;
                    break;
                };
                let cur = front.frames[s.pos];
                let mix = if s.frac == 0.0 {
                    cur
                } else {
                    let next = front
                        .frames
                        .get(s.pos + 1)
                        .or_else(|| s.queue.get(1).map(|b| &b.frames[0]))
                        .copied()
                        .unwrap_or(cur);
                    let f = s.frac as f32;
                    [0, 1].map(|c| cur[c] + ((next[c] as f32 - cur[c] as f32) * f) as i16)
                };
                a[0] += mix[0] as f32 * s.gain;
                a[1] += mix[1] as f32 * s.gain;
                s.frac += step;
                while s.frac >= 1.0 {
                    s.frac -= 1.0;
                    s.pos += 1;
                    s.played_frames += 1;
                    if s.pos >= s.queue[0].frames.len() {
                        s.queue.pop_front();
                        s.pos = 0;
                        s.played_index = s.played_index.wrapping_add(1);
                        freed = true;
                        self.shared.events.push(MixerEvent::BufferDone(id));
                        if s.queue.is_empty() {
                            s.frac = 0.0;
                            self.shared.events.push(MixerEvent::QueueEmpty(id));
                            break;
                        }
                    }
                }
            }
            if starved {
                underruns += 1;
            }
        }
        g.stats.frames_out += acc.len() as u64;
        g.stats.underruns += underruns;
        drop(g);
        if freed {
            self.shared.space.notify_all();
        }
        for (o, a) in out.chunks_exact_mut(2).zip(acc) {
            o[0] = (a[0] * master).clamp(-32768.0, 32767.0) as i16;
            o[1] = (a[1] * master).clamp(-32768.0, 32767.0) as i16;
        }
    }
}
