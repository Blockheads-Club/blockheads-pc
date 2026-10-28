//! Voice chat of the client loader (milestone V5). The voice core itself (key exchange, crypto, Opus, VAD, the
//! push-to-talk state machine, jitter buffers, the mix) is mod-sdk's `bhmod-voice` `VoiceClient`; this module is the
//! thin layer between it and the PC's devices:
//!
//! - [`features`]: what this client tells the server in `HELLO.features`.
//! - [`VoiceDriver`]: the calls the loader makes on the voice client (the `VoiceClient` driver of mod-sdk, or a
//!   fake in tests); it is also the [`VoiceHost`] the mods' `voice.*` calls reach.
//! - [`VoiceIo`]: the PC's devices and sends (microphone, playback, unsequenced ENet, the voice UDP socket).
//! - [`VoicePump`]: once per frame on the game's main thread, moves microphone frames in, packets out and the mix to
//!   the speakers, asks the player before the microphone opens the first time, and reports the microphone state.
//! - [`TalkKeys`]: push-to-talk and mod key-binding presses from the window's input thread.
//! - [`VoicePrefs`]: what the loader keeps on the device (microphone consent, the voice client's volumes and mutes).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use bhmod_core::voice::{MicState, Mode, VoiceEvent};
use bhmod_core::VoiceHost;
use bhmod_proto::Message;
use bhpc_input::VoiceMode;

/// `HELLO.features`: the client takes `SETTINGS_SCHEMA` and shows the settings screen.
pub const FEATURE_SETTINGS: &str = "settings";
/// `HELLO.features`: the client runs voice chat.
pub const FEATURE_VOICE: &str = "voice";
/// `HELLO.features`: the client sends voice frames as unsequenced ENet packets on the game connection.
pub const FEATURE_VOICE_UNSEQUENCED: &str = "voice-unsequenced";

/// The `HELLO.features` of this client: `settings` always, `voice` when a voice client can run (`voice`), and
/// `voice-unsequenced` when it also can send unsequenced packets in-band (`unsequenced`; without it the server uses
/// its UDP port, if it has one).
pub fn features(voice: bool, unsequenced: bool) -> Vec<String> {
    let mut f = vec![FEATURE_SETTINGS.to_string()];
    if voice {
        f.push(FEATURE_VOICE.to_string());
        if unsequenced {
            f.push(FEATURE_VOICE_UNSEQUENCED.to_string());
        }
    }
    f
}

/// The voice mode of a loader push-to-talk key.
pub fn mode_of(m: VoiceMode) -> Mode {
    match m {
        VoiceMode::Proximity => Mode::Proximity,
        VoiceMode::Channel => Mode::Channel,
        VoiceMode::Global => Mode::Global,
        VoiceMode::Broadcast => Mode::Broadcast,
    }
}

/// Samples per channel of one 20 ms frame at 48 kHz.
pub const FRAME: usize = 960;
/// Mixed audio the speakers should have queued (in frames of [`FRAME`] stereo samples): 60 ms.
pub const OUT_TARGET: usize = 3;

/// A packet the voice client wants sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// A 0xBC packet on the reliable path (`VOICE_KEY`, `VOICE_CTRL`).
    Reliable(Vec<u8>),
    /// A 0xBC packet (`VOICE_FRAME`) as an unsequenced ENet packet on the game connection.
    Unsequenced(Vec<u8>),
    /// A datagram for the server's voice UDP port.
    Udp(Vec<u8>),
}

/// The loader's calls on the voice client (mod-sdk `bhmod_voice::client::VoiceClient`, shared). All take `&self`:
/// the same object answers the mods' `voice.*` calls through [`VoiceDriver::as_host`] while the loader drives it.
pub trait VoiceDriver {
    /// The client as the mods' [`VoiceHost`].
    fn as_host(&self) -> &dyn VoiceHost;
    /// Starts the key exchange: the `VOICE_KEY` packet to send (reliable); `None` when no key could be made.
    fn start(&self) -> Option<Vec<u8>>;
    /// `VOICE_KEY` (the server's answer), `VOICE_STATE` or `VOICE_FRAME` from the server.
    fn on_message(&self, msg: &Message);
    /// A datagram from the server's voice UDP port.
    fn on_udp(&self, datagram: &[u8]);
    /// One 20 ms microphone frame (48 kHz mono, [`FRAME`] samples, -1..1).
    fn on_mic_frame(&self, pcm: &[f32]);
    /// What to send now (`now_ms`: Unix time in ms).
    fn poll_send(&self, now_ms: u64) -> Vec<Outgoing>;
    /// Mixes what the player hears into `out` (interleaved stereo 48 kHz, zeroed by the caller).
    fn mix(&self, out: &mut [f32]);
    /// The client wants the microphone open (a mod turned it on and someone may talk).
    fn mic_wanted(&self) -> bool;
    /// The microphone opened, closed, or the player or the OS refused it.
    fn mic_state(&self, state: MicState);
    /// The mode sent right now, for the host-drawn mic indicator (`None`: not transmitting).
    fn transmitting(&self) -> Option<Mode>;
    /// Talk start/stop, state changes, the microphone: for [`bhmod_core::ModHost::voice_event`].
    fn take_events(&self) -> Vec<VoiceEvent>;
    /// The server's voice UDP port when the client switched to it.
    fn udp_port(&self) -> Option<u16>;
    /// Every player the server listed in `VOICE_STATE`: `(player id, alias)`.
    fn players(&self) -> Vec<(String, String)>;
    /// A listed player's alias.
    fn alias(&self, player: &str) -> Option<String>;
    /// Master volume, per-player volumes and local mutes as text (kept on the device).
    fn prefs(&self) -> String;
    /// Restores [`VoiceDriver::prefs`].
    fn load_prefs(&self, text: &str);
}

/// The microphone device as the platform sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicDevice {
    Closed,
    Open,
    /// Opening failed (no device, the OS refused).
    Failed,
}

/// The PC's side of voice chat (implemented over `bhpc_audio` and the game connection in `bhpc-run`).
pub trait VoiceIo {
    /// Ask for the microphone open or closed (it opens on the window thread later).
    fn set_mic(&mut self, on: bool);
    /// The device now.
    fn mic(&self) -> MicDevice;
    /// The next recorded 20 ms chunk (48 kHz mono i16).
    fn mic_chunk(&mut self) -> Option<Vec<i16>>;
    /// Send a 0xBC packet reliably.
    fn send_reliable(&mut self, packet: Vec<u8>);
    /// Send a 0xBC packet unsequenced; false when it could not be queued.
    fn send_unsequenced(&mut self, packet: &[u8]) -> bool;
    /// Send a datagram to the server's voice UDP `port`.
    fn send_udp(&mut self, port: u16, datagram: &[u8]);
    /// Datagrams received on the voice UDP socket since the last call.
    fn recv_udp(&mut self) -> Vec<Vec<u8>>;
    /// Stereo frames waiting to be played.
    fn out_queued(&self) -> usize;
    /// Queue interleaved stereo 48 kHz samples for the speakers.
    fn out_push(&mut self, stereo: &[f32]);
    /// Drop everything queued and stop (the session ended).
    fn out_clear(&mut self);
}

/// The player's answer to "use the microphone for voice chat?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Consent {
    /// Not asked yet.
    #[default]
    Unknown,
    Granted,
    Refused,
}

/// What the loader keeps on the device for voice chat (`<data>/voice.toml`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VoicePrefs {
    /// Microphone opt-in ("never" is remembered as refused; the settings screen can change it).
    pub mic: Consent,
    /// The voice client's own text ([`VoiceDriver::prefs`]): master volume, per-player volumes, local mutes.
    pub client: String,
    /// The voice master volume the launcher last passed (its Settings page); a new value replaces the client's.
    pub launcher_master: Option<u16>,
}

impl VoicePrefs {
    /// The file in the launcher's data folder.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("voice.toml")
    }

    /// Reads the file; a missing or broken file gives the defaults.
    pub fn load(path: &Path) -> VoicePrefs {
        std::fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    /// Writes the file (and its folder).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, toml::to_string(self).expect("voice prefs serialise"))
    }
}

/// What a frame of the pump asks the glue to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PumpOut {
    /// Show the microphone opt-in dialog now (answer with [`VoicePump::consent`]).
    pub ask_mic: bool,
    /// For the mods ([`bhmod_core::ModHost::voice_event`]).
    pub events: Vec<VoiceEvent>,
    /// The mic indicator shows this mode.
    pub transmitting: Option<Mode>,
}

/// Drives a [`VoiceDriver`] from the PC's [`VoiceIo`], once per frame.
#[derive(Debug, Default)]
pub struct VoicePump {
    started: bool,
    consent: Consent,
    /// The dialog is up.
    asking: bool,
    /// The last microphone state told to the driver.
    told: Option<MicState>,
    /// A mod wanted the microphone at the last frame.
    was_wanted: bool,
    /// The key of the last key event belongs to a mod binding: the push-to-talk event that follows it is not for the
    /// loader's own push-to-talk.
    key_bound: bool,
}

impl VoicePump {
    /// A pump with the player's stored microphone answer.
    pub fn new(consent: Consent) -> VoicePump {
        VoicePump { consent, ..VoicePump::default() }
    }

    /// The player's stored or given microphone answer.
    pub fn consent_state(&self) -> Consent {
        self.consent
    }

    /// The player answered the opt-in dialog (or changed it in the settings screen).
    pub fn consent(&mut self, granted: bool) {
        self.asking = false;
        self.consent = if granted { Consent::Granted } else { Consent::Refused };
        self.told = None;
    }

    /// A key event from [`TalkKeys`]: `bound` says whether a mod's binding took the key (then the loader's own
    /// push-to-talk for the same key is skipped).
    pub fn key_bound(&mut self, bound: bool) {
        self.key_bound = bound;
    }

    /// One push-to-talk key of the loader went down or up.
    pub fn talk(&mut self, d: &dyn VoiceDriver, mode: VoiceMode, pressed: bool) {
        if pressed && std::mem::take(&mut self.key_bound) {
            return;
        }
        let _ = d.as_host().talk(Some(mode_of(mode)), pressed);
    }

    fn tell(&mut self, d: &dyn VoiceDriver, s: MicState) {
        if self.told != Some(s) {
            self.told = Some(s);
            d.mic_state(s);
        }
    }

    /// One frame: start the key exchange, open or close the microphone, feed it, send, receive and play.
    pub fn frame(&mut self, d: &dyn VoiceDriver, io: &mut dyn VoiceIo, now_ms: u64) -> PumpOut {
        let mut out = PumpOut::default();
        if !self.started {
            self.started = true;
            if let Some(p) = d.start() {
                io.send_reliable(p);
            }
        }
        let wanted = d.mic_wanted();
        if wanted && !self.was_wanted {
            // A new request gets a new answer.
            self.told = None;
        }
        self.was_wanted = wanted;
        match (wanted, self.consent) {
            (true, Consent::Granted) => io.set_mic(true),
            (true, Consent::Unknown) => {
                io.set_mic(false);
                out.ask_mic = !std::mem::replace(&mut self.asking, true);
            }
            (true, Consent::Refused) => {
                io.set_mic(false);
                self.tell(d, MicState::Denied);
            }
            (false, _) => io.set_mic(false),
        }
        match io.mic() {
            MicDevice::Open if wanted => self.tell(d, MicState::On),
            MicDevice::Failed if wanted => self.tell(d, MicState::Denied),
            // `denied` stands until the next request (the client drops its wish on it).
            _ if !wanted && self.told != Some(MicState::Denied)
                || wanted && self.consent == Consent::Granted && self.told == Some(MicState::On) =>
            {
                self.tell(d, MicState::Off)
            }
            _ => {}
        }
        let mut pcm = vec![0f32; FRAME];
        while let Some(chunk) = io.mic_chunk() {
            if !wanted || chunk.len() != FRAME {
                continue;
            }
            for (o, s) in pcm.iter_mut().zip(&chunk) {
                *o = f32::from(*s) / 32768.0;
            }
            d.on_mic_frame(&pcm);
        }
        for datagram in io.recv_udp() {
            d.on_udp(&datagram);
        }
        for o in d.poll_send(now_ms) {
            match o {
                Outgoing::Reliable(p) => io.send_reliable(p),
                Outgoing::Unsequenced(p) => {
                    // Dropped when it cannot go (voice is useless late or reliable).
                    io.send_unsequenced(&p);
                }
                Outgoing::Udp(p) => {
                    if let Some(port) = d.udp_port() {
                        io.send_udp(port, &p);
                    }
                }
            }
        }
        let mut buf = vec![0f32; FRAME * 2];
        let mut n = 0;
        while io.out_queued() < OUT_TARGET * FRAME && n < OUT_TARGET {
            buf.fill(0.0);
            d.mix(&mut buf);
            io.out_push(&buf);
            n += 1;
        }
        out.events = d.take_events();
        out.transmitting = d.transmitting();
        out
    }

    /// The session ended: microphone closed, playback dropped.
    pub fn stop(&mut self, io: &mut dyn VoiceIo) {
        io.set_mic(false);
        io.out_clear();
        self.started = false;
        self.asking = false;
        self.told = None;
    }
}

/// A key press for the voice chat or a mod binding, from the input thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyIn {
    /// A loader push-to-talk key.
    Talk { mode: VoiceMode, pressed: bool },
    /// Any key with an `input` interface name (for the mods' bindings), sent before the push-to-talk event of the
    /// same key.
    Key { name: &'static str, pressed: bool },
}

/// Push-to-talk key events from the window's input thread to the mod host on the game's main thread, plus which
/// modes are held right now.
#[derive(Default)]
pub struct TalkKeys {
    inner: Mutex<(Vec<KeyIn>, Vec<VoiceMode>)>,
}

impl TalkKeys {
    /// The input thread saw a push-to-talk key go down or up (repeats are already filtered by the mapper).
    pub fn post(&self, mode: VoiceMode, pressed: bool) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.0.push(KeyIn::Talk { mode, pressed });
        g.1.retain(|m| *m != mode);
        if pressed {
            g.1.push(mode);
        }
    }

    /// The input thread saw a named key go down or up (no repeats, not while a text field is open).
    pub fn post_key(&self, name: &'static str, pressed: bool) {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).0.push(KeyIn::Key { name, pressed });
    }

    /// The events since the last call, in order (main thread, once per frame).
    pub fn take(&self) -> Vec<KeyIn> {
        std::mem::take(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner).0)
    }

    /// Modes whose key is held, in press order.
    pub fn held(&self) -> Vec<VoiceMode> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).1.clone()
    }
}

/// A send of the voice client as the loader's [`Outgoing`].
pub(crate) fn outgoing(o: bhmod_voice::client::Outgoing) -> Outgoing {
    use bhmod_voice::client::Outgoing as O;
    match o {
        O::Reliable(p) => Outgoing::Reliable(p),
        O::Unsequenced(p) => Outgoing::Unsequenced(p),
        O::Udp(p) => Outgoing::Udp(p),
    }
}

/// The voice client of mod-sdk (`bhmod-voice`, libopus through `bhmod-opus-sys`) as the loader drives it.
impl VoiceDriver for bhmod_voice::client::SharedVoiceClient {
    fn as_host(&self) -> &dyn VoiceHost {
        self
    }
    fn start(&self) -> Option<Vec<u8>> {
        self.lock().start()
    }
    fn on_message(&self, msg: &Message) {
        self.lock().on_message(msg);
    }
    fn on_udp(&self, datagram: &[u8]) {
        self.lock().on_udp(datagram);
    }
    fn on_mic_frame(&self, pcm: &[f32]) {
        self.lock().on_mic_frame(pcm);
    }
    fn poll_send(&self, now_ms: u64) -> Vec<Outgoing> {
        let out = self.lock().poll_send(now_ms);
        out.into_iter().map(outgoing).collect()
    }
    fn mix(&self, out: &mut [f32]) {
        self.lock().mix(out);
    }
    fn mic_wanted(&self) -> bool {
        self.lock().mic_wanted()
    }
    fn mic_state(&self, state: MicState) {
        self.lock().mic_state(state);
    }
    fn transmitting(&self) -> Option<Mode> {
        self.lock().transmitting()
    }
    fn take_events(&self) -> Vec<VoiceEvent> {
        self.lock().take_events()
    }
    fn udp_port(&self) -> Option<u16> {
        self.lock().udp_port()
    }
    fn players(&self) -> Vec<(String, String)> {
        self.lock().players()
    }
    fn alias(&self, player: &str) -> Option<String> {
        self.lock().alias(player)
    }
    fn prefs(&self) -> String {
        self.lock().prefs()
    }
    fn load_prefs(&self, text: &str) {
        self.lock().load_prefs(text);
    }
}

/// A new voice client with Opus (`unsequenced`: in-band sends work; else it needs the server's UDP port).
pub fn opus_client(unsequenced: bool) -> std::rc::Rc<dyn VoiceDriver> {
    let codec = std::sync::Arc::new(bhmod_opus_sys::OpusCodec);
    std::rc::Rc::new(bhmod_voice::client::SharedVoiceClient::new(bhmod_voice::client::VoiceClient::new(
        codec,
        unsequenced,
    )))
}
