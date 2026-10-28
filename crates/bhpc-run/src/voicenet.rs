//! Unsequenced in-band send for voice chat: frames go out on the game's own ENet connection as
//! `ENET_PACKET_FLAG_UNSEQUENCED` packets, through the game's exported `enet_packet_create` / `enet_peer_send`, to the
//! peer in `BHNetClientMatch.enetPeer` (notes/voice-transport.md section 2).
//!
//! ENet is not thread-safe and the game services it on its main thread (t0), so the voice code on any thread only
//! [`Outbox::queue`]s payloads; the mod glue's frame hook (t0) calls [`Unsequenced::flush`]. The game's
//! `sendData:...withDataMode:` is not used: what the client does with its modes is not known, and the reliable 0xBC
//! path stays as it is.
//!
//! Guards: only while in a world on a server ([`Outbox::set_in_world`]; leaving drops what is queued), payloads of
//! 1..=[`MAX_PAYLOAD`] bytes (above ENet's fragment threshold an unsequenced packet would turn into reliable
//! fragments), at most [`QUEUE_LEN`] waiting (the oldest goes: voice is useless late).

use bhpc_cpu::Cpu;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// `ENET_PACKET_FLAG_UNSEQUENCED` (ENet 1.3).
pub const ENET_PACKET_FLAG_UNSEQUENCED: u32 = 2;
/// The ENet channel voice uses (the server relays on channel 0; the vanilla client opens 8).
pub const CHANNEL: u32 = 0;
/// Largest payload: ENet's fragment threshold for the default MTU 1392.
pub const MAX_PAYLOAD: usize = 1364;
/// Payloads waiting for the next frame at most (~640 ms of 20 ms frames).
pub const QUEUE_LEN: usize = 32;

/// The `HELLO.features` entry this capability stands for.
pub const FEATURE: &str = bhpc_mods::voice::FEATURE_VOICE_UNSEQUENCED;

/// Guest addresses of what the send needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnetExports {
    pub packet_create: u32,
    pub peer_send: u32,
    /// Frees a packet `enet_peer_send` refused (`None`: the packet leaks, a few bytes per refusal).
    pub packet_destroy: Option<u32>,
    /// `OBJC_IVAR_$_BHNetClientMatch.enetPeer`: the address of the ivar's offset.
    pub peer_ivar: u32,
}

impl EnetExports {
    /// Looks the exports up (`export` finds a symbol in the loaded libraries).
    pub fn resolve(export: impl Fn(&str) -> Option<u32>) -> Result<EnetExports, String> {
        let need = |n: &str| export(n).ok_or_else(|| format!("no {n} export"));
        Ok(EnetExports {
            packet_create: need("enet_packet_create")?,
            peer_send: need("enet_peer_send")?,
            packet_destroy: export("enet_packet_destroy"),
            peer_ivar: need("OBJC_IVAR_$_BHNetClientMatch.enetPeer")?,
        })
    }
}

/// Why a payload was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    Empty,
    TooBig(usize),
    /// Not in a world on a server.
    NotInWorld,
    /// The game does not export what the send needs.
    Unsupported,
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::Empty => f.write_str("empty payload"),
            SendError::TooBig(n) => write!(f, "payload of {n} bytes is over {MAX_PAYLOAD}"),
            SendError::NotInWorld => f.write_str("not in a world on a server"),
            SendError::Unsupported => f.write_str("unsequenced send is not available in this game build"),
        }
    }
}

impl std::error::Error for SendError {}

/// Counters, for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub sent: u64,
    /// Dropped because the queue was full.
    pub overflow: u64,
    /// Refused by ENet (peer not connected) or no peer.
    pub refused: u64,
}

/// Payloads waiting for the main thread (any thread may queue).
#[derive(Default)]
pub struct Outbox {
    q: Mutex<VecDeque<Vec<u8>>>,
    in_world: AtomicBool,
    sent: AtomicU64,
    overflow: AtomicU64,
    refused: AtomicU64,
}

impl Outbox {
    /// Queue one payload (whole 0xBC packet: `bc 16 ...`) for the next frame.
    pub fn queue(&self, payload: &[u8]) -> Result<(), SendError> {
        if payload.is_empty() {
            return Err(SendError::Empty);
        }
        if payload.len() > MAX_PAYLOAD {
            return Err(SendError::TooBig(payload.len()));
        }
        if !self.in_world.load(Ordering::SeqCst) {
            return Err(SendError::NotInWorld);
        }
        let mut q = self.q.lock().unwrap_or_else(PoisonError::into_inner);
        if q.len() >= QUEUE_LEN {
            q.pop_front();
            self.overflow.fetch_add(1, Ordering::Relaxed);
        }
        q.push_back(payload.to_vec());
        Ok(())
    }

    /// The glue says whether a world on a server runs; leaving drops what waits.
    pub fn set_in_world(&self, on: bool) {
        if !self.in_world.swap(on, Ordering::SeqCst) || on {
            return;
        }
        self.q.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    pub fn in_world(&self) -> bool {
        self.in_world.load(Ordering::SeqCst)
    }

    pub fn pending(&self) -> usize {
        self.q.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    pub fn stats(&self) -> Stats {
        Stats {
            sent: self.sent.load(Ordering::Relaxed),
            overflow: self.overflow.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
        }
    }

    fn take(&self) -> Vec<Vec<u8>> {
        self.q.lock().unwrap_or_else(PoisonError::into_inner).drain(..).collect()
    }
}

/// The send path: the exports plus the queue.
pub struct Unsequenced {
    pub exports: EnetExports,
    pub outbox: Arc<Outbox>,
}

impl Unsequenced {
    pub fn new(exports: EnetExports) -> Unsequenced {
        Unsequenced { exports, outbox: Arc::new(Outbox::default()) }
    }

    /// The `ENetPeer*` of a `BHNetClientMatch` (0 when unknown).
    pub fn peer(&self, c: &mut Cpu, matcher: u32) -> u32 {
        if matcher == 0 {
            return 0;
        }
        let off = c.mem().read_u32(self.exports.peer_ivar).unwrap_or(0);
        c.mem().read_u32(matcher.wrapping_add(off)).unwrap_or(0)
    }

    /// Send everything queued to the server of `matcher` (the `BHNetClientMatch`). Main thread (t0) only, from the
    /// frame hook: ENet is not thread-safe. Returns how many packets ENet took.
    pub fn flush(&self, c: &mut Cpu, matcher: u32) -> usize {
        let all = self.outbox.take();
        if all.is_empty() {
            return 0;
        }
        let peer = self.peer(c, matcher);
        let mut sent = 0;
        for p in all {
            if peer != 0 && self.send_one(c, peer, &p) {
                sent += 1;
            } else {
                self.outbox.refused.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.outbox.sent.fetch_add(sent as u64, Ordering::Relaxed);
        sent
    }

    /// `enet_peer_send(peer, 0, enet_packet_create(bytes, n, UNSEQUENCED))`; the bytes sit on the guest stack (ENet
    /// copies them).
    fn send_one(&self, c: &mut Cpu, peer: u32, bytes: &[u8]) -> bool {
        let e = self.exports;
        let sp = c.sp();
        let at = sp.wrapping_sub(bytes.len() as u32 + 16) & !7;
        if c.mem().write_bytes(at, bytes).is_err() {
            return false;
        }
        c.set_sp(at);
        let packet =
            c.call(e.packet_create, &[at, bytes.len() as u32, ENET_PACKET_FLAG_UNSEQUENCED]).map_or(0, |r| r.r0);
        let accepted = packet != 0 && c.call(e.peer_send, &[peer, CHANNEL, packet]).is_ok_and(|r| (r.r0 as i32) >= 0);
        if packet != 0 && !accepted {
            // A refused packet stays ours.
            if let Some(d) = e.packet_destroy {
                let _ = c.call(d, &[packet]);
            }
        }
        c.set_sp(sp);
        accepted
    }
}
