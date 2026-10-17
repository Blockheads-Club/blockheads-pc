//! Container probing and decoding to the mixer's stereo `i16` frames.

use super::MusicError;
use crate::pcm::Frame;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

/// An opened audio stream.
pub struct Decoder {
    format: Box<dyn FormatReader>,
    dec: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    rate: u32,
    duration_ms: Option<u32>,
    scratch: Vec<i16>,
    /// Frames at the start of the next packets to throw away after an accurate seek.
    skip: usize,
}

impl Decoder {
    /// Probe `src` (`ext` is an optional file-extension hint) and open its default audio track.
    pub fn open(src: Box<dyn MediaSource>, ext: Option<&str>) -> Result<Decoder, MusicError> {
        let mss = MediaSourceStream::new(src, Default::default());
        let mut hint = Hint::new();
        if let Some(e) = ext {
            hint.with_extension(e);
        }
        let format =
            symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())?;
        let track = format.default_track(TrackType::Audio);
        let audio = track.and_then(|t| Some((t, t.codec_params.as_ref()?.audio()?)));
        let usable = audio.and_then(|(t, p)| Some((t, p, p.sample_rate.filter(|&r| r > 0)?)));
        let (track, params, rate) = usable.ok_or(MusicError::Decode("no usable audio track".into()))?;
        let dec = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;
        let duration_ms = track.num_frames.map(|n| (n * 1000 / rate as u64).min(u32::MAX as u64) as u32);
        Ok(Decoder {
            track_id: track.id,
            time_base: track.time_base,
            rate,
            duration_ms,
            scratch: Vec::new(),
            skip: 0,
            format,
            dec,
        })
    }

    /// Native sample rate in Hz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Length in milliseconds, when the container says.
    pub fn duration_ms(&self) -> Option<u32> {
        self.duration_ms
    }

    /// The next decoded packet as stereo frames (mono is duplicated, extra channels dropped), or
    /// `None` at the end of the stream. Packets that fail to decode are skipped.
    pub fn next_frames(&mut self) -> Result<Option<Vec<Frame>>, MusicError> {
        loop {
            let Some(packet) = self.format.next_packet()? else { return Ok(None) };
            // Packets of other tracks and packets that fail to decode are skipped.
            let decoded = (packet.track_id == self.track_id).then(|| self.dec.decode(&packet));
            let Some(Ok(buf)) = decoded else { continue };
            let channels = buf.spec().channels().count().max(1);
            buf.copy_to_vec_interleaved(&mut self.scratch);
            let drop_now = self.skip.min(self.scratch.len() / channels);
            self.skip -= drop_now;
            let frames: Vec<Frame> = self
                .scratch
                .chunks_exact(channels)
                .skip(drop_now)
                .map(|f| if channels == 1 { [f[0], f[0]] } else { [f[0], f[1]] })
                .collect();
            if frames.is_empty() {
                continue;
            }
            return Ok(Some(frames));
        }
    }

    /// Seek to `ms` (sample accurate where the container allows it).
    pub fn seek_ms(&mut self, ms: u32) -> Result<(), MusicError> {
        let time = Time::try_from_nanos_i128(ms as i128 * 1_000_000).expect("u32 ms fits in Time");
        let to = SeekTo::Time { time, track_id: Some(self.track_id) };
        let sought = self.format.seek(SeekMode::Accurate, to)?;
        self.dec.reset();
        let ms_of = |ts| self.time_base.and_then(|tb| tb.calc_time(ts)).map_or(0, |t| t.as_nanos().max(0));
        // The container lands on a packet at or before the target; decode up to it and drop the rest.
        let gap_ns = ms_of(sought.required_ts) - ms_of(sought.actual_ts);
        self.skip = (gap_ns.max(0) * self.rate as i128 / 1_000_000_000) as usize;
        Ok(())
    }
}
