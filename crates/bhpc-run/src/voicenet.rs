//! Voice chat I/O of the PC client: voice frames go only to the server's dedicated voice UDP port (mod-sdk decision
//! 2026-10-28); the control messages (`VOICE_KEY`, `VOICE_CTRL`) are reliable 0xBC packets the mod glue sends.

use std::sync::Arc;

/// The speaker id of the voice client's finished mix on [`bhpc_audio::voice::VoiceOut`] (the client mixes every
/// speaker itself, with pan and muffle).
pub const MIX_STREAM: u32 = u32::MAX;

/// The PC's devices and sends for the voice client ([`bhpc_mods::voice::VoiceIo`]): the microphone seam, the mixer
/// stream of the mix, reliable packets for the glue to send, and a UDP socket to the server's voice port (the only
/// path of voice frames).
pub struct PcVoiceIo {
    mic: Arc<bhpc_audio::capture::MicControl>,
    out: Arc<bhpc_audio::voice::VoiceOut>,
    reliable: Vec<Vec<u8>>,
    /// The server's address (the UDP port's host).
    server: Option<std::net::IpAddr>,
    udp: Option<std::net::UdpSocket>,
}

impl PcVoiceIo {
    pub fn new(
        mic: Arc<bhpc_audio::capture::MicControl>,
        out: Arc<bhpc_audio::voice::VoiceOut>,
        server: Option<std::net::IpAddr>,
    ) -> PcVoiceIo {
        PcVoiceIo { mic, out, reliable: Vec::new(), server, udp: None }
    }

    /// The reliable packets the voice client asked for since the last call (the glue sends them as 0xBC).
    pub fn take_reliable(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.reliable)
    }

    fn socket(&mut self, ip: std::net::IpAddr) -> Option<&std::net::UdpSocket> {
        if self.udp.is_none() {
            let any: std::net::SocketAddr = match ip {
                std::net::IpAddr::V4(_) => (std::net::Ipv4Addr::UNSPECIFIED, 0).into(),
                std::net::IpAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
            };
            self.udp = std::net::UdpSocket::bind(any).ok().filter(|s| s.set_nonblocking(true).is_ok());
        }
        self.udp.as_ref()
    }
}

impl bhpc_mods::voice::VoiceIo for PcVoiceIo {
    fn set_mic(&mut self, on: bool) {
        self.mic.set_wanted(on);
    }

    fn mic(&self) -> bhpc_mods::voice::MicDevice {
        use bhpc_mods::voice::MicDevice;
        if self.mic.live() {
            MicDevice::Open
        } else if self.mic.wanted() && self.mic.last_error().is_some() {
            MicDevice::Failed
        } else {
            MicDevice::Closed
        }
    }

    fn mic_chunk(&mut self) -> Option<Vec<i16>> {
        self.mic.pop()
    }

    fn send_reliable(&mut self, packet: Vec<u8>) {
        self.reliable.push(packet);
    }

    fn send_udp(&mut self, port: u16, datagram: &[u8]) {
        let Some(ip) = self.server else { return };
        if let Some(s) = self.socket(ip) {
            // Lost datagrams are voice that arrives late anyway.
            let _ = s.send_to(datagram, (ip, port));
        }
    }

    fn recv_udp(&mut self) -> Vec<Vec<u8>> {
        let (Some(s), Some(ip)) = (self.udp.as_ref(), self.server) else { return Vec::new() };
        let mut out = Vec::new();
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = s.recv_from(&mut buf) {
            if from.ip() == ip {
                out.push(buf[..n].to_vec());
            }
        }
        out
    }

    fn out_queued(&self) -> usize {
        self.out.queued(self.out.voice_stream(MIX_STREAM)).unwrap_or(0)
    }

    fn out_push(&mut self, stereo: &[f32]) {
        // A full stream drops the frame: late voice is no use.
        let _ = self.out.push_stereo(self.out.voice_stream(MIX_STREAM), stereo);
    }

    fn out_clear(&mut self) {
        self.out.clear();
    }
}
