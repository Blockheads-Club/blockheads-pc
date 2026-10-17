//! PCM formats and their conversion to the mixer's internal frame: stereo `i16`.

/// One stereo frame.
pub type Frame = [i16; 2];

/// Sample encodings the guest can queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Unsigned 8 bit (Android `ENCODING_PCM_8BIT`, OpenSL 8-bit PCM).
    U8,
    /// Signed 16 bit little endian.
    S16,
    /// IEEE float 32 bit little endian.
    F32,
}

impl SampleFormat {
    pub fn bytes(self) -> usize {
        match self {
            SampleFormat::U8 => 1,
            SampleFormat::S16 => 2,
            SampleFormat::F32 => 4,
        }
    }
}

/// Layout of a queued buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub rate_hz: u32,
    /// 1 or 2.
    pub channels: u8,
    pub sample: SampleFormat,
}

impl PcmFormat {
    /// Bytes per frame (all channels).
    pub fn frame_bytes(&self) -> usize {
        self.sample.bytes() * self.channels as usize
    }

    /// Convert interleaved `bytes` to stereo frames. A trailing partial frame is dropped; mono is
    /// duplicated to both sides.
    pub fn decode(&self, bytes: &[u8]) -> Vec<Frame> {
        let sb = self.sample.bytes();
        let sample = |chunk: &[u8]| -> i16 {
            match self.sample {
                SampleFormat::U8 => ((chunk[0] as i16) - 128) << 8,
                SampleFormat::S16 => i16::from_le_bytes([chunk[0], chunk[1]]),
                SampleFormat::F32 => {
                    let f = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    // NaN converts to 0 with `as`.
                    (f.clamp(-1.0, 1.0) * 32767.0) as i16
                }
            }
        };
        bytes
            .chunks_exact(self.frame_bytes())
            .map(|f| {
                let l = sample(&f[..sb]);
                if self.channels == 1 {
                    [l, l]
                } else {
                    [l, sample(&f[sb..])]
                }
            })
            .collect()
    }
}
