//! Byte sources for the decoder.

use std::io::{self, Read, Seek, SeekFrom};
use symphonia::core::io::MediaSource;

/// A window `[offset, offset + length)` of a seekable reader, presented as a stream of its own.
/// This is `MediaPlayer.setDataSource(FileDescriptor, offset, length)`: the game's music lives
/// inside the APK, so the file is the whole archive and the window is one stored entry.
pub struct SliceSource<R> {
    inner: R,
    offset: u64,
    length: u64,
    /// Position inside the window.
    pos: u64,
}

impl<R: Read + Seek> SliceSource<R> {
    /// `length` is clamped to what the reader actually holds past `offset`.
    pub fn new(mut inner: R, offset: u64, length: Option<u64>) -> io::Result<Self> {
        let total = inner.seek(SeekFrom::End(0))?;
        let avail = total.saturating_sub(offset);
        let length = length.map_or(avail, |l| l.min(avail));
        inner.seek(SeekFrom::Start(offset))?;
        Ok(SliceSource { inner, offset, length, pos: 0 })
    }

    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
}

impl<R: Read + Seek> Read for SliceSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = (self.length - self.pos) as usize;
        let want = buf.len().min(left);
        let n = self.inner.read(&mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for SliceSource<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.length.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        let Some(target) = target else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before the start of the window"));
        };
        // Seeking past the end is allowed, as for files; reads then return 0.
        self.pos = target.min(self.length);
        self.inner.seek(SeekFrom::Start(self.offset + self.pos))?;
        Ok(target)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for SliceSource<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.length)
    }
}
