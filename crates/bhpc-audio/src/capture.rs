//! Microphone capture for voice chat: 48 kHz mono `i16`, cut into 20 ms chunks and handed over through a bounded
//! queue whose producer side never blocks (the device callback only does `try_send`).
//!
//! Split by thread:
//! * [`MicControl`] (shared, any thread): what the voice code wants (mic on/off, device), the chunks, the device
//!   list, and [`MicControl::live`], the state of the host-owned mic-on indicator.
//! * [`MicDriver`] (the thread that owns the audio library, SDL's main thread): [`MicDriver::sync`] opens or closes
//!   the device through a [`CaptureBackend`] so it matches the wish. The device is open only while wanted: the mic is
//!   off by default and closed again as soon as nothing asks for it.
//!
//! Nothing here links an audio library; `bhpc-run` implements [`CaptureBackend`] over SDL2 (`open_capture`).

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, PoisonError};

pub use crate::voice::{FRAME_SAMPLES as CHUNK_SAMPLES, VOICE_RATE as CAPTURE_RATE};

/// Chunks the queue holds before new ones are dropped (1 s): the consumer is the game's frame, which may stall.
pub const QUEUE_CHUNKS: usize = 50;

/// Receives captured samples on the device's thread.
pub type CaptureSink = Box<dyn FnMut(&[i16]) + Send>;

/// An audio library that can record.
pub trait CaptureBackend {
    /// Names of the capture devices.
    fn devices(&mut self) -> Vec<String>;
    /// Open `device` (`None`: the default) for mono `i16` at `rate` Hz with a device buffer of `samples`, and start
    /// it; samples arrive through `sink`. Dropping the returned value closes the device.
    fn open(
        &mut self,
        device: Option<&str>,
        rate: u32,
        samples: u16,
        sink: CaptureSink,
    ) -> Result<Box<dyn Any>, String>;
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Cuts device buffers of any size into [`CHUNK_SAMPLES`] chunks and queues them.
pub struct Chunker {
    buf: Vec<i16>,
    tx: SyncSender<Vec<i16>>,
    dropped: std::sync::Arc<AtomicU64>,
}

impl Chunker {
    /// Feed samples (device thread). A full queue drops the chunk and counts it.
    pub fn push(&mut self, mut samples: &[i16]) {
        while !samples.is_empty() {
            let take = (CHUNK_SAMPLES - self.buf.len()).min(samples.len());
            self.buf.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.buf.len() == CHUNK_SAMPLES {
                let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK_SAMPLES));
                if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = self.tx.try_send(chunk) {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// The voice side of the microphone (shared between threads).
pub struct MicControl {
    want: AtomicBool,
    device: Mutex<Option<String>>,
    /// Bumped when the wish (device) changes, so the driver reopens.
    serial: AtomicU64,
    live: AtomicBool,
    refresh: AtomicBool,
    devices: Mutex<Vec<String>>,
    error: Mutex<Option<String>>,
    tx: SyncSender<Vec<i16>>,
    rx: Mutex<Receiver<Vec<i16>>>,
    dropped: std::sync::Arc<AtomicU64>,
}

impl Default for MicControl {
    fn default() -> Self {
        let (tx, rx) = sync_channel(QUEUE_CHUNKS);
        MicControl {
            want: AtomicBool::new(false),
            device: Mutex::new(None),
            serial: AtomicU64::new(0),
            live: AtomicBool::new(false),
            refresh: AtomicBool::new(false),
            devices: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            tx,
            rx: Mutex::new(rx),
            dropped: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }
}

impl MicControl {
    /// Ask for the mic to be open (`true`) or closed. Off by default.
    pub fn set_wanted(&self, on: bool) {
        self.want.store(on, Ordering::SeqCst);
    }

    pub fn wanted(&self) -> bool {
        self.want.load(Ordering::SeqCst)
    }

    /// Record from `device` (`None`: the system default); an open mic switches over.
    pub fn set_device(&self, device: Option<String>) {
        let mut d = lock(&self.device);
        if *d != device {
            *d = device;
            self.serial.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn device(&self) -> Option<String> {
        lock(&self.device).clone()
    }

    /// The device is open and recording: the host draws the mic-on indicator (mods cannot hide it).
    pub fn live(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// Ask the driver for a fresh device list ([`MicControl::devices`] after its next sync).
    pub fn request_devices(&self) {
        self.refresh.store(true, Ordering::SeqCst);
    }

    /// The capture devices as last listed.
    pub fn devices(&self) -> Vec<String> {
        lock(&self.devices).clone()
    }

    /// Why the last open failed (cleared by a successful open).
    pub fn last_error(&self) -> Option<String> {
        lock(&self.error).clone()
    }

    /// The next 20 ms chunk, if one is waiting.
    pub fn pop(&self) -> Option<Vec<i16>> {
        lock(&self.rx).try_recv().ok()
    }

    /// Chunks dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn discard(&self) {
        while self.pop().is_some() {}
    }

    fn chunker(&self) -> Chunker {
        Chunker { buf: Vec::with_capacity(CHUNK_SAMPLES), tx: self.tx.clone(), dropped: self.dropped.clone() }
    }
}

/// Opens and closes the device to follow a [`MicControl`] (on the audio library's thread).
pub struct MicDriver {
    ctl: std::sync::Arc<MicControl>,
    /// The open device and the wish serial it was opened for.
    open: Option<(u64, Box<dyn Any>)>,
    /// The serial whose open failed (not retried until the wish changes or the mic is turned off and on).
    failed: Option<u64>,
}

impl MicDriver {
    pub fn new(ctl: std::sync::Arc<MicControl>) -> MicDriver {
        MicDriver { ctl, open: None, failed: None }
    }

    /// Bring the device in line with the wish; cheap when nothing changed (call it once per event-loop turn).
    pub fn sync(&mut self, backend: &mut dyn CaptureBackend) {
        let ctl = self.ctl.clone();
        if ctl.refresh.swap(false, Ordering::SeqCst) {
            *lock(&ctl.devices) = backend.devices();
        }
        let serial = ctl.serial.load(Ordering::SeqCst);
        if !ctl.wanted() {
            self.failed = None;
            if self.open.take().is_some() {
                ctl.live.store(false, Ordering::SeqCst);
                ctl.discard();
            }
            return;
        }
        if self.open.as_ref().is_some_and(|(s, _)| *s == serial) || self.failed == Some(serial) {
            return;
        }
        // A different device was asked for: close the old one first (SDL may not open the same device twice).
        self.open = None;
        ctl.live.store(false, Ordering::SeqCst);
        let mut chunker = ctl.chunker();
        let device = ctl.device();
        match backend.open(device.as_deref(), CAPTURE_RATE, CHUNK_SAMPLES as u16, Box::new(move |s| chunker.push(s))) {
            Ok(dev) => {
                self.open = Some((serial, dev));
                self.failed = None;
                *lock(&ctl.error) = None;
                ctl.live.store(true, Ordering::SeqCst);
            }
            Err(e) => {
                self.failed = Some(serial);
                *lock(&ctl.error) = Some(e);
            }
        }
    }

    /// The device is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
}
