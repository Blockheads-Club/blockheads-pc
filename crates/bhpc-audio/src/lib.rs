//! Audio for the Blockheads PC port (PLAN 1.6).
//!
//! * [`mixer`]: a device-independent stereo mixer. The audio device (SDL2 `AudioCallback`, cpal, a
//!   WAV writer) only calls [`Mixer::fill`]; nothing here links an audio library.
//! * [`sles`]: OpenSL ES HLE. The guest's `libOpenSLES.so` calls (`slCreateEngine`, the object,
//!   engine, play, buffer-queue and volume vtables) are served from guest memory, and the PCM the
//!   guest enqueues is played through the mixer. Buffer-queue callbacks run on a pump thread.
//! * [`track`]: a host model of `android.media.AudioTrack` for the fake JVM, feeding the same mixer
//!   (the fallback path of libOpenAL when `libOpenSLES.so` is not offered).
//! * [`music`]: a `MediaPlayer` model decoding AAC/MP4, MP3, Ogg, FLAC and WAV into the mixer.
//! * [`pcm`]: sample formats.
//! * [`voice`]: voice chat playback (one 48 kHz stream per speaker, gain and constant-power pan).
//! * [`capture`]: microphone capture seam (20 ms mono chunks, opened only on demand).
//!
//! See `README.md` for the wiring into `bhpc-run`.

pub mod capture;
pub mod mixer;
pub mod music;
pub mod pcm;
pub mod sles;
pub mod track;
pub mod voice;

pub use mixer::{EventQueue, MixError, Mixer, MixerEvent, PlayState, Stats, StreamId};
pub use music::decode::Decoder;
pub use music::player::{MediaPlayer, PlayerEvent, PlayerState, PumpGuard};
pub use music::source::SliceSource;
pub use music::MusicError;
pub use pcm::{Frame, PcmFormat, SampleFormat};
pub use sles::{ExportKind, GuestCall, PumpHandle, Sles, SlesError};
pub use track::{AudioTrack, TrackError};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_music;
#[cfg(test)]
mod tests_sles;
#[cfg(test)]
mod tests_voice;
