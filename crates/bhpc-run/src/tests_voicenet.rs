//! The unsequenced voice send: guards, the queue, and the guest calls (host thunks stand in for the game's ENet).

use crate::new_env;
use crate::voicenet::*;
use bhpc_cpu::Cpu;
use std::sync::{Arc, Mutex};

const DATA: u32 = 0x3000_0000;
/// The ivar offset variable, the `BHNetClientMatch` and the stack.
const IVAR: u32 = DATA;
const MATCHER: u32 = DATA + 0x100;
const PEER_OFFSET: u32 = 0x24;
const PEER: u32 = 0x4000_1234;
const SP: u32 = DATA + 0x8000;

#[derive(Default)]
struct Enet {
    /// (bytes, flags) of each packet made.
    created: Vec<(Vec<u8>, u32)>,
    /// (peer, channel, packet) of each send.
    sent: Vec<(u32, u32, u32)>,
    destroyed: Vec<u32>,
    /// `enet_peer_send` answers -1.
    refuse: bool,
    /// `enet_packet_create` answers NULL.
    no_memory: bool,
}

fn setup(destroy: bool) -> (Box<Cpu>, Arc<Mutex<Enet>>, Unsequenced) {
    let (cpu, _st) = new_env(Box::new(std::io::sink()), None).unwrap();
    cpu.mem().map(DATA, 0x10000).unwrap();
    cpu.mem().write_u32(IVAR, PEER_OFFSET).unwrap();
    cpu.mem().write_u32(MATCHER + PEER_OFFSET, PEER).unwrap();
    let enet = Arc::new(Mutex::new(Enet::default()));
    let e = enet.clone();
    let create = cpu
        .register_shared_thunk(
            "test_enet_packet_create",
            Arc::new(move |c: &mut Cpu| {
                let (p, n, f) = (c.reg(0), c.reg(1), c.reg(2));
                let bytes = c.mem().read_bytes(p, n).unwrap();
                let mut g = e.lock().unwrap();
                g.created.push((bytes, f));
                let r = if g.no_memory { 0 } else { 0x5000_0000 + g.created.len() as u32 };
                c.set_reg(0, r);
            }),
        )
        .unwrap();
    let e = enet.clone();
    let send = cpu
        .register_shared_thunk(
            "test_enet_peer_send",
            Arc::new(move |c: &mut Cpu| {
                let mut g = e.lock().unwrap();
                g.sent.push((c.reg(0), c.reg(1), c.reg(2)));
                c.set_reg(0, if g.refuse { u32::MAX } else { 0 });
            }),
        )
        .unwrap();
    let e = enet.clone();
    let destroy_fn = cpu
        .register_shared_thunk(
            "test_enet_packet_destroy",
            Arc::new(move |c: &mut Cpu| e.lock().unwrap().destroyed.push(c.reg(0))),
        )
        .unwrap();
    let names = [
        ("enet_packet_create", create),
        ("enet_peer_send", send),
        ("enet_packet_destroy", destroy_fn),
        ("OBJC_IVAR_$_BHNetClientMatch.enetPeer", IVAR),
    ];
    let ex = EnetExports::resolve(|n| {
        names.iter().find(|(k, _)| *k == n && (destroy || *k != "enet_packet_destroy")).map(|(_, a)| *a)
    })
    .unwrap();
    assert_eq!(ex.packet_destroy.is_some(), destroy);
    let mut cpu = cpu;
    cpu.set_sp(SP);
    cpu.set_ticks(1_000_000);
    (cpu, enet, Unsequenced::new(ex))
}

#[test]
fn resolve_needs_the_three_exports() {
    let all = |n: &str| Some(n.len() as u32);
    assert!(EnetExports::resolve(all).is_ok());
    for missing in ["enet_packet_create", "enet_peer_send", "OBJC_IVAR_$_BHNetClientMatch.enetPeer"] {
        let e = EnetExports::resolve(|n| (n != missing).then_some(1)).unwrap_err();
        assert_eq!(e, format!("no {missing} export"));
    }
    assert_eq!(FEATURE, "voice-unsequenced");
}

#[test]
fn queue_guards() {
    let o = Outbox::default();
    assert_eq!(o.queue(&[0xbc, 0x16]), Err(SendError::NotInWorld));
    o.set_in_world(true);
    assert!(o.in_world());
    assert_eq!(o.queue(&[]), Err(SendError::Empty));
    assert_eq!(o.queue(&vec![0; MAX_PAYLOAD + 1]), Err(SendError::TooBig(MAX_PAYLOAD + 1)));
    o.queue(&vec![0; MAX_PAYLOAD]).unwrap();
    for i in 0..QUEUE_LEN as u8 {
        o.queue(&[i]).unwrap();
    }
    assert_eq!(o.pending(), QUEUE_LEN);
    assert_eq!(o.stats().overflow, 1, "the oldest went");
    o.set_in_world(true);
    assert_eq!(o.pending(), QUEUE_LEN);
    o.set_in_world(false);
    assert_eq!(o.pending(), 0, "leaving the world drops what waits");
    o.set_in_world(false);
    let texts: Vec<String> = [SendError::Empty, SendError::TooBig(2000), SendError::NotInWorld, SendError::Unsupported]
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(texts[1], "payload of 2000 bytes is over 1364");
    assert!(texts.iter().all(|t| !t.is_empty()));
}

#[test]
fn flush_sends_unsequenced_on_channel_0_to_the_match_peer() {
    let (mut cpu, enet, u) = setup(true);
    assert_eq!(u.flush(&mut cpu, MATCHER), 0, "nothing queued");
    u.outbox.set_in_world(true);
    u.outbox.queue(&[0xbc, 0x16, 1, 2, 3]).unwrap();
    u.outbox.queue(&[0xbc, 0x16, 4]).unwrap();
    assert_eq!(u.flush(&mut cpu, MATCHER), 2);
    assert_eq!(cpu.sp(), SP, "the stack is restored");
    let g = enet.lock().unwrap();
    assert_eq!(g.created, vec![(vec![0xbc, 0x16, 1, 2, 3], 2), (vec![0xbc, 0x16, 4], 2)]);
    assert_eq!(g.sent, vec![(PEER, 0, 0x5000_0001), (PEER, 0, 0x5000_0002)]);
    assert!(g.destroyed.is_empty());
    drop(g);
    assert_eq!(u.outbox.stats(), Stats { sent: 2, overflow: 0, refused: 0 });
    assert_eq!(u.outbox.pending(), 0);
}

#[test]
fn refused_packets_are_freed_and_counted() {
    let (mut cpu, enet, u) = setup(true);
    u.outbox.set_in_world(true);
    enet.lock().unwrap().refuse = true;
    u.outbox.queue(&[1]).unwrap();
    assert_eq!(u.flush(&mut cpu, MATCHER), 0);
    assert_eq!(enet.lock().unwrap().destroyed, vec![0x5000_0001]);
    // No packet memory: nothing to send or free.
    let mut g = enet.lock().unwrap();
    (g.refuse, g.no_memory) = (false, true);
    drop(g);
    u.outbox.queue(&[2]).unwrap();
    assert_eq!(u.flush(&mut cpu, MATCHER), 0);
    assert_eq!(enet.lock().unwrap().sent.len(), 1);
    // No peer (no match, or not connected yet): the payloads are dropped.
    u.outbox.queue(&[3]).unwrap();
    assert_eq!(u.flush(&mut cpu, 0), 0);
    cpu.mem().write_u32(MATCHER + PEER_OFFSET, 0).unwrap();
    u.outbox.queue(&[4]).unwrap();
    assert_eq!(u.flush(&mut cpu, MATCHER), 0);
    assert_eq!(u.outbox.stats().refused, 4);
}

#[test]
fn without_packet_destroy_a_refused_packet_is_left_alone() {
    let (mut cpu, enet, u) = setup(false);
    u.outbox.set_in_world(true);
    enet.lock().unwrap().refuse = true;
    u.outbox.queue(&[1]).unwrap();
    assert_eq!(u.flush(&mut cpu, MATCHER), 0);
    assert!(enet.lock().unwrap().destroyed.is_empty());
}

#[test]
fn an_unwritable_stack_sends_nothing() {
    let (mut cpu, enet, u) = setup(true);
    u.outbox.set_in_world(true);
    u.outbox.queue(&[1, 2, 3]).unwrap();
    cpu.set_sp(0x100);
    assert_eq!(u.flush(&mut cpu, MATCHER), 0);
    assert!(enet.lock().unwrap().created.is_empty());
    assert_eq!(cpu.sp(), 0x100);
}

#[test]
fn pc_voice_io_over_the_devices() {
    use bhpc_audio::capture::MicControl;
    use bhpc_audio::mixer::Mixer;
    use bhpc_audio::voice::VoiceOut;
    use bhpc_mods::voice::{MicDevice, VoiceIo};
    use std::net::{IpAddr, Ipv4Addr, UdpSocket};
    let mic = Arc::new(MicControl::default());
    let out = Arc::new(VoiceOut::new(Mixer::new(48_000)));
    let outbox = Arc::new(Outbox::default());
    let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = server.local_addr().unwrap().port();
    let mut io = PcVoiceIo::new(mic.clone(), out.clone(), Some(outbox.clone()), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    // Microphone.
    assert_eq!(io.mic(), MicDevice::Closed);
    io.set_mic(true);
    assert!(mic.wanted());
    assert_eq!(io.mic(), MicDevice::Closed);
    assert_eq!(io.mic_chunk(), None);
    struct Dev(bool);
    impl bhpc_audio::capture::CaptureBackend for Dev {
        fn devices(&mut self) -> Vec<String> {
            Vec::new()
        }
        fn open(
            &mut self,
            _: Option<&str>,
            _: u32,
            _: u16,
            _: bhpc_audio::capture::CaptureSink,
        ) -> Result<Box<dyn std::any::Any>, String> {
            if self.0 {
                Ok(Box::new(()))
            } else {
                Err("no device".into())
            }
        }
    }
    let mut drv = bhpc_audio::capture::MicDriver::new(mic.clone());
    drv.sync(&mut Dev(false));
    assert_eq!(io.mic(), MicDevice::Failed);
    mic.set_device(Some("other".into()));
    drv.sync(&mut Dev(true));
    assert_eq!(io.mic(), MicDevice::Open);
    // Sends.
    io.send_reliable(vec![0xbc, 0x15]);
    assert_eq!(io.take_reliable(), vec![vec![0xbc, 0x15]]);
    assert!(io.take_reliable().is_empty());
    assert!(!io.send_unsequenced(&[0xbc, 0x16]), "not in a world");
    outbox.set_in_world(true);
    assert!(io.send_unsequenced(&[0xbc, 0x16]));
    assert_eq!(outbox.pending(), 1);
    assert!(io.recv_udp().is_empty(), "no socket before the first send");
    io.send_udp(port, b"hello");
    let mut buf = [0u8; 16];
    let (n, from) = server.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"hello");
    server.send_to(b"back", from).unwrap();
    let mut got = Vec::new();
    for _ in 0..200 {
        got = io.recv_udp();
        if !got.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(got, vec![b"back".to_vec()]);
    // Playback.
    assert_eq!(io.out_queued(), 0);
    io.out_push(&[0.1; 1920]);
    assert_eq!(io.out_queued(), 960);
    io.out_clear();
    assert!(out.speakers().is_empty());
    // Without a server address or an outbox nothing goes out.
    let mut none = PcVoiceIo::new(mic, out, None, None);
    none.send_udp(port, b"x");
    assert!(none.recv_udp().is_empty());
    assert!(!none.send_unsequenced(&[1]));
    let v6 = PcVoiceIo::new(
        Arc::new(MicControl::default()),
        Arc::new(VoiceOut::new(Mixer::new(48_000))),
        None,
        Some(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
    );
    let mut v6 = v6;
    v6.send_udp(port, b"x");
}
