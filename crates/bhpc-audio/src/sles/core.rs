//! The OpenSL ES object model behind the guest vtables.
//!
//! Every guest-visible handle is the address of a 4-byte word in guest memory that holds the
//! address of an interface vtable (`const struct SLxxxItf_ * const *`). That word's address is the
//! key into [`State`]. Methods return `Err(SLresult)` for failures; the thunk wrappers turn the
//! `Result` into the value for `r0`.

use super::consts::*;
use crate::mixer::{Mixer, MixerEvent, PlayState, StreamId};
use crate::pcm::{PcmFormat, SampleFormat};
use bhpc_mem::GuestMemory;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub(crate) type R = Result<(), u32>;

/// Interface kinds the host implements. `BUFFERQUEUE` and `ANDROIDSIMPLEBUFFERQUEUE` share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Object,
    Engine,
    Play,
    Bq,
    Volume,
}

/// A guest function the pump has to call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestCall {
    pub addr: u32,
    pub args: Vec<u32>,
}

/// Guest addresses of the five vtables.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Vtables {
    pub object: u32,
    pub engine: u32,
    pub play: u32,
    pub bq: u32,
    pub volume: u32,
}

/// Bytes per handle slot (a slot only needs 4, but distinct handles stay distinct and aligned).
const SLOT: u32 = 16;

struct Player {
    stream: StreamId,
    fmt: PcmFormat,
    bq_cb: Option<(u32, u32)>,
    play_cb: Option<(u32, u32)>,
    play_mask: u32,
    level_mb: i16,
    mute: bool,
}

enum ObjKind {
    Engine,
    OutputMix,
    Player(Player),
}

struct Object {
    state: u32,
    kind: ObjKind,
    cb: Option<(u32, u32)>,
    itfs: HashMap<Kind, u32>,
    slots: Vec<u32>,
}

pub(crate) struct State {
    free: Vec<u32>,
    objects: HashMap<u32, Object>,
    /// Every interface handle -> (owning object handle, kind).
    itfs: HashMap<u32, (u32, Kind)>,
    pending: VecDeque<GuestCall>,
}

pub(crate) struct Core {
    mem: Arc<GuestMemory>,
    mixer: Mixer,
    vt: Vtables,
    state: Mutex<State>,
}

/// SLmillibel to linear gain.
fn millibel_gain(mb: i16) -> f32 {
    10f32.powf(mb as f32 / 2000.0)
}

impl Core {
    /// `pool` is the guest address of the first handle slot, `slots` how many there are.
    pub(crate) fn new(mem: Arc<GuestMemory>, mixer: Mixer, vt: Vtables, pool: u32, slots: u32) -> Core {
        let free = (0..slots).rev().map(|i| pool + i * SLOT).collect();
        let state = State { free, objects: HashMap::new(), itfs: HashMap::new(), pending: VecDeque::new() };
        Core { mem, mixer, vt, state: Mutex::new(state) }
    }

    pub(crate) fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn live_objects(&self) -> usize {
        self.lock().objects.len()
    }

    pub(crate) fn free_slots(&self) -> usize {
        self.lock().free.len()
    }

    // ---------- guest memory ----------

    fn r32(&self, a: u32) -> Result<u32, u32> {
        self.mem.read_u32(a).map_err(|_| SL_RESULT_PARAMETER_INVALID)
    }

    fn w32(&self, a: u32, v: u32) -> R {
        self.mem.write_u32(a, v).map_err(|_| SL_RESULT_PARAMETER_INVALID)
    }

    fn w16(&self, a: u32, v: u16) -> R {
        self.mem.write_u16(a, v).map_err(|_| SL_RESULT_PARAMETER_INVALID)
    }

    fn read_iid(&self, p: u32) -> Result<[u8; 16], u32> {
        let v = self.mem.read_bytes(p, 16).map_err(|_| SL_RESULT_PARAMETER_INVALID)?;
        Ok(v.try_into().expect("read_bytes returned 16 bytes"))
    }

    fn vtable(&self, k: Kind) -> u32 {
        match k {
            Kind::Object => self.vt.object,
            Kind::Engine => self.vt.engine,
            Kind::Play => self.vt.play,
            Kind::Bq => self.vt.bq,
            Kind::Volume => self.vt.volume,
        }
    }

    // ---------- object bookkeeping ----------

    /// Create an object with its `Object` interface plus `extra` interfaces. `None` if the slot
    /// pool is exhausted.
    fn make_object(&self, st: &mut State, kind: ObjKind, extra: &[Kind]) -> Option<u32> {
        let need = 1 + extra.len();
        if st.free.len() < need {
            return None;
        }
        let slots: Vec<u32> = (0..need).map(|_| st.free.pop().expect("checked above")).collect();
        let obj = slots[0];
        let mut itfs = HashMap::new();
        for (slot, k) in slots.iter().zip(std::iter::once(&Kind::Object).chain(extra)) {
            // The pool lies in memory we mapped ourselves.
            let _ = self.mem.write_u32(*slot, self.vtable(*k));
            itfs.insert(*k, *slot);
            st.itfs.insert(*slot, (obj, *k));
        }
        st.objects.insert(obj, Object { state: SL_OBJECT_STATE_UNREALIZED, kind, cb: None, itfs, slots });
        Some(obj)
    }

    /// Create an engine; returns its object handle (for `slCreateEngine`).
    pub(crate) fn create_engine(&self, out: u32, n: u32, ids: u32, req: u32) -> R {
        self.check_ifaces(n, ids, req, &[Iid::Object, Iid::Engine])?;
        let obj = {
            let mut st = self.lock();
            self.make_object(&mut st, ObjKind::Engine, &[Kind::Engine]).ok_or(SL_RESULT_MEMORY_FAILURE)?
        };
        self.w32(out, obj)
    }

    /// `SLresult` for the interfaces a `Create*` call asks for.
    fn check_ifaces(&self, n: u32, ids: u32, req: u32, supported: &[Iid]) -> R {
        if n == 0 {
            return Ok(());
        }
        if n > 32 || ids == 0 || req == 0 {
            return Err(SL_RESULT_PARAMETER_INVALID);
        }
        for i in 0..n {
            let iid = iid_of(&self.read_iid(self.r32(ids + 4 * i)?)?);
            let required = self.r32(req + 4 * i)? != 0;
            if required && !iid.is_some_and(|k| supported.contains(&k)) {
                return Err(SL_RESULT_FEATURE_UNSUPPORTED);
            }
        }
        Ok(())
    }

    fn engine_check(st: &State, itf: u32) -> R {
        match st.itfs.get(&itf) {
            Some((_, Kind::Engine)) => Ok(()),
            _ => Err(SL_RESULT_PARAMETER_INVALID),
        }
    }

    fn player_mut(st: &mut State, itf: u32, kind: Kind) -> Result<&mut Player, u32> {
        let (obj, k) = *st.itfs.get(&itf).ok_or(SL_RESULT_PARAMETER_INVALID)?;
        match (k == kind, &mut st.objects.get_mut(&obj).ok_or(SL_RESULT_PARAMETER_INVALID)?.kind) {
            (true, ObjKind::Player(p)) => Ok(p),
            _ => Err(SL_RESULT_PARAMETER_INVALID),
        }
    }

    fn object_mut(st: &mut State, handle: u32) -> Result<&mut Object, u32> {
        st.objects.get_mut(&handle).ok_or(SL_RESULT_PARAMETER_INVALID)
    }

    // ---------- SLObjectItf ----------

    pub(crate) fn realize(&self, a: &[u32]) -> R {
        let mut st = self.lock();
        let o = Self::object_mut(&mut st, a[0])?;
        if o.state != SL_OBJECT_STATE_UNREALIZED {
            return Err(SL_RESULT_PRECONDITIONS_VIOLATED);
        }
        o.state = SL_OBJECT_STATE_REALIZED;
        if let (true, Some((cb, ctx))) = (a[1] != 0, o.cb) {
            let args = vec![a[0], ctx, SL_OBJECT_EVENT_ASYNC_TERMINATION, SL_RESULT_SUCCESS, 0, 0];
            st.pending.push_back(GuestCall { addr: cb, args });
            drop(st);
            self.mixer.events().push(MixerEvent::Wake);
        }
        Ok(())
    }

    /// Objects are never suspended, so there is nothing to resume.
    pub(crate) fn resume(&self, a: &[u32]) -> R {
        Self::object_mut(&mut self.lock(), a[0])?;
        Err(SL_RESULT_PRECONDITIONS_VIOLATED)
    }

    pub(crate) fn get_state(&self, a: &[u32]) -> R {
        let state = Self::object_mut(&mut self.lock(), a[0])?.state;
        self.w32(a[1], state)
    }

    pub(crate) fn get_interface(&self, a: &[u32]) -> R {
        let want = iid_of(&self.read_iid(a[1])?);
        let mut st = self.lock();
        let o = Self::object_mut(&mut st, a[0])?;
        let kind = match want {
            Some(Iid::Object) => Some(Kind::Object),
            Some(Iid::Engine) => Some(Kind::Engine),
            Some(Iid::Play) => Some(Kind::Play),
            Some(Iid::BufferQueue | Iid::AndroidSimpleBufferQueue) => Some(Kind::Bq),
            Some(Iid::Volume) => Some(Kind::Volume),
            Some(Iid::Null | Iid::Record) | None => None,
        };
        let found = kind.and_then(|k| o.itfs.get(&k)).copied();
        self.w32(a[2], found.unwrap_or(0))?;
        found.map(|_| ()).ok_or(SL_RESULT_FEATURE_UNSUPPORTED)
    }

    pub(crate) fn object_register_callback(&self, a: &[u32]) -> R {
        Self::object_mut(&mut self.lock(), a[0])?.cb = (a[1] != 0).then_some((a[1], a[2]));
        Ok(())
    }

    pub(crate) fn destroy(&self, a: &[u32]) -> R {
        let mut st = self.lock();
        let o = st.objects.remove(&a[0]).ok_or(SL_RESULT_PARAMETER_INVALID)?;
        for s in &o.slots {
            st.itfs.remove(s);
            st.free.push(*s);
        }
        if let ObjKind::Player(p) = o.kind {
            self.mixer.destroy_stream(p.stream);
        }
        Ok(())
    }

    pub(crate) fn get_priority(&self, a: &[u32]) -> R {
        Self::object_mut(&mut self.lock(), a[0])?;
        self.w32(a[1], 0)?;
        self.w32(a[2], 0)
    }

    /// Priorities and loss-of-control settings are accepted and ignored.
    pub(crate) fn accept_on_object(&self, a: &[u32]) -> R {
        Self::object_mut(&mut self.lock(), a[0]).map(|_| ())
    }

    // ---------- SLEngineItf ----------

    pub(crate) fn create_output_mix(&self, a: &[u32]) -> R {
        // self, pMix, numInterfaces, ids, required
        let mut st = self.lock();
        Self::engine_check(&st, a[0])?;
        self.check_ifaces(a[2], a[3], a[4], &[Iid::Object])?;
        let obj = self.make_object(&mut st, ObjKind::OutputMix, &[]).ok_or(SL_RESULT_MEMORY_FAILURE)?;
        self.w32(a[1], obj)
    }

    pub(crate) fn create_audio_player(&self, a: &[u32]) -> R {
        // self, pPlayer, pAudioSrc, pAudioSnk, numInterfaces, ids, required
        let mut st = self.lock();
        Self::engine_check(&st, a[0])?;
        self.check_ifaces(
            a[4],
            a[5],
            a[6],
            &[Iid::Object, Iid::Play, Iid::BufferQueue, Iid::AndroidSimpleBufferQueue, Iid::Volume],
        )?;
        let (fmt, max_buffers) = self.parse_source(a[2])?;
        self.check_sink(&st, a[3])?;
        let stream = self.mixer.create_stream(fmt.rate_hz, max_buffers as usize);
        let player = Player { stream, fmt, bq_cb: None, play_cb: None, play_mask: 0, level_mb: 0, mute: false };
        let made = self.make_object(&mut st, ObjKind::Player(player), &[Kind::Play, Kind::Bq, Kind::Volume]);
        let Some(obj) = made else {
            self.mixer.destroy_stream(stream);
            return Err(SL_RESULT_MEMORY_FAILURE);
        };
        self.w32(a[1], obj).inspect_err(|_| {
            // The out pointer was bad: do not leak the half-made player.
            let o = st.objects.remove(&obj).expect("just created");
            for s in &o.slots {
                st.itfs.remove(s);
                st.free.push(*s);
            }
            self.mixer.destroy_stream(stream);
        })
    }

    /// Parse an `SLDataSource` of a buffer queue with PCM data.
    fn parse_source(&self, src: u32) -> Result<(PcmFormat, u32), u32> {
        let (loc, f) = (self.r32(src)?, self.r32(src + 4)?);
        if loc == 0 || f == 0 {
            return Err(SL_RESULT_PARAMETER_INVALID);
        }
        let ltype = self.r32(loc)?;
        if ltype != SL_DATALOCATOR_BUFFERQUEUE && ltype != SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE {
            return Err(SL_RESULT_CONTENT_UNSUPPORTED);
        }
        let buffers = self.r32(loc + 4)?;
        if buffers == 0 {
            return Err(SL_RESULT_PARAMETER_INVALID);
        }
        let ftype = self.r32(f)?;
        let (channels, mhz, bits) = (self.r32(f + 4)?, self.r32(f + 8)?, self.r32(f + 12)?);
        let (container, endian) = (self.r32(f + 16)?, self.r32(f + 24)?);
        let rep = if ftype == SL_ANDROID_DATAFORMAT_PCM_EX { self.r32(f + 28)? } else { 0 };
        let sample = match (ftype, bits, rep) {
            (SL_DATAFORMAT_PCM, 8, _) => Some(SampleFormat::U8),
            (SL_DATAFORMAT_PCM, 16, _) => Some(SampleFormat::S16),
            (SL_ANDROID_DATAFORMAT_PCM_EX, 8, SL_ANDROID_PCM_REPRESENTATION_UNSIGNED_INT) => Some(SampleFormat::U8),
            (SL_ANDROID_DATAFORMAT_PCM_EX, 16, 1) => Some(SampleFormat::S16),
            (SL_ANDROID_DATAFORMAT_PCM_EX, 32, SL_ANDROID_PCM_REPRESENTATION_FLOAT) => Some(SampleFormat::F32),
            _ => None,
        };
        let rate_hz = mhz / 1000;
        match sample {
            Some(sample)
                if (1..=2).contains(&channels)
                    && (4000..=192_000).contains(&rate_hz)
                    && container == bits
                    && endian == SL_BYTEORDER_LITTLEENDIAN =>
            {
                Ok((PcmFormat { rate_hz, channels: channels as u8, sample }, buffers))
            }
            _ => Err(SL_RESULT_CONTENT_UNSUPPORTED),
        }
    }

    /// The sink must be an output mix object we created.
    fn check_sink(&self, st: &State, snk: u32) -> R {
        let loc = self.r32(snk)?;
        if loc == 0 {
            return Err(SL_RESULT_PARAMETER_INVALID);
        }
        if self.r32(loc)? != SL_DATALOCATOR_OUTPUTMIX {
            return Err(SL_RESULT_CONTENT_UNSUPPORTED);
        }
        match st.objects.get(&self.r32(loc + 4)?) {
            Some(Object { kind: ObjKind::OutputMix, .. }) => Ok(()),
            _ => Err(SL_RESULT_PARAMETER_INVALID),
        }
    }

    pub(crate) fn is_extension_supported(&self, a: &[u32]) -> R {
        // self, name, pSupported
        self.w32(a[2], 0)
    }

    pub(crate) fn num_extensions(&self, a: &[u32]) -> R {
        // self, pNumExtensions
        self.w32(a[1], 0)
    }

    // ---------- SLPlayItf ----------

    pub(crate) fn set_play_state(&self, a: &[u32]) -> R {
        let state = match a[1] {
            SL_PLAYSTATE_STOPPED => PlayState::Stopped,
            SL_PLAYSTATE_PAUSED => PlayState::Paused,
            SL_PLAYSTATE_PLAYING => PlayState::Playing,
            _ => return Err(SL_RESULT_PARAMETER_INVALID),
        };
        let stream = Self::player_mut(&mut self.lock(), a[0], Kind::Play)?.stream;
        self.mixer.set_state(stream, state).map_err(|_| SL_RESULT_PARAMETER_INVALID)
    }

    pub(crate) fn get_play_state(&self, a: &[u32]) -> R {
        let stream = Self::player_mut(&mut self.lock(), a[0], Kind::Play)?.stream;
        let v = match self.mixer.state(stream).map_err(|_| SL_RESULT_PARAMETER_INVALID)? {
            PlayState::Stopped => SL_PLAYSTATE_STOPPED,
            PlayState::Paused => SL_PLAYSTATE_PAUSED,
            PlayState::Playing => SL_PLAYSTATE_PLAYING,
        };
        self.w32(a[1], v)
    }

    pub(crate) fn get_duration(&self, a: &[u32]) -> R {
        Self::player_mut(&mut self.lock(), a[0], Kind::Play)?;
        self.w32(a[1], SL_TIME_UNKNOWN)
    }

    pub(crate) fn get_position(&self, a: &[u32]) -> R {
        let stream = Self::player_mut(&mut self.lock(), a[0], Kind::Play)?.stream;
        let ms = self.mixer.position_ms(stream).map_err(|_| SL_RESULT_PARAMETER_INVALID)?;
        self.w32(a[1], ms)
    }

    pub(crate) fn play_register_callback(&self, a: &[u32]) -> R {
        Self::player_mut(&mut self.lock(), a[0], Kind::Play)?.play_cb = (a[1] != 0).then_some((a[1], a[2]));
        Ok(())
    }

    pub(crate) fn set_callback_events_mask(&self, a: &[u32]) -> R {
        Self::player_mut(&mut self.lock(), a[0], Kind::Play)?.play_mask = a[1];
        Ok(())
    }

    pub(crate) fn get_callback_events_mask(&self, a: &[u32]) -> R {
        let mask = Self::player_mut(&mut self.lock(), a[0], Kind::Play)?.play_mask;
        self.w32(a[1], mask)
    }

    // ---------- SLBufferQueueItf / SLAndroidSimpleBufferQueueItf ----------

    pub(crate) fn enqueue(&self, a: &[u32]) -> R {
        // self, pBuffer, size
        let (stream, fmt) = {
            let mut st = self.lock();
            let p = Self::player_mut(&mut st, a[0], Kind::Bq)?;
            (p.stream, p.fmt)
        };
        if a[1] == 0 || a[2] == 0 || a[2] as usize % fmt.frame_bytes() != 0 {
            return Err(SL_RESULT_PARAMETER_INVALID);
        }
        let bytes = self.mem.read_bytes(a[1], a[2]).map_err(|_| SL_RESULT_PARAMETER_INVALID)?;
        self.mixer.enqueue(stream, fmt.decode(&bytes)).map_err(|_| SL_RESULT_BUFFER_INSUFFICIENT)
    }

    pub(crate) fn clear(&self, a: &[u32]) -> R {
        let stream = Self::player_mut(&mut self.lock(), a[0], Kind::Bq)?.stream;
        self.mixer.clear(stream).map_err(|_| SL_RESULT_PARAMETER_INVALID)
    }

    pub(crate) fn queue_get_state(&self, a: &[u32]) -> R {
        let stream = Self::player_mut(&mut self.lock(), a[0], Kind::Bq)?.stream;
        let (count, index) = self.mixer.queue_state(stream).map_err(|_| SL_RESULT_PARAMETER_INVALID)?;
        self.w32(a[1], count)?;
        self.w32(a[1] + 4, index)
    }

    pub(crate) fn queue_register_callback(&self, a: &[u32]) -> R {
        Self::player_mut(&mut self.lock(), a[0], Kind::Bq)?.bq_cb = (a[1] != 0).then_some((a[1], a[2]));
        Ok(())
    }

    // ---------- SLVolumeItf ----------

    fn apply_volume(&self, p: &Player) {
        let gain = if p.mute { 0.0 } else { millibel_gain(p.level_mb) };
        // The stream exists as long as the player does.
        let _ = self.mixer.set_gain(p.stream, gain);
    }

    pub(crate) fn set_volume_level(&self, a: &[u32]) -> R {
        let level = a[1] as i16;
        if level > 0 {
            return Err(SL_RESULT_PARAMETER_INVALID);
        }
        let mut st = self.lock();
        let p = Self::player_mut(&mut st, a[0], Kind::Volume)?;
        p.level_mb = level;
        self.apply_volume(p);
        Ok(())
    }

    pub(crate) fn get_volume_level(&self, a: &[u32]) -> R {
        let level = Self::player_mut(&mut self.lock(), a[0], Kind::Volume)?.level_mb;
        self.w16(a[1], level as u16)
    }

    /// Unity is the loudest level the host offers.
    pub(crate) fn get_max_volume_level(&self, a: &[u32]) -> R {
        Self::player_mut(&mut self.lock(), a[0], Kind::Volume)?;
        self.w16(a[1], 0)
    }

    pub(crate) fn set_mute(&self, a: &[u32]) -> R {
        let mut st = self.lock();
        let p = Self::player_mut(&mut st, a[0], Kind::Volume)?;
        p.mute = a[1] != 0;
        self.apply_volume(p);
        Ok(())
    }

    pub(crate) fn get_mute(&self, a: &[u32]) -> R {
        let mute = Self::player_mut(&mut self.lock(), a[0], Kind::Volume)?.mute;
        self.w32(a[1], mute as u32)
    }

    /// Stereo positioning is not implemented; asking for it to be off is fine.
    pub(crate) fn enable_stereo_position(&self, a: &[u32]) -> R {
        Self::player_mut(&mut self.lock(), a[0], Kind::Volume)?;
        if a[1] != 0 {
            return Err(SL_RESULT_FEATURE_UNSUPPORTED);
        }
        Ok(())
    }

    /// `IsEnabledStereoPosition` and `GetStereoPosition`: always off / centre.
    pub(crate) fn write_zero(&self, a: &[u32]) -> R {
        Self::player_mut(&mut self.lock(), a[0], Kind::Volume)?;
        self.w32(a[1], 0)
    }

    // ---------- plain "not supported" entries ----------

    pub(crate) fn unsupported(&self, _a: &[u32]) -> R {
        Err(SL_RESULT_FEATURE_UNSUPPORTED)
    }

    // ---------- the pump side ----------

    /// Translate one mixer event into the guest calls it triggers.
    fn calls_for(&self, ev: MixerEvent, out: &mut Vec<GuestCall>) {
        let (stream, done) = match ev {
            MixerEvent::BufferDone(s) => (s, true),
            MixerEvent::QueueEmpty(s) => (s, false),
            MixerEvent::Wake => return,
        };
        let st = self.lock();
        for o in st.objects.values() {
            let ObjKind::Player(p) = &o.kind else { continue };
            if p.stream != stream {
                continue;
            }
            if done {
                if let Some((cb, ctx)) = p.bq_cb {
                    out.push(GuestCall { addr: cb, args: vec![o.itfs[&Kind::Bq], ctx] });
                }
            } else if let (Some((cb, ctx)), true) = (p.play_cb, p.play_mask & SL_PLAYEVENT_HEADATEND != 0) {
                out.push(GuestCall { addr: cb, args: vec![o.itfs[&Kind::Play], ctx, SL_PLAYEVENT_HEADATEND] });
            }
        }
    }

    /// Wait up to `wait` for events, then collect every guest call they (and pending asynchronous
    /// completions) require.
    pub(crate) fn collect_calls(&self, wait: Duration) -> Vec<GuestCall> {
        let mut calls = Vec::new();
        let mut ev = self.mixer.events().wait(wait);
        // Bounded so a flood of events cannot starve the caller's stop flag.
        for _ in 0..64 {
            let Some(e) = ev else { break };
            self.calls_for(e, &mut calls);
            ev = self.mixer.events().try_pop();
        }
        calls.extend(self.lock().pending.drain(..));
        calls
    }
}
