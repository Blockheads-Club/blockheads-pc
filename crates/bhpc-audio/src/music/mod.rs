//! Music playback: a host-side model of `android.media.MediaPlayer` that decodes compressed audio
//! (AAC in MP4 as shipped by Blockheads, plus MP3, Ogg Vorbis, FLAC and WAV) with pure-Rust
//! decoders and streams it into a [`Mixer`](crate::Mixer) stream.
//!
//! * [`source`]: byte windows (`setDataSource(fd, offset, length)`) over files and memory.
//! * [`decode`]: container probing, decoding to stereo `i16` frames, seeking.
//! * [`player`]: the `MediaPlayer` state machine, listener events and the pump that keeps the mixer
//!   stream fed.

pub mod decode;
pub mod player;
pub mod source;

use std::fmt;

/// Why a music operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MusicError {
    /// The call is not valid in the player's current state (Java `IllegalStateException`).
    IllegalState(&'static str),
    /// The data source could not be read (Java `IOException`).
    Io(String),
    /// The container or codec is not supported, or the stream is corrupt.
    Decode(String),
}

impl fmt::Display for MusicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MusicError::IllegalState(s) => write!(f, "illegal state: {s}"),
            MusicError::Io(s) => write!(f, "i/o error: {s}"),
            MusicError::Decode(s) => write!(f, "decode error: {s}"),
        }
    }
}

impl std::error::Error for MusicError {}

impl From<symphonia::core::errors::Error> for MusicError {
    fn from(e: symphonia::core::errors::Error) -> Self {
        match e {
            symphonia::core::errors::Error::IoError(e) => MusicError::Io(e.to_string()),
            e => MusicError::Decode(e.to_string()),
        }
    }
}
