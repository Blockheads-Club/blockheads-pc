//! OpenSL ES high-level emulation for the guest.
//!
//! The guest reaches OpenSL ES in two ways: `dlopen("libOpenSLES.so")` followed by `dlsym` of
//! `slCreateEngine` and the `SL_IID_*` variables (libOpenAL's `opensles` backend and libAudioUnit),
//! or a direct import. [`Sles::install`] registers a thunk for `slCreateEngine` and one for every
//! method of the interface vtables it builds in guest memory, and [`Sles::lookup`] answers the
//! `dlsym` calls (functions and the `SL_IID_*` data words).
//!
//! What is implemented: engine, output mix, audio player with a buffer queue
//! (`SL_IID_BUFFERQUEUE` and `SL_IID_ANDROIDSIMPLEBUFFERQUEUE` are the same interface), play and
//! volume interfaces, PCM 8/16 bit and float, mono and stereo, any rate from 4 kHz. Everything else
//! (recorders, MIDI, LED/vibra, 3D, metadata, effects, stereo positioning, markers) returns
//! `SL_RESULT_FEATURE_UNSUPPORTED`, as do requests for it marked "required". Optional interfaces
//! that are not available are skipped, as the spec says.
//!
//! Buffers are copied out of guest memory by `Enqueue`, so the guest may reuse them at once. Buffer
//! queue callbacks (and the play interface's `HEADATEND` event, and the completion of an
//! asynchronous `Realize`) are delivered by the *pump*, which runs them on a guest CPU of its own:
//! see [`Sles::pump_once`] and [`Sles::spawn_pump`].

pub mod consts;
mod core;

pub use self::core::GuestCall;

use self::consts::*;
use self::core::{Core, Vtables, R};
use crate::mixer::{Mixer, MixerEvent};
use bhpc_cpu::{Cpu, CpuError, Stop};
use bhpc_mem::MemError;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Size of the guest region holding vtables, IIDs and handle slots.
const REGION: u32 = 0x1_0000;
/// Offset of the handle-slot pool inside the region.
const POOL_OFFSET: u32 = 0x400;
const SLOT: u32 = 16;

/// What a [`Sles::lookup`] result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// Address of a callable thunk.
    Function,
    /// Address of a data word (a `const SLInterfaceID` variable).
    Data,
}

#[derive(Debug)]
pub enum SlesError {
    Cpu(CpuError),
    Mem(MemError),
}

impl fmt::Display for SlesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlesError::Cpu(e) => write!(f, "opensles install: {e}"),
            SlesError::Mem(e) => write!(f, "opensles install: {e}"),
        }
    }
}

impl std::error::Error for SlesError {}

impl From<CpuError> for SlesError {
    fn from(e: CpuError) -> Self {
        SlesError::Cpu(e)
    }
}

impl From<MemError> for SlesError {
    fn from(e: MemError) -> Self {
        SlesError::Mem(e)
    }
}

type Method = fn(&Core, &[u32]) -> R;

/// A vtable slot: method name, number of arguments it reads (including `self`), implementation.
struct Entry(&'static str, usize, Method);

const OBJECT: &[Entry] = &[
    Entry("Realize", 2, Core::realize),
    Entry("Resume", 2, Core::resume),
    Entry("GetState", 2, Core::get_state),
    Entry("GetInterface", 3, Core::get_interface),
    Entry("RegisterCallback", 3, Core::object_register_callback),
    Entry("AbortAsyncOperation", 1, Core::accept_on_object),
    Entry("Destroy", 1, Core::destroy),
    Entry("SetPriority", 3, Core::accept_on_object),
    Entry("GetPriority", 3, Core::get_priority),
    Entry("SetLossOfControlInterfaces", 4, Core::accept_on_object),
];

const ENGINE: &[Entry] = &[
    Entry("CreateLEDDevice", 1, Core::unsupported),
    Entry("CreateVibraDevice", 1, Core::unsupported),
    Entry("CreateAudioPlayer", 7, Core::create_audio_player),
    Entry("CreateAudioRecorder", 1, Core::unsupported),
    Entry("CreateMidiPlayer", 1, Core::unsupported),
    Entry("CreateListener", 1, Core::unsupported),
    Entry("Create3DGroup", 1, Core::unsupported),
    Entry("CreateOutputMix", 5, Core::create_output_mix),
    Entry("CreateMetadataExtractor", 1, Core::unsupported),
    Entry("CreateExtensionObject", 1, Core::unsupported),
    Entry("QueryNumSupportedInterfaces", 1, Core::unsupported),
    Entry("QuerySupportedInterfaces", 1, Core::unsupported),
    Entry("QueryNumSupportedExtensions", 2, Core::num_extensions),
    Entry("QuerySupportedExtension", 1, Core::unsupported),
    Entry("IsExtensionSupported", 3, Core::is_extension_supported),
];

const PLAY: &[Entry] = &[
    Entry("SetPlayState", 2, Core::set_play_state),
    Entry("GetPlayState", 2, Core::get_play_state),
    Entry("GetDuration", 2, Core::get_duration),
    Entry("GetPosition", 2, Core::get_position),
    Entry("RegisterCallback", 3, Core::play_register_callback),
    Entry("SetCallbackEventsMask", 2, Core::set_callback_events_mask),
    Entry("GetCallbackEventsMask", 2, Core::get_callback_events_mask),
    Entry("SetMarkerPosition", 1, Core::unsupported),
    Entry("ClearMarkerPosition", 1, Core::unsupported),
    Entry("GetMarkerPosition", 1, Core::unsupported),
    Entry("SetPositionUpdatePeriod", 1, Core::unsupported),
    Entry("GetPositionUpdatePeriod", 1, Core::unsupported),
];

const BUFFER_QUEUE: &[Entry] = &[
    Entry("Enqueue", 3, Core::enqueue),
    Entry("Clear", 1, Core::clear),
    Entry("GetState", 2, Core::queue_get_state),
    Entry("RegisterCallback", 3, Core::queue_register_callback),
];

const VOLUME: &[Entry] = &[
    Entry("SetVolumeLevel", 2, Core::set_volume_level),
    Entry("GetVolumeLevel", 2, Core::get_volume_level),
    Entry("GetMaxVolumeLevel", 2, Core::get_max_volume_level),
    Entry("SetMute", 2, Core::set_mute),
    Entry("GetMute", 2, Core::get_mute),
    Entry("EnableStereoPosition", 2, Core::enable_stereo_position),
    Entry("IsEnabledStereoPosition", 2, Core::write_zero),
    Entry("SetStereoPosition", 1, Core::unsupported),
    Entry("GetStereoPosition", 2, Core::write_zero),
];

/// `slCreateEngine(pEngine, numOptions, pEngineOptions, numInterfaces, pInterfaceIds, pInterfaceRequired)`
fn sl_create_engine(c: &Core, a: &[u32]) -> R {
    c.create_engine(a[0], a[3], a[4], a[5])
}

/// The installed OpenSL ES emulation.
pub struct Sles {
    core: Arc<Core>,
    exports: HashMap<&'static str, (u32, ExportKind)>,
    thunks: Vec<String>,
}

fn register(cpu: &Cpu, core: &Arc<Core>, name: &str, nargs: usize, f: Method) -> Result<u32, CpuError> {
    let core = core.clone();
    cpu.table().register_shared_thunk(
        name,
        Arc::new(move |c: &mut Cpu| {
            let mut cur = c.args();
            let mut a = [0u32; 8];
            for v in a.iter_mut().take(nargs) {
                *v = cur.u32(c);
            }
            let code = f(&core, &a[..nargs]).err().unwrap_or(SL_RESULT_SUCCESS);
            c.ret_u32(code);
        }),
    )
}

impl Sles {
    /// Map the emulation's guest region, register its thunks on `cpu` (and so on every sibling
    /// CPU) and make `mixer` the place the audio goes.
    pub fn install(cpu: &Cpu, mixer: Mixer) -> Result<Arc<Sles>, SlesError> {
        let mem = cpu.mem().clone();
        let base = mem.map_anywhere(REGION, 0x1000)?;
        let tables: [(&str, &[Entry]); 5] = [
            ("SLObjectItf", OBJECT),
            ("SLEngineItf", ENGINE),
            ("SLPlayItf", PLAY),
            ("SLBufferQueueItf", BUFFER_QUEUE),
            ("SLVolumeItf", VOLUME),
        ];
        // Static layout: the five vtables, the interface IDs, the SL_IID_* pointer words.
        let mut vt_addrs = [0u32; 5];
        let mut cursor = base;
        for (i, (_, t)) in tables.iter().enumerate() {
            vt_addrs[i] = cursor;
            cursor += 4 * t.len() as u32;
        }
        let vt = Vtables {
            object: vt_addrs[0],
            engine: vt_addrs[1],
            play: vt_addrs[2],
            bq: vt_addrs[3],
            volume: vt_addrs[4],
        };
        let pool = base + POOL_OFFSET;
        let core = Arc::new(Core::new(mem.clone(), mixer, vt, pool, (REGION - POOL_OFFSET) / SLOT));

        let mut exports = HashMap::new();
        let mut thunks = Vec::new();
        for (i, (itf, table)) in tables.iter().enumerate() {
            for (j, Entry(method, nargs, f)) in table.iter().enumerate() {
                let name = format!("OpenSLES:{itf}.{method}");
                let addr = register(cpu, &core, &name, *nargs, *f)?;
                mem.write_u32(vt_addrs[i] + 4 * j as u32, addr)?;
                thunks.push(name);
            }
        }
        let create = register(cpu, &core, "slCreateEngine", 6, sl_create_engine)?;
        exports.insert("slCreateEngine", (create, ExportKind::Function));
        thunks.push("slCreateEngine".to_string());

        let mut iid_at = cursor;
        let mut word_at = cursor + 16 * IIDS.len() as u32;
        for (name, _, bytes) in &IIDS {
            mem.write_bytes(iid_at, bytes)?;
            mem.write_u32(word_at, iid_at)?;
            exports.insert(*name, (word_at, ExportKind::Data));
            iid_at += 16;
            word_at += 4;
        }
        Ok(Arc::new(Sles { core, exports, thunks }))
    }

    /// Resolve a symbol of `libOpenSLES.so`: `slCreateEngine` or an `SL_IID_*` variable.
    pub fn lookup(&self, name: &str) -> Option<(u32, ExportKind)> {
        self.exports.get(name).copied()
    }

    /// Every exported symbol, sorted by name.
    pub fn exports(&self) -> Vec<(&'static str, u32, ExportKind)> {
        let mut v: Vec<_> = self.exports.iter().map(|(n, (a, k))| (*n, *a, *k)).collect();
        v.sort_unstable_by_key(|e| e.0);
        v
    }

    /// Names of all thunks registered (vtable methods and exports).
    pub fn thunk_names(&self) -> &[String] {
        &self.thunks
    }

    pub fn mixer(&self) -> &Mixer {
        self.core.mixer()
    }

    /// Objects created and not yet destroyed.
    pub fn live_objects(&self) -> usize {
        self.core.live_objects()
    }

    /// Handle slots still available.
    pub fn free_slots(&self) -> usize {
        self.core.free_slots()
    }

    /// Wait up to `wait` for something to deliver, then run the guest callbacks it requires on
    /// `cpu` (which must have a valid stack and must not be running). Returns how many guest
    /// functions were called, or the first guest failure.
    pub fn pump_once(&self, cpu: &mut Cpu, wait: Duration) -> Result<usize, Stop> {
        let calls = self.core.collect_calls(wait);
        for c in &calls {
            cpu.call(c.addr, &c.args)?;
        }
        Ok(calls.len())
    }

    /// Run the pump on its own OS thread with a fresh guest CPU whose stack grows down from
    /// `stack_top` (the caller maps the stack). Callbacks arrive on that thread, like OpenSL ES's
    /// own callback thread.
    pub fn spawn_pump(self: &Arc<Self>, cpu: &Cpu, stack_top: u32) -> Result<PumpHandle, CpuError> {
        let mut worker = cpu.new_thread()?;
        worker.set_sp(stack_top);
        let (sles, stop) = (self.clone(), Arc::new(AtomicBool::new(false)));
        let flag = stop.clone();
        let join = std::thread::Builder::new().name("bhpc-sles".into()).spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                sles.pump_once(&mut worker, Duration::from_millis(20))?;
            }
            Ok(())
        });
        Ok(PumpHandle { stop, join: join.ok(), mixer: self.core.mixer().clone() })
    }
}

/// A running pump thread. Dropping it stops the thread.
pub struct PumpHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<(), Stop>>>,
    mixer: Mixer,
}

impl PumpHandle {
    /// True once the thread has ended on its own, which only happens after a guest failure.
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|j| j.is_finished())
    }

    /// Stop the thread and report the guest failure that ended it, if any.
    pub fn stop(mut self) -> Result<(), Stop> {
        self.finish()
    }

    fn finish(&mut self) -> Result<(), Stop> {
        self.stop.store(true, Ordering::Relaxed);
        self.mixer.events().push(MixerEvent::Wake);
        self.join.take().map_or(Ok(()), |j| j.join().unwrap_or(Ok(())))
    }
}

impl Drop for PumpHandle {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
