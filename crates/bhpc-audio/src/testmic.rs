//! A test microphone for headless voice tests (`BHPC_VOICE_TEST_MIC`): a [`CaptureBackend`] that records a
//! generated or recorded signal instead of a device, in real time (one 20 ms chunk every 20 ms on its own thread),
//! so the voice path (VAD, Opus, the voice UDP port, the other client's mix) runs end to end without a sound card.
//!
//! Specs (the value of the environment variable):
//! * `sine[:HZ]`: a continuous tone (default 440 Hz) at about -10 dBFS;
//! * `pulse[:HZ[:ON_MS[:PERIOD_MS]]]`: the tone for `ON_MS` (default 200) at the start of every `PERIOD_MS`
//!   (default 2000), silence otherwise: rising edges for latency measurements;
//! * anything else: a WAV file (16-bit PCM, 48 kHz, mono or stereo, mixed down), looped.
//!
//! Never used unless the variable is set; it only replaces where the samples come from.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::capture::{CaptureBackend, CaptureSink, CAPTURE_RATE, CHUNK_SAMPLES};

/// Amplitude of the generated tone (about -10 dBFS).
pub const AMPLITUDE: f32 = 0.3;

/// What the test microphone records.
#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    /// A continuous tone of `hz`.
    Sine { hz: f32 },
    /// A tone of `hz` for `on_ms` at the start of every `period_ms`.
    Pulse { hz: f32, on_ms: u32, period_ms: u32 },
    /// Recorded samples (48 kHz mono), looped.
    Samples(Vec<i16>),
}

fn num<T: std::str::FromStr>(part: Option<&str>, default: T, what: &str) -> Result<T, String> {
    match part {
        None | Some("") => Ok(default),
        Some(p) => p.parse().map_err(|_| format!("test mic: bad {what} {p:?}")),
    }
}

impl Signal {
    /// Parses a spec (see the module doc); a WAV file is read from disk.
    pub fn parse(spec: &str) -> Result<Signal, String> {
        let mut parts = spec.split(':');
        match parts.next() {
            Some("sine") => Ok(Signal::Sine { hz: num(parts.next(), 440.0, "frequency")? }),
            Some("pulse") => {
                let hz = num(parts.next(), 440.0, "frequency")?;
                let on_ms = num(parts.next(), 200, "pulse length")?;
                let period_ms: u32 = num(parts.next(), 2000, "period")?;
                if period_ms == 0 || on_ms > period_ms {
                    return Err(format!("test mic: pulse of {on_ms} ms does not fit a period of {period_ms} ms"));
                }
                Ok(Signal::Pulse { hz, on_ms, period_ms })
            }
            _ => {
                let bytes = std::fs::read(spec).map_err(|e| format!("test mic: {spec}: {e}"))?;
                Ok(Signal::Samples(parse_wav(&bytes)?))
            }
        }
    }

    /// The sample at position `n` (samples since the microphone opened).
    pub fn sample(&self, n: u64) -> i16 {
        let tone = |hz: f32| {
            let t = (n % u64::from(CAPTURE_RATE)) as f32 / CAPTURE_RATE as f32;
            ((t * hz * std::f32::consts::TAU).sin() * AMPLITUDE * 32767.0) as i16
        };
        match self {
            Signal::Sine { hz } => tone(*hz),
            Signal::Pulse { hz, on_ms, period_ms } => {
                let ms = n * 1000 / u64::from(CAPTURE_RATE);
                if ms % u64::from(*period_ms) < u64::from(*on_ms) {
                    tone(*hz)
                } else {
                    0
                }
            }
            Signal::Samples(s) if s.is_empty() => 0,
            Signal::Samples(s) => s[(n % s.len() as u64) as usize],
        }
    }

    /// Fills `out` from position `*pos` on and advances it.
    pub fn fill(&self, pos: &mut u64, out: &mut [i16]) {
        for o in out {
            *o = self.sample(*pos);
            *pos += 1;
        }
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The samples of a WAV file: 16-bit PCM at 48 kHz, mono or stereo (mixed down to mono).
pub fn parse_wav(b: &[u8]) -> Result<Vec<i16>, String> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("test mic: not a WAV file".into());
    }
    let mut at = 12;
    let mut format = None;
    while at + 8 <= b.len() {
        let (id, len) = (&b[at..at + 4], u32_at(b, at + 4) as usize);
        let body = at + 8;
        let end = body.checked_add(len).filter(|e| *e <= b.len()).ok_or("test mic: WAV chunk cut short")?;
        match id {
            b"fmt " if len >= 16 => {
                format = Some((u16_at(b, body), u16_at(b, body + 2), u32_at(b, body + 4), u16_at(b, body + 14)));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format.ok_or("test mic: WAV data before its format")?;
                if tag != 1 || bits != 16 || rate != CAPTURE_RATE || !(1..=2).contains(&channels) {
                    return Err(format!(
                        "test mic: WAV must be 16-bit PCM, {CAPTURE_RATE} Hz, mono or stereo (got format {tag}, \
                         {bits} bit, {rate} Hz, {channels} channels)"
                    ));
                }
                let frame = 2 * usize::from(channels);
                return Ok(b[body..end]
                    .chunks_exact(frame)
                    .map(|f| {
                        let sum: i32 = f.chunks_exact(2).map(|s| i32::from(i16::from_le_bytes([s[0], s[1]]))).sum();
                        (sum / i32::from(channels)) as i16
                    })
                    .collect());
            }
            _ => {}
        }
        at = end + (len & 1);
    }
    Err("test mic: WAV file without data".into())
}

/// The test microphone as a capture backend.
pub struct TestMic {
    signal: Arc<Signal>,
}

/// The running test microphone; dropping it stops its thread.
struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl TestMic {
    pub fn new(signal: Signal) -> TestMic {
        TestMic { signal: Arc::new(signal) }
    }

    /// The test microphone of the environment (`BHPC_VOICE_TEST_MIC`), if set; a bad spec is an error.
    pub fn from_env() -> Option<Result<TestMic, String>> {
        std::env::var("BHPC_VOICE_TEST_MIC").ok().map(|s| Signal::parse(&s).map(TestMic::new))
    }
}

impl CaptureBackend for TestMic {
    fn devices(&mut self) -> Vec<String> {
        vec!["test".into()]
    }

    fn open(
        &mut self,
        _device: Option<&str>,
        _rate: u32,
        _samples: u16,
        mut sink: CaptureSink,
    ) -> Result<Box<dyn Any>, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (signal, flag) = (self.signal.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("bhpc-test-mic".into())
            .spawn(move || {
                let start = Instant::now();
                let (mut pos, mut chunk, mut n) = (0u64, vec![0i16; CHUNK_SAMPLES], 0u32);
                while !flag.load(Ordering::SeqCst) {
                    signal.fill(&mut pos, &mut chunk);
                    sink(&chunk);
                    n += 1;
                    // Real time: chunk n is due n * 20 ms after the start (no drift).
                    let due = start + Duration::from_millis(u64::from(n) * 20);
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                }
            })
            .ok()
            .ok_or("test mic: cannot start its thread")?;
        Ok(Box::new(Running { stop, thread: Some(thread) }))
    }
}
