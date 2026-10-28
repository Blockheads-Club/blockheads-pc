//! Voice: HELLO features, the pump between a voice client and the PC's devices, prefs, key events.

use std::cell::{Cell, RefCell};

use crate::voice::*;
use bhmod_core::types::ApiError;
use bhmod_core::voice::{Icons, MicState, Mode, VoiceEvent};
use bhmod_core::VoiceHost;
use bhmod_proto::Message;
use bhpc_input::VoiceMode;

#[test]
fn hello_features() {
    assert_eq!(features(true), vec!["settings", "voice"]);
    assert_eq!(features(false), vec!["settings"]);
    assert_eq!(mode_of(VoiceMode::Proximity), Mode::Proximity);
    assert_eq!(mode_of(VoiceMode::Channel), Mode::Channel);
    assert_eq!(mode_of(VoiceMode::Global), Mode::Global);
    assert_eq!(mode_of(VoiceMode::Broadcast), Mode::Broadcast);
}

#[test]
fn talk_keys_queue_and_track_what_is_held() {
    let k = TalkKeys::default();
    k.post_key("v", true);
    k.post(VoiceMode::Proximity, true);
    k.post(VoiceMode::Global, true);
    k.post(VoiceMode::Proximity, false);
    assert_eq!(k.held(), vec![VoiceMode::Global]);
    let ev = k.take();
    assert_eq!(ev.len(), 4);
    assert_eq!(ev[0], KeyIn::Key { name: "v", pressed: true });
    assert_eq!(ev[3], KeyIn::Talk { mode: VoiceMode::Proximity, pressed: false });
    assert!(k.take().is_empty());
    k.post(VoiceMode::Global, false);
    assert!(k.held().is_empty());
}

#[test]
fn prefs_round_trip_and_default_on_bad_files() {
    let d = tempfile::tempdir().unwrap();
    let p = VoicePrefs::path(&d.path().join("sub"));
    assert_eq!(VoicePrefs::load(&p), VoicePrefs::default());
    let v = VoicePrefs { mic: Consent::Granted, client: "master=150".into(), launcher_master: Some(80) };
    v.save(&p).unwrap();
    assert_eq!(VoicePrefs::load(&p), v);
    std::fs::write(&p, "mic = 7").unwrap();
    assert_eq!(VoicePrefs::load(&p), VoicePrefs::default());
    assert!(VoicePrefs::default().save(std::path::Path::new("")).is_err());
}

/// A voice client that records what the pump did.
#[derive(Default)]
pub(crate) struct FakeDriver {
    pub wanted: Cell<bool>,
    pub mic: RefCell<Vec<MicState>>,
    pub frames: Cell<usize>,
    pub udp_in: Cell<usize>,
    pub messages: RefCell<Vec<Message>>,
    pub talk: RefCell<Vec<(Option<Mode>, bool)>>,
    pub send: RefCell<Vec<Outgoing>>,
    pub port: Cell<Option<u16>>,
    pub prefs: RefCell<String>,
    pub icons: Cell<Option<Icons>>,
    pub events: RefCell<Vec<VoiceEvent>>,
    pub no_key: Cell<bool>,
    pub names: RefCell<Vec<(String, String)>>,
    pub extra_speakers: RefCell<Vec<String>>,
}

impl VoiceHost for FakeDriver {
    fn talk(&self, mode: Option<Mode>, pressed: bool) -> Result<(), ApiError> {
        self.talk.borrow_mut().push((mode, pressed));
        Ok(())
    }
    fn icons(&self) -> Result<Icons, ApiError> {
        self.icons.get().ok_or(ApiError::Unsupported)
    }
}

impl VoiceDriver for FakeDriver {
    fn as_host(&self) -> &dyn VoiceHost {
        self
    }
    fn start(&self) -> Option<Vec<u8>> {
        (!self.no_key.get()).then(|| vec![0xbc, 0x15])
    }
    fn on_message(&self, msg: &Message) {
        self.messages.borrow_mut().push(msg.clone());
    }
    fn on_udp(&self, _: &[u8]) {
        self.udp_in.set(self.udp_in.get() + 1);
    }
    fn on_mic_frame(&self, pcm: &[f32]) {
        assert_eq!(pcm.len(), FRAME);
        self.frames.set(self.frames.get() + 1);
    }
    fn poll_send(&self, _: u64) -> Vec<Outgoing> {
        std::mem::take(&mut self.send.borrow_mut())
    }
    fn mix(&self, out: &mut [f32]) {
        out[0] = 0.5;
    }
    fn mic_wanted(&self) -> bool {
        self.wanted.get()
    }
    fn mic_state(&self, s: MicState) {
        self.mic.borrow_mut().push(s);
    }
    fn transmitting(&self) -> Option<Mode> {
        self.wanted.get().then_some(Mode::Global)
    }
    fn take_events(&self) -> Vec<VoiceEvent> {
        std::mem::take(&mut self.events.borrow_mut())
    }
    fn udp_port(&self) -> Option<u16> {
        self.port.get()
    }
    fn players(&self) -> Vec<(String, String)> {
        self.names.borrow().clone()
    }
    fn speakers(&self) -> Vec<String> {
        self.names.borrow().iter().map(|n| n.0.clone()).chain(self.extra_speakers.borrow().iter().cloned()).collect()
    }
    fn alias(&self, player: &str) -> Option<String> {
        self.names.borrow().iter().find(|(p, _)| p == player).map(|(_, a)| a.clone())
    }
    fn prefs(&self) -> String {
        self.prefs.borrow().clone()
    }
    fn load_prefs(&self, text: &str) {
        *self.prefs.borrow_mut() = text.to_string();
    }
}

#[derive(Default)]
struct FakeIo {
    want: Option<bool>,
    device: Option<MicDevice>,
    chunks: Vec<Vec<i16>>,
    reliable: Vec<Vec<u8>>,
    udp: Vec<(u16, Vec<u8>)>,
    udp_in: Vec<Vec<u8>>,
    queued: usize,
    pushed: Vec<Vec<f32>>,
    cleared: bool,
}

impl VoiceIo for FakeIo {
    fn set_mic(&mut self, on: bool) {
        self.want = Some(on);
    }
    fn mic(&self) -> MicDevice {
        self.device.unwrap_or(MicDevice::Closed)
    }
    fn mic_chunk(&mut self) -> Option<Vec<i16>> {
        self.chunks.pop()
    }
    fn send_reliable(&mut self, p: Vec<u8>) {
        self.reliable.push(p);
    }
    fn send_udp(&mut self, port: u16, d: &[u8]) {
        self.udp.push((port, d.to_vec()));
    }
    fn recv_udp(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.udp_in)
    }
    fn out_queued(&self) -> usize {
        self.queued
    }
    fn out_push(&mut self, s: &[f32]) {
        assert_eq!(s.len(), FRAME * 2);
        self.queued += FRAME;
        self.pushed.push(s.to_vec());
    }
    fn out_clear(&mut self) {
        self.cleared = true;
    }
}

#[test]
fn pump_starts_sends_receives_and_mixes() {
    let d = FakeDriver::default();
    let mut io = FakeIo::default();
    let mut p = VoicePump::new(Consent::Granted);
    assert_eq!(p.consent_state(), Consent::Granted);
    d.send.borrow_mut().extend([Outgoing::Reliable(vec![1]), Outgoing::Udp(vec![3])]);
    io.udp_in.push(vec![9]);
    let out = p.frame(&d, &mut io, 1);
    assert_eq!(io.reliable, vec![vec![0xbc, 0x15], vec![1]]);
    assert!(io.udp.is_empty(), "no port yet: the datagram is dropped");
    assert_eq!(d.udp_in.get(), 1);
    assert_eq!(io.pushed.len(), OUT_TARGET);
    assert_eq!(io.pushed[0][0], 0.5);
    assert_eq!(io.want, Some(false));
    assert_eq!(out, PumpOut::default());
    // Second frame: no new VOICE_KEY, the queue is full, the UDP port is known.
    d.port.set(Some(7777));
    d.send.borrow_mut().push(Outgoing::Udp(vec![4]));
    d.events.borrow_mut().push(VoiceEvent::Mic(MicState::On));
    let out = p.frame(&d, &mut io, 2);
    assert_eq!(io.reliable.len(), 2);
    assert_eq!(io.udp, vec![(7777, vec![4])]);
    assert_eq!(io.pushed.len(), OUT_TARGET);
    assert_eq!(out.events, vec![VoiceEvent::Mic(MicState::On)]);
    p.stop(&mut io);
    assert!(io.cleared);
    p.frame(&d, &mut io, 3);
    assert_eq!(io.reliable.len(), 3, "a new session starts the key exchange again");
    p.stop(&mut io);
    d.no_key.set(true);
    p.frame(&d, &mut io, 4);
    assert_eq!(io.reliable.len(), 3, "no key, nothing to send");
}

#[test]
fn the_opus_voice_client_behind_the_driver_seam() {
    let d = opus_client();
    let key = d.start().unwrap();
    assert!(matches!(Message::decode(&key), Ok(Message::VoiceKey(_))));
    d.on_message(&Message::ListRequest);
    d.on_udp(&[1, 2, 3]);
    d.on_mic_frame(&[0.0; FRAME]);
    assert!(d.poll_send(1).is_empty(), "nothing before the server's answer");
    let mut out = vec![1.0; FRAME * 2];
    d.mix(&mut out);
    assert!(out.iter().all(|s| *s == 0.0), "nobody talks");
    assert!(!d.mic_wanted());
    d.as_host().set_mic(true).unwrap();
    assert!(d.mic_wanted());
    d.mic_state(MicState::On);
    assert_eq!(d.take_events(), vec![VoiceEvent::Mic(MicState::On)]);
    assert_eq!(d.transmitting(), None);
    assert_eq!(d.udp_port(), None);
    assert!(d.players().is_empty());
    assert!(d.speakers().is_empty());
    assert!(player_volumes(&*d).is_empty());
    assert_eq!(d.alias("p"), None);
    d.load_prefs("master 150\n");
    assert!(d.prefs().starts_with("master 150"));
    assert_eq!(d.as_host().master(), Ok(150));
}

#[test]
fn microphone_opt_in_and_states() {
    let d = FakeDriver::default();
    let mut io = FakeIo::default();
    let mut p = VoicePump::new(Consent::Unknown);
    d.wanted.set(true);
    // A mod turned the mic on: ask once, keep it closed meanwhile.
    assert!(p.frame(&d, &mut io, 1).ask_mic);
    assert_eq!(io.want, Some(false));
    assert!(!p.frame(&d, &mut io, 2).ask_mic);
    // Refused: the client hears `denied` once.
    p.consent(false);
    p.frame(&d, &mut io, 3);
    p.frame(&d, &mut io, 4);
    assert_eq!(*d.mic.borrow(), vec![MicState::Denied]);
    // Granted (from the settings screen): opens; the device follows a frame later.
    p.consent(true);
    p.frame(&d, &mut io, 5);
    assert_eq!(io.want, Some(true));
    io.device = Some(MicDevice::Open);
    io.chunks = vec![vec![100; FRAME], vec![1; 10]];
    let out = p.frame(&d, &mut io, 6);
    assert_eq!(out.transmitting, Some(Mode::Global));
    assert_eq!(d.frames.get(), 1, "only whole 20 ms frames are fed");
    assert_eq!(*d.mic.borrow(), vec![MicState::Denied, MicState::On]);
    // The device fails.
    io.device = Some(MicDevice::Failed);
    p.frame(&d, &mut io, 7);
    // The mod turns the mic off: closed, chunks still in the queue are dropped.
    d.wanted.set(false);
    io.device = Some(MicDevice::Closed);
    io.chunks = vec![vec![0; FRAME]];
    p.frame(&d, &mut io, 8);
    assert_eq!(io.want, Some(false));
    assert_eq!(d.frames.get(), 1);
    assert_eq!(*d.mic.borrow(), vec![MicState::Denied, MicState::On, MicState::Denied], "denied stands");
    // On again while the device is still closed: nothing new is reported until it opens.
    d.wanted.set(true);
    p.frame(&d, &mut io, 9);
    assert_eq!(d.mic.borrow().len(), 3);
    io.device = Some(MicDevice::Open);
    p.frame(&d, &mut io, 10);
    d.wanted.set(false);
    io.device = Some(MicDevice::Closed);
    p.frame(&d, &mut io, 11);
    assert_eq!(&d.mic.borrow()[3..], &[MicState::On, MicState::Off]);
}

#[test]
fn the_per_player_volume_list() {
    let d = FakeDriver::default();
    *d.names.borrow_mut() = vec![("id-a".into(), "Ann".into())];
    let v = player_volumes(&d);
    assert_eq!(
        v,
        vec![crate::form::PlayerVolume { id: "id-a".into(), alias: "Ann".into(), volume: 100, muted: false }]
    );
    // A speaker the client has no alias for shows by id.
    *d.names.borrow_mut() = vec![];
    d.extra_speakers.borrow_mut().push("id-b".into());
    assert_eq!(player_volumes(&d)[0].alias, "id-b");
}

#[test]
fn push_to_talk_skips_keys_a_mod_took() {
    let d = FakeDriver::default();
    let mut p = VoicePump::default();
    p.key_bound(false);
    p.talk(&d, VoiceMode::Channel, true);
    p.key_bound(true);
    p.talk(&d, VoiceMode::Proximity, true);
    p.talk(&d, VoiceMode::Proximity, false);
    assert_eq!(*d.talk.borrow(), vec![(Some(Mode::Channel), true), (Some(Mode::Proximity), false)]);
}

#[test]
fn fake_driver_rest() {
    let d = FakeDriver::default();
    d.on_message(&Message::ListRequest);
    assert_eq!(d.messages.borrow().len(), 1);
    d.load_prefs("x");
    assert_eq!(d.prefs(), "x");
    assert_eq!(d.as_host().icons(), Err(ApiError::Unsupported));
}

#[test]
fn voice_client_sends_map_one_to_one() {
    use bhmod_voice::client::Outgoing as O;
    assert_eq!(outgoing(O::Reliable(vec![1])), Outgoing::Reliable(vec![1]));
    assert_eq!(outgoing(O::Udp(vec![3])), Outgoing::Udp(vec![3]));
}
