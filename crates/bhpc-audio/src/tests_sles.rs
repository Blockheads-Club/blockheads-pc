use crate::mixer::*;
use crate::sles::consts::*;
use crate::sles::*;
use bhpc_cpu::{Cpu, Stop};
use bhpc_mem::GuestMemory;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Vtable indices (OpenSL ES 1.0.1).
const O_REALIZE: u32 = 0;
const O_RESUME: u32 = 1;
const O_GETSTATE: u32 = 2;
const O_GETIFACE: u32 = 3;
const O_REGCB: u32 = 4;
const O_ABORT: u32 = 5;
const O_DESTROY: u32 = 6;
const O_SETPRIO: u32 = 7;
const O_GETPRIO: u32 = 8;
const O_SETLOSS: u32 = 9;
const E_CREATE_PLAYER: u32 = 2;
const E_CREATE_RECORDER: u32 = 3;
const E_CREATE_MIX: u32 = 7;
const E_NUM_EXT: u32 = 12;
const E_IS_EXT: u32 = 14;
const P_SET_STATE: u32 = 0;
const P_GET_STATE: u32 = 1;
const P_GET_DURATION: u32 = 2;
const P_GET_POSITION: u32 = 3;
const P_REGCB: u32 = 4;
const P_SET_MASK: u32 = 5;
const P_GET_MASK: u32 = 6;
const Q_ENQUEUE: u32 = 0;
const Q_CLEAR: u32 = 1;
const Q_GETSTATE: u32 = 2;
const Q_REGCB: u32 = 3;
const V_SET_LEVEL: u32 = 0;
const V_GET_LEVEL: u32 = 1;
const V_GET_MAX: u32 = 2;
const V_SET_MUTE: u32 = 3;
const V_GET_MUTE: u32 = 4;
const V_EN_STEREO: u32 = 5;
const V_IS_STEREO: u32 = 6;
const V_SET_STEREO: u32 = 7;
const V_GET_STEREO: u32 = 8;

const CTX: u32 = 0xC0DE;

struct Env {
    cpu: Box<Cpu>,
    sles: Arc<Sles>,
    mixer: Mixer,
    mem: Arc<GuestMemory>,
    bump: u32,
    log: Arc<Mutex<Vec<Vec<u32>>>>,
    cb: u32,
    /// A callback that stops the guest.
    fail: u32,
}

struct Player {
    obj: u32,
    play: u32,
    bq: u32,
    vol: u32,
}

impl Env {
    fn new() -> Env {
        let mem = Arc::new(GuestMemory::new().unwrap());
        let mut cpu = Cpu::new(mem.clone()).unwrap();
        let stack = mem.map_anywhere(1 << 20, 4096).unwrap();
        cpu.set_sp(stack + (1 << 20));
        let scratch = mem.map_anywhere(1 << 20, 4096).unwrap();
        let mixer = Mixer::new(44100);
        let sles = Sles::install(&cpu, mixer.clone()).unwrap();
        let log: Arc<Mutex<Vec<Vec<u32>>>> = Arc::default();
        let l = log.clone();
        let cb = cpu
            .table()
            .register_shared_thunk(
                "test_callback",
                Arc::new(move |c: &mut Cpu| {
                    let mut cur = c.args();
                    let args = (0..6).map(|_| cur.u32(c)).collect();
                    l.lock().unwrap().push(args);
                    c.ret_u32(0);
                }),
            )
            .unwrap();
        let fail = cpu
            .table()
            .register_shared_thunk("test_fail", Arc::new(|c: &mut Cpu| c.request_stop(Stop::Host("boom".into()))))
            .unwrap();
        Env { cpu, sles, mixer, mem, bump: scratch, log, cb, fail }
    }

    fn alloc(&mut self, n: u32) -> u32 {
        let a = self.bump;
        self.bump += (n + 15) & !15;
        a
    }

    fn r(&self, a: u32) -> u32 {
        self.mem.read_u32(a).unwrap()
    }

    fn put(&mut self, words: &[u32]) -> u32 {
        let a = self.alloc(4 * words.len() as u32);
        for (i, w) in words.iter().enumerate() {
            self.mem.write_u32(a + 4 * i as u32, *w).unwrap();
        }
        a
    }

    fn call(&mut self, addr: u32, args: &[u32]) -> u32 {
        self.cpu.call(addr, args).unwrap().r0
    }

    fn vcall(&mut self, itf: u32, idx: u32, args: &[u32]) -> u32 {
        let f = self.r(self.r(itf) + 4 * idx);
        let mut a = vec![itf];
        a.extend_from_slice(args);
        self.call(f, &a)
    }

    /// Call method `idx` of the vtable that `table_of` uses, with `self_h` as the receiver.
    fn via(&mut self, table_of: u32, idx: u32, self_h: u32, args: &[u32]) -> u32 {
        let f = self.r(self.r(table_of) + 4 * idx);
        let mut a = vec![self_h];
        a.extend_from_slice(args);
        self.call(f, &a)
    }

    fn sym(&self, name: &str) -> u32 {
        self.sles.lookup(name).unwrap().0
    }

    /// Address of the SLInterfaceID_ struct that `SL_IID_xxx` points at.
    fn iid(&self, name: &str) -> u32 {
        self.r(self.sym(name))
    }

    fn get_itf(&mut self, obj: u32, iid: &str) -> (u32, u32) {
        let out = self.put(&[0xdead]);
        let iid = self.iid(iid);
        let code = self.vcall(obj, O_GETIFACE, &[iid, out]);
        (code, self.r(out))
    }

    fn engine(&mut self) -> (u32, u32) {
        let out = self.put(&[0]);
        let create = self.sym("slCreateEngine");
        assert_eq!(self.call(create, &[out, 0, 0, 0, 0, 0]), SL_RESULT_SUCCESS);
        let obj = self.r(out);
        assert_eq!(self.vcall(obj, O_REALIZE, &[0]), SL_RESULT_SUCCESS);
        let (code, itf) = self.get_itf(obj, "SL_IID_ENGINE");
        assert_eq!(code, SL_RESULT_SUCCESS);
        (obj, itf)
    }

    fn mix(&mut self, eng: u32) -> u32 {
        let out = self.put(&[0]);
        assert_eq!(self.vcall(eng, E_CREATE_MIX, &[out, 0, 0, 0]), SL_RESULT_SUCCESS);
        self.r(out)
    }

    fn pcm(&mut self, ch: u32, hz: u32, bits: u32) -> u32 {
        self.put(&[SL_DATAFORMAT_PCM, ch, hz * 1000, bits, bits, 3, SL_BYTEORDER_LITTLEENDIAN])
    }

    fn src(&mut self, ltype: u32, buffers: u32, fmt: u32) -> u32 {
        let loc = self.put(&[ltype, buffers]);
        self.put(&[loc, fmt])
    }

    fn snk(&mut self, mix: u32) -> u32 {
        let loc = self.put(&[SL_DATALOCATOR_OUTPUTMIX, mix]);
        self.put(&[loc, 0])
    }

    fn create_player(&mut self, eng: u32, src: u32, snk: u32) -> (u32, u32) {
        let out = self.put(&[0xdead]);
        let code = self.vcall(eng, E_CREATE_PLAYER, &[out, src, snk, 0, 0, 0]);
        (code, self.r(out))
    }

    /// Stereo 16-bit 44.1 kHz player with a 4-buffer Android simple buffer queue.
    fn player(&mut self, eng: u32, mix: u32) -> Player {
        let fmt = self.pcm(2, 44100, 16);
        let src = self.src(SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE, 4, fmt);
        let snk = self.snk(mix);
        let (code, obj) = self.create_player(eng, src, snk);
        assert_eq!(code, SL_RESULT_SUCCESS);
        assert_eq!(self.vcall(obj, O_REALIZE, &[0]), SL_RESULT_SUCCESS);
        let play = self.get_itf(obj, "SL_IID_PLAY").1;
        let bq = self.get_itf(obj, "SL_IID_ANDROIDSIMPLEBUFFERQUEUE").1;
        let vol = self.get_itf(obj, "SL_IID_VOLUME").1;
        Player { obj, play, bq, vol }
    }

    fn setup(&mut self) -> (u32, u32, Player) {
        let (_, eng) = self.engine();
        let mix = self.mix(eng);
        let p = self.player(eng, mix);
        (eng, mix, p)
    }

    /// Queue `frames` (stereo i16) on a player's buffer queue.
    fn enqueue(&mut self, p: &Player, frames: &[[i16; 2]]) -> u32 {
        let bytes: Vec<u8> = frames.iter().flatten().flat_map(|s| s.to_le_bytes()).collect();
        let buf = self.alloc(bytes.len() as u32);
        self.mem.write_bytes(buf, &bytes).unwrap();
        self.vcall(p.bq, Q_ENQUEUE, &[buf, bytes.len() as u32])
    }

    fn fill(&self, frames: usize) -> Vec<i16> {
        let mut v = vec![0i16; frames * 2];
        self.mixer.fill(&mut v);
        v
    }

    fn pump(&mut self) -> usize {
        self.sles.pump_once(&mut self.cpu, Duration::ZERO).unwrap()
    }

    fn calls(&self) -> Vec<Vec<u32>> {
        self.log.lock().unwrap().clone()
    }
}

#[test]
fn exports_and_data_symbols() {
    let e = Env::new();
    let names: Vec<_> = e.sles.exports().iter().map(|x| x.0).collect();
    assert_eq!(
        names,
        [
            "SL_IID_ANDROIDSIMPLEBUFFERQUEUE",
            "SL_IID_BUFFERQUEUE",
            "SL_IID_ENGINE",
            "SL_IID_NULL",
            "SL_IID_OBJECT",
            "SL_IID_PLAY",
            "SL_IID_RECORD",
            "SL_IID_VOLUME",
            "slCreateEngine"
        ]
    );
    assert_eq!(e.sles.lookup("slCreateEngine").unwrap().1, ExportKind::Function);
    assert_eq!(e.sles.lookup("SL_IID_PLAY").unwrap().1, ExportKind::Data);
    assert_eq!(e.sles.lookup("slNope"), None);
    // The data word points at a 16-byte ID with the published layout (time_low, little endian, first).
    let play = e.mem.read_bytes(e.iid("SL_IID_PLAY"), 16).unwrap();
    assert_eq!(&play[..4], &0xef0b_d9c0u32.to_le_bytes());
    assert_eq!(e.mem.read_bytes(e.iid("SL_IID_NULL"), 16).unwrap(), vec![0; 16]);
    // 10 + 15 + 12 + 4 + 9 vtable methods plus slCreateEngine, all with distinct names.
    let mut t = e.sles.thunk_names().to_vec();
    assert_eq!(t.len(), 51);
    t.sort();
    t.dedup();
    assert_eq!(t.len(), 51);
    assert_eq!(e.sles.mixer().out_rate(), 44100);
    assert_eq!(e.sles.live_objects(), 0);
}

#[test]
fn reinstalling_rebinds_and_errors_are_typed() {
    // A second installation re-registers the same thunk names: allowed, the newest wins.
    let e = Env::new();
    let again = Sles::install(&e.cpu, Mixer::new(48000)).unwrap();
    assert_ne!(again.lookup("SL_IID_PLAY"), e.sles.lookup("SL_IID_PLAY"));
    assert!(SlesError::from(bhpc_mem::MemError::OutOfSpace).to_string().contains("opensles"));
    assert!(SlesError::from(bhpc_cpu::CpuError::TooManyThunks).to_string().contains("opensles"));
}

#[test]
fn full_playback_flow() {
    let mut e = Env::new();
    let (eng, mix, p) = e.setup();
    assert_eq!(e.sles.live_objects(), 3); // engine, output mix, player
    let _ = (eng, mix);
    assert_eq!(e.vcall(p.bq, Q_REGCB, &[e.cb, CTX]), SL_RESULT_SUCCESS);
    // The standard BUFFERQUEUE IID answers with the very same interface.
    assert_eq!(e.get_itf(p.obj, "SL_IID_BUFFERQUEUE"), (SL_RESULT_SUCCESS, p.bq));
    assert_eq!(e.get_itf(p.obj, "SL_IID_OBJECT"), (SL_RESULT_SUCCESS, p.obj));

    assert_eq!(e.enqueue(&p, &[[100, -100], [200, -200]]), SL_RESULT_SUCCESS);
    let st = e.put(&[0, 0]);
    assert_eq!(e.vcall(p.bq, Q_GETSTATE, &[st]), SL_RESULT_SUCCESS);
    assert_eq!((e.r(st), e.r(st + 4)), (1, 0));

    // Stopped: the device gets silence and no callback happens.
    let ps = e.put(&[0]);
    assert_eq!(e.vcall(p.play, P_GET_STATE, &[ps]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(ps), SL_PLAYSTATE_STOPPED);
    assert_eq!(e.fill(1), [0, 0]);
    assert_eq!(e.pump(), 0);

    assert_eq!(e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.play, P_GET_STATE, &[ps]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(ps), SL_PLAYSTATE_PLAYING);
    assert_eq!(e.fill(2), [100, -100, 200, -200]);
    assert_eq!(e.pump(), 1);
    let calls = e.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0][..2], [p.bq, CTX]);
    assert_eq!(e.vcall(p.bq, Q_GETSTATE, &[st]), SL_RESULT_SUCCESS);
    assert_eq!((e.r(st), e.r(st + 4)), (0, 1));

    // Position and duration.
    let pos = e.put(&[0xdead]);
    assert_eq!(e.vcall(p.play, P_GET_POSITION, &[pos]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(pos), 0, "2 frames at 44.1 kHz is under a millisecond");
    assert_eq!(e.vcall(p.play, P_GET_DURATION, &[pos]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(pos), SL_TIME_UNKNOWN);

    // Pausing and stopping.
    assert_eq!(e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PAUSED]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.play, P_GET_STATE, &[ps]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(ps), SL_PLAYSTATE_PAUSED);
    assert_eq!(e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_STOPPED]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.play, P_SET_STATE, &[99]), SL_RESULT_PARAMETER_INVALID);
}

#[test]
fn clearing_unregistering_and_queue_limits() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    for _ in 0..4 {
        assert_eq!(e.enqueue(&p, &[[1, 1]]), SL_RESULT_SUCCESS);
    }
    assert_eq!(e.enqueue(&p, &[[1, 1]]), SL_RESULT_BUFFER_INSUFFICIENT);
    assert_eq!(e.vcall(p.bq, Q_CLEAR, &[]), SL_RESULT_SUCCESS);
    let st = e.put(&[7, 7]);
    assert_eq!(e.vcall(p.bq, Q_GETSTATE, &[st]), SL_RESULT_SUCCESS);
    assert_eq!((e.r(st), e.r(st + 4)), (0, 0));
    // Argument checks of Enqueue.
    let buf = e.alloc(64);
    assert_eq!(e.vcall(p.bq, Q_ENQUEUE, &[0, 4]), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(e.vcall(p.bq, Q_ENQUEUE, &[buf, 0]), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(e.vcall(p.bq, Q_ENQUEUE, &[buf, 6]), SL_RESULT_PARAMETER_INVALID, "not whole frames");
    assert_eq!(e.vcall(p.bq, Q_ENQUEUE, &[0x10, 4]), SL_RESULT_PARAMETER_INVALID, "unmapped");
    // A callback can be removed again; then completion calls nothing.
    assert_eq!(e.vcall(p.bq, Q_REGCB, &[e.cb, CTX]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.bq, Q_REGCB, &[0, 0]), SL_RESULT_SUCCESS);
    e.enqueue(&p, &[[5, 5]]);
    e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
    e.fill(1);
    assert_eq!(e.pump(), 0);
    // The play interface is not a buffer queue and vice versa.
    assert_eq!(e.via(p.bq, Q_CLEAR, p.play, &[]), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(e.via(p.play, P_SET_STATE, p.bq, &[SL_PLAYSTATE_PLAYING]), SL_RESULT_PARAMETER_INVALID);
}

#[test]
fn head_at_end_event() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
    let mask = e.put(&[0xdead]);
    assert_eq!(e.vcall(p.play, P_GET_MASK, &[mask]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(mask), 0);
    // A callback without the event enabled hears nothing.
    assert_eq!(e.vcall(p.play, P_REGCB, &[e.cb, CTX]), SL_RESULT_SUCCESS);
    e.enqueue(&p, &[[1, 1]]);
    e.fill(1);
    assert_eq!(e.pump(), 0);
    // Enabled: told when the queue runs dry.
    assert_eq!(e.vcall(p.play, P_SET_MASK, &[SL_PLAYEVENT_HEADATEND]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.play, P_GET_MASK, &[mask]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(mask), SL_PLAYEVENT_HEADATEND);
    e.enqueue(&p, &[[1, 1]]);
    e.fill(1);
    assert_eq!(e.pump(), 1);
    assert_eq!(e.calls()[0][..3], [p.play, CTX, SL_PLAYEVENT_HEADATEND]);
    // Without a callback the enabled mask is harmless.
    assert_eq!(e.vcall(p.play, P_REGCB, &[0, 0]), SL_RESULT_SUCCESS);
    e.enqueue(&p, &[[1, 1]]);
    e.fill(1);
    assert_eq!(e.pump(), 0);
}

#[test]
fn volume_interface() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
    let out = e.put(&[0xdead_beef]);
    assert_eq!(e.vcall(p.vol, V_GET_MAX, &[out]), SL_RESULT_SUCCESS);
    assert_eq!(e.mem.read_u16(out).unwrap(), 0);
    assert_eq!(e.vcall(p.vol, V_GET_LEVEL, &[out]), SL_RESULT_SUCCESS);
    assert_eq!(e.mem.read_u16(out).unwrap(), 0);
    assert_eq!(e.vcall(p.vol, V_SET_LEVEL, &[(-600i16) as u16 as u32]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.vol, V_GET_LEVEL, &[out]), SL_RESULT_SUCCESS);
    assert_eq!(e.mem.read_u16(out).unwrap() as i16, -600);
    assert_eq!(e.vcall(p.vol, V_SET_LEVEL, &[100]), SL_RESULT_PARAMETER_INVALID, "louder than unity");
    e.enqueue(&p, &[[10000, -10000]]);
    let half = (10000.0 * 10f32.powf(-0.3)) as i16;
    assert_eq!(e.fill(1), [half, -half]);
    // Mute overrides the level; unmuting restores it.
    assert_eq!(e.vcall(p.vol, V_SET_MUTE, &[1]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.vol, V_GET_MUTE, &[out]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(out), 1);
    e.enqueue(&p, &[[10000, -10000]]);
    assert_eq!(e.fill(1), [0, 0]);
    assert_eq!(e.vcall(p.vol, V_SET_MUTE, &[0]), SL_RESULT_SUCCESS);
    e.enqueue(&p, &[[10000, -10000]]);
    assert_eq!(e.fill(1), [half, -half]);
    // Stereo positioning stays off.
    assert_eq!(e.vcall(p.vol, V_EN_STEREO, &[0]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.vol, V_EN_STEREO, &[1]), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(e.vcall(p.vol, V_IS_STEREO, &[out]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(out), 0);
    assert_eq!(e.vcall(p.vol, V_SET_STEREO, &[5]), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(e.vcall(p.vol, V_GET_STEREO, &[out]), SL_RESULT_SUCCESS);
    // The volume interface of the wrong object is refused.
    assert_eq!(e.via(p.vol, V_SET_MUTE, p.play, &[1]), SL_RESULT_PARAMETER_INVALID);
}

#[test]
fn object_interface_semantics() {
    let mut e = Env::new();
    let (eng_obj, eng) = e.engine();
    let st = e.put(&[0]);
    assert_eq!(e.vcall(eng_obj, O_GETSTATE, &[st]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(st), SL_OBJECT_STATE_REALIZED);
    assert_eq!(e.vcall(eng_obj, O_REALIZE, &[0]), SL_RESULT_PRECONDITIONS_VIOLATED);
    assert_eq!(e.vcall(eng_obj, O_RESUME, &[0]), SL_RESULT_PRECONDITIONS_VIOLATED);
    assert_eq!(e.vcall(eng_obj, O_GETSTATE, &[0x10]), SL_RESULT_PARAMETER_INVALID);
    // A fresh mix is unrealized until Realize.
    let mix = e.mix(eng);
    assert_eq!(e.vcall(mix, O_GETSTATE, &[st]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(st), SL_OBJECT_STATE_UNREALIZED);
    // Interfaces: unknown or unimplemented ones are reported with a NULL out value.
    assert_eq!(e.get_itf(mix, "SL_IID_RECORD"), (SL_RESULT_FEATURE_UNSUPPORTED, 0));
    assert_eq!(e.get_itf(mix, "SL_IID_NULL"), (SL_RESULT_FEATURE_UNSUPPORTED, 0));
    assert_eq!(e.get_itf(mix, "SL_IID_PLAY"), (SL_RESULT_FEATURE_UNSUPPORTED, 0));
    let junk = e.put(&[1, 2, 3, 4]);
    let out = e.put(&[0xdead]);
    assert_eq!(e.vcall(mix, O_GETIFACE, &[junk, out]), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(e.vcall(mix, O_GETIFACE, &[0x10, out]), SL_RESULT_PARAMETER_INVALID);
    let iid = e.iid("SL_IID_OBJECT");
    assert_eq!(e.vcall(mix, O_GETIFACE, &[iid, 0x10]), SL_RESULT_PARAMETER_INVALID);
    // Priority and loss-of-control are accepted.
    assert_eq!(e.vcall(mix, O_SETPRIO, &[1, 0]), SL_RESULT_SUCCESS);
    let pp = e.put(&[9, 9]);
    assert_eq!(e.vcall(mix, O_GETPRIO, &[pp, pp + 4]), SL_RESULT_SUCCESS);
    assert_eq!((e.r(pp), e.r(pp + 4)), (0, 0));
    assert_eq!(e.vcall(mix, O_SETLOSS, &[0, 0, 0]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(mix, O_ABORT, &[]), SL_RESULT_SUCCESS);
    // Every object method rejects a handle that is not an object.
    for idx in [O_REALIZE, O_RESUME, O_GETSTATE, O_REGCB, O_ABORT, O_DESTROY, O_SETPRIO, O_GETPRIO, O_SETLOSS] {
        assert_eq!(
            e.via(mix, idx, eng, &[]),
            SL_RESULT_PARAMETER_INVALID,
            "object method {idx} on an engine interface"
        );
    }
    // Destroy returns slots and rejects stale handles.
    let free = e.sles.free_slots();
    e.vcall(mix, O_DESTROY, &[]);
    assert_eq!(e.sles.free_slots(), free + 1);
    assert_eq!(e.vcall(mix, O_GETSTATE, &[st]), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(e.vcall(mix, O_DESTROY, &[]), SL_RESULT_PARAMETER_INVALID);
}

#[test]
fn async_realize_completes_through_the_pump() {
    let mut e = Env::new();
    let (_, eng) = e.engine();
    let mix = e.mix(eng);
    assert_eq!(e.vcall(mix, O_REGCB, &[e.cb, CTX]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(mix, O_REALIZE, &[1]), SL_RESULT_SUCCESS);
    assert_eq!(e.pump(), 1);
    assert_eq!(e.calls()[0], [mix, CTX, SL_OBJECT_EVENT_ASYNC_TERMINATION, SL_RESULT_SUCCESS, 0, 0]);
    // Without a callback, asynchronous realize just works; unregistering is possible.
    let mix2 = e.mix(eng);
    assert_eq!(e.vcall(mix2, O_REALIZE, &[1]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(mix2, O_REGCB, &[0, 0]), SL_RESULT_SUCCESS);
    assert_eq!(e.pump(), 0);
}

#[test]
fn player_creation_accepts_the_formats_android_does() {
    let mut e = Env::new();
    let (_, eng) = e.engine();
    let mix = e.mix(eng);
    let snk = e.snk(mix);
    let formats = [
        e.pcm(1, 8000, 8),
        e.put(&[SL_ANDROID_DATAFORMAT_PCM_EX, 2, 48_000_000, 32, 32, 3, 2, SL_ANDROID_PCM_REPRESENTATION_FLOAT]),
        e.put(&[SL_ANDROID_DATAFORMAT_PCM_EX, 2, 22_050_000, 16, 16, 3, 2, 1]),
        e.put(&[SL_ANDROID_DATAFORMAT_PCM_EX, 1, 22_050_000, 8, 8, 4, 2, SL_ANDROID_PCM_REPRESENTATION_UNSIGNED_INT]),
    ];
    for (i, f) in formats.into_iter().enumerate() {
        let ltype = if i % 2 == 0 { SL_DATALOCATOR_BUFFERQUEUE } else { SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE };
        let src = e.src(ltype, 2, f);
        let (code, obj) = e.create_player(eng, src, snk);
        assert_eq!(code, SL_RESULT_SUCCESS, "format {i}");
        assert_ne!(obj, 0);
    }
    assert_eq!(e.mixer.stream_count(), 4);
}

#[test]
fn player_creation_rejects_bad_requests_without_leaking() {
    let mut e = Env::new();
    let (eng_obj, eng) = e.engine();
    let mix = e.mix(eng);
    let snk = e.snk(mix);
    let good = e.pcm(2, 44100, 16);
    let (slots, objects) = (e.sles.free_slots(), e.sles.live_objects());
    let bq = SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE;

    let bad_formats: Vec<Vec<u32>> = vec![
        vec![SL_DATAFORMAT_PCM, 3, 44_100_000, 16, 16, 3, 2], // 3 channels
        vec![SL_DATAFORMAT_PCM, 0, 44_100_000, 16, 16, 3, 2], // 0 channels
        vec![SL_DATAFORMAT_PCM, 2, 100, 16, 16, 3, 2],        // rate below 4 kHz
        vec![SL_DATAFORMAT_PCM, 2, 44_100_000, 16, 16, 3, 3], // big endian
        vec![SL_DATAFORMAT_PCM, 2, 44_100_000, 16, 32, 3, 2], // container != bits
        vec![SL_DATAFORMAT_PCM, 2, 44_100_000, 24, 24, 3, 2], // 24 bit
        vec![1, 2, 44_100_000, 16, 16, 3, 2],                 // MIME format
        vec![SL_ANDROID_DATAFORMAT_PCM_EX, 2, 44_100_000, 16, 16, 3, 2, 3], // 16 bit float
        vec![SL_ANDROID_DATAFORMAT_PCM_EX, 2, 44_100_000, 8, 8, 3, 2, 1], // signed 8 bit
    ];
    for f in bad_formats {
        let fmt = e.put(&f);
        let src = e.src(bq, 2, fmt);
        assert_eq!(e.create_player(eng, src, snk).0, SL_RESULT_CONTENT_UNSUPPORTED, "{f:?}");
    }
    let src_uri = e.src(1, 2, good);
    assert_eq!(e.create_player(eng, src_uri, snk).0, SL_RESULT_CONTENT_UNSUPPORTED);
    let src0 = e.src(bq, 0, good);
    assert_eq!(e.create_player(eng, src0, snk).0, SL_RESULT_PARAMETER_INVALID);
    let nulls = [e.put(&[0, good]), e.put(&[0x10, 0]), e.put(&[bq, 0])];
    // (loc = 0), (fmt = 0 with a valid-looking locator address is not mapped), and a null format.
    for src in [nulls[0], nulls[1], 0] {
        assert_eq!(e.create_player(eng, src, snk).0, SL_RESULT_PARAMETER_INVALID);
    }
    let loc_null_fmt = e.put(&[0, 0]);
    assert_eq!(e.create_player(eng, loc_null_fmt, snk).0, SL_RESULT_PARAMETER_INVALID);

    // Sinks.
    let src = e.src(bq, 2, good);
    let not_mix_loc = e.put(&[3, mix]);
    let not_mix = e.put(&[not_mix_loc, 0]);
    assert_eq!(e.create_player(eng, src, not_mix).0, SL_RESULT_CONTENT_UNSUPPORTED);
    let eng_as_mix = e.snk(eng_obj);
    assert_eq!(e.create_player(eng, src, eng_as_mix).0, SL_RESULT_PARAMETER_INVALID);
    let bogus_mix = e.snk(0xdead_0000);
    assert_eq!(e.create_player(eng, src, bogus_mix).0, SL_RESULT_PARAMETER_INVALID);
    let null_loc_sink = e.put(&[0, 0]);
    assert_eq!(e.create_player(eng, src, null_loc_sink).0, SL_RESULT_PARAMETER_INVALID);
    assert_eq!(e.create_player(eng, src, 0).0, SL_RESULT_PARAMETER_INVALID);

    // The engine handle must be an engine interface.
    let out = e.put(&[0]);
    assert_eq!(e.via(eng, E_CREATE_PLAYER, mix, &[out, src, snk, 0, 0, 0]), SL_RESULT_PARAMETER_INVALID);
    // A bad out pointer undoes the creation.
    assert_eq!(e.vcall(eng, E_CREATE_PLAYER, &[0x10, src, snk, 0, 0, 0]), SL_RESULT_PARAMETER_INVALID);

    assert_eq!((e.sles.free_slots(), e.sles.live_objects()), (slots, objects));
    assert_eq!(e.mixer.stream_count(), 0);
}

#[test]
fn interface_requests_are_checked() {
    let mut e = Env::new();
    let (_, eng) = e.engine();
    let mix = e.mix(eng);
    let snk = e.snk(mix);
    let fmt = e.pcm(2, 44100, 16);
    let src = e.src(SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE, 2, fmt);
    let (play, rec, junk) = (e.iid("SL_IID_PLAY"), e.iid("SL_IID_RECORD"), e.put(&[7, 7, 7, 7]));
    let ids = e.put(&[play, rec, junk]);
    let optional = e.put(&[0, 0, 0]);
    let required_play = e.put(&[1, 0, 0]);
    let required_rec = e.put(&[0, 1, 0]);
    let required_junk = e.put(&[0, 0, 1]);
    let out = e.put(&[0]);
    let create = |e: &mut Env, n: u32, ids: u32, req: u32| e.vcall(eng, E_CREATE_PLAYER, &[out, src, snk, n, ids, req]);
    assert_eq!(create(&mut e, 3, ids, optional), SL_RESULT_SUCCESS, "optional interfaces may be missing");
    assert_eq!(create(&mut e, 3, ids, required_play), SL_RESULT_SUCCESS);
    assert_eq!(create(&mut e, 3, ids, required_rec), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(create(&mut e, 3, ids, required_junk), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(create(&mut e, 33, ids, optional), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(create(&mut e, 1, 0, optional), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(create(&mut e, 1, ids, 0), SL_RESULT_PARAMETER_INVALID);
    let bad_list = e.put(&[0x10]);
    assert_eq!(create(&mut e, 1, bad_list, optional), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(create(&mut e, 1, ids, 0x10), SL_RESULT_PARAMETER_INVALID);
    // The mix supports only the object interface.
    let mix_out = e.put(&[0]);
    assert_eq!(e.vcall(eng, E_CREATE_MIX, &[mix_out, 1, ids, required_play]), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(e.vcall(eng, E_CREATE_MIX, &[mix_out, 1, ids, optional]), SL_RESULT_SUCCESS);
    assert_eq!(e.via(eng, E_CREATE_MIX, mix, &[mix_out, 0, 0, 0]), SL_RESULT_PARAMETER_INVALID, "not an engine");
    // slCreateEngine honours the same rules (engine and object interfaces only).
    let create_engine = e.sym("slCreateEngine");
    let eng_iid = e.iid("SL_IID_ENGINE");
    let eng_ids = e.put(&[eng_iid]);
    let one = e.put(&[1]);
    assert_eq!(e.call(create_engine, &[out, 0, 0, 1, eng_ids, one]), SL_RESULT_SUCCESS);
    assert_eq!(e.call(create_engine, &[out, 0, 0, 1, ids, required_play]), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(e.call(create_engine, &[0x10, 0, 0, 0, 0, 0]), SL_RESULT_PARAMETER_INVALID);
}

#[test]
fn unsupported_and_trivial_engine_entries() {
    let mut e = Env::new();
    let (_, eng) = e.engine();
    let out = e.put(&[0xdead]);
    assert_eq!(e.vcall(eng, E_CREATE_RECORDER, &[out]), SL_RESULT_FEATURE_UNSUPPORTED);
    assert_eq!(e.vcall(eng, E_NUM_EXT, &[out]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(out), 0);
    e.mem.write_u32(out, 5).unwrap();
    assert_eq!(e.vcall(eng, E_IS_EXT, &[out, out]), SL_RESULT_SUCCESS);
    assert_eq!(e.r(out), 0);
    // Sweep: every method of every vtable is callable with garbage and answers with a plain result code.
    for name in e.sles.thunk_names().to_vec() {
        let addr = e.cpu.thunk_addr(&name).unwrap();
        let r = e.call(addr, &[0, 0, 0, 0, 0, 0, 0]);
        assert!(r == SL_RESULT_PARAMETER_INVALID || r == SL_RESULT_FEATURE_UNSUPPORTED, "{name} -> {r}");
    }
}

#[test]
fn handle_slots_run_out_gracefully() {
    let mut e = Env::new();
    let (_, eng) = e.engine();
    let out = e.put(&[0]);
    let mut made = 0;
    while e.sles.free_slots() > 3 {
        assert_eq!(e.vcall(eng, E_CREATE_MIX, &[out, 0, 0, 0]), SL_RESULT_SUCCESS);
        made += 1;
    }
    assert!(made > 1000);
    // A player needs four slots.
    let fmt = e.pcm(2, 44100, 16);
    let src = e.src(SL_DATALOCATOR_ANDROIDSIMPLEBUFFERQUEUE, 2, fmt);
    let snk = e.snk(e.r(out));
    assert_eq!(e.create_player(eng, src, snk).0, SL_RESULT_MEMORY_FAILURE);
    assert_eq!(e.mixer.stream_count(), 0);
    // Two slots left for an engine (object + engine interface), one short for another... use them up.
    let create = e.sym("slCreateEngine");
    assert_eq!(e.call(create, &[out, 0, 0, 0, 0, 0]), SL_RESULT_SUCCESS);
    assert_eq!(e.call(create, &[out, 0, 0, 0, 0, 0]), SL_RESULT_MEMORY_FAILURE);
    assert_eq!(e.vcall(eng, E_CREATE_MIX, &[out, 0, 0, 0]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(eng, E_CREATE_MIX, &[out, 0, 0, 0]), SL_RESULT_MEMORY_FAILURE);
}

#[test]
fn destroying_a_player_removes_its_stream() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    assert_eq!(e.mixer.stream_count(), 1);
    e.vcall(p.bq, Q_REGCB, &[e.cb, CTX]);
    e.enqueue(&p, &[[1, 1]]);
    e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
    e.fill(1);
    e.vcall(p.obj, O_DESTROY, &[]);
    assert_eq!(e.mixer.stream_count(), 0);
    // Events of the dead stream are dropped, and its interfaces no longer resolve.
    assert_eq!(e.pump(), 0);
    assert_eq!(e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(e.vcall(p.bq, Q_CLEAR, &[]), SL_RESULT_PARAMETER_INVALID);
    assert_eq!(e.sles.live_objects(), 2);
}

#[test]
fn a_failing_guest_callback_surfaces_as_a_stop() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    e.vcall(p.bq, Q_REGCB, &[e.fail, CTX]);
    e.enqueue(&p, &[[1, 1]]);
    e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
    e.fill(1);
    let r = e.sles.pump_once(&mut e.cpu, Duration::ZERO);
    assert_eq!(r, Err(Stop::Host("boom".into())));
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_pump_thread_delivers_callbacks() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    let stack = e.mem.map_anywhere(1 << 20, 4096).unwrap();
    let handle = e.sles.spawn_pump(&e.cpu, stack + (1 << 20)).unwrap();
    assert!(!handle.is_finished());
    e.vcall(p.bq, Q_REGCB, &[e.cb, CTX]);
    e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
    for i in 1..=3 {
        e.enqueue(&p, &[[i, i]]);
        e.fill(1);
        let log = e.log.clone();
        wait_until("callback", move || log.lock().unwrap().len() == i as usize);
    }
    assert_eq!(e.calls()[2][..2], [p.bq, CTX]);
    assert_eq!(handle.stop(), Ok(()));
}

#[test]
fn the_pump_thread_reports_guest_failures_and_stops_on_drop() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    let stack = e.mem.map_anywhere(1 << 20, 4096).unwrap();
    let handle = e.sles.spawn_pump(&e.cpu, stack + (1 << 20)).unwrap();
    e.vcall(p.bq, Q_REGCB, &[e.fail, CTX]);
    e.enqueue(&p, &[[1, 1]]);
    e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
    e.fill(1);
    wait_until("the pump to give up", || handle.is_finished());
    assert!(handle.stop().is_err());
    // Dropping a healthy handle stops its thread as well.
    let handle = e.sles.spawn_pump(&e.cpu, stack + (1 << 20)).unwrap();
    drop(handle);
}

#[test]
fn a_cpu_limit_is_reported_by_spawn_pump() {
    let e = Env::new();
    let stack = e.mem.map_anywhere(4096, 4096).unwrap();
    let mut cpus = Vec::new();
    while let Ok(c) = e.cpu.new_thread() {
        cpus.push(c);
    }
    assert!(e.sles.spawn_pump(&e.cpu, stack).is_err());
}

#[test]
fn events_go_to_the_player_they_belong_to() {
    let mut e = Env::new();
    let (eng, mix, a) = e.setup();
    let b = e.player(eng, mix);
    e.vcall(b.bq, Q_REGCB, &[e.cb, CTX]);
    for p in [&a, &b] {
        e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]);
        e.enqueue(p, &[[100, 100]]);
    }
    // Both streams mix into the one output.
    assert_eq!(e.fill(1), [200, 200]);
    // Only the second player has a callback.
    assert_eq!(e.pump(), 1);
    assert_eq!(e.calls()[0][..2], [b.bq, CTX]);
}

/// What libOpenAL's opensles backend does, in ARM: the buffer-queue callback enqueues again through
/// the vtable (`ctx` is the address of a 4-byte buffer), from the pump's own CPU.
#[test]
fn a_real_arm_callback_keeps_the_queue_fed() {
    let mut e = Env::new();
    let (_, _, p) = e.setup();
    let code = e.alloc(32);
    let arm = [
        0xE590_3000u32, // ldr r3, [r0]       ; vtable of the queue interface
        0xE593_3000,    // ldr r3, [r3]       ; Enqueue (slot 0)
        0xE3A0_2004,    // mov r2, #4         ; size of one stereo s16 frame
        0xE12F_FF13,    // bx  r3             ; Enqueue(bq, ctx, 4)
    ];
    for (i, w) in arm.iter().enumerate() {
        e.mem.write_u32(code + 4 * i as u32, *w).unwrap();
    }
    let frame = e.put(&[0x0123_0456]);
    let stack = e.mem.map_anywhere(1 << 20, 4096).unwrap();
    let pump = e.sles.spawn_pump(&e.cpu, stack + (1 << 20)).unwrap();
    assert_eq!(e.vcall(p.bq, Q_REGCB, &[code, frame]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.play, P_SET_STATE, &[SL_PLAYSTATE_PLAYING]), SL_RESULT_SUCCESS);
    assert_eq!(e.vcall(p.bq, Q_ENQUEUE, &[frame, 4]), SL_RESULT_SUCCESS);
    for round in 1..=5u32 {
        assert_eq!(e.fill(1), [0x0456, 0x0123]);
        // The callback re-enqueued from the pump thread.
        let mixer = e.mixer.clone();
        // Stream 1 is the player's: it has one buffer again and has played `round` of them.
        wait_until("the refill", || mixer.stats().frames_out >= round as u64 && mixer.queue_state(1) == Ok((1, round)));
    }
    assert_eq!(pump.stop(), Ok(()));
}
