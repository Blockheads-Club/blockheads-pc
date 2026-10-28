//! The PC's voice I/O: microphone, reliable packets, the voice UDP socket and playback.

use crate::voicenet::*;
use std::sync::Arc;

#[test]
fn pc_voice_io_over_the_devices() {
    use bhpc_audio::capture::MicControl;
    use bhpc_audio::mixer::Mixer;
    use bhpc_audio::voice::VoiceOut;
    use bhpc_mods::voice::{MicDevice, VoiceIo};
    use std::net::{IpAddr, Ipv4Addr, UdpSocket};
    let mic = Arc::new(MicControl::default());
    let out = Arc::new(VoiceOut::new(Mixer::new(48_000)));
    let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = server.local_addr().unwrap().port();
    let mut io = PcVoiceIo::new(mic.clone(), out.clone(), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
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
    io.send_udp(port, b"again");
    let (n, _) = server.recv_from(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"again", "the same socket");
    // Playback.
    assert_eq!(io.out_queued(), 0);
    io.out_push(&[0.1; 1920]);
    assert_eq!(io.out_queued(), 960);
    io.out_clear();
    assert!(out.speakers().is_empty());
    // Without a server address nothing goes out.
    let mut none = PcVoiceIo::new(mic, out, None);
    none.send_udp(port, b"x");
    assert!(none.recv_udp().is_empty());
    let v6 = PcVoiceIo::new(
        Arc::new(MicControl::default()),
        Arc::new(VoiceOut::new(Mixer::new(48_000))),
        Some(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
    );
    let mut v6 = v6;
    v6.send_udp(port, b"x");
}
