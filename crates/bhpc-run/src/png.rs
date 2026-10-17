//! Minimal PNG writer (RGBA8, stored deflate blocks) for frame dumps. No dependencies, no compression.

use std::io::Write;

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode `rgba` (`width * height * 4` bytes, top row first) as a PNG.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), width as usize * height as usize * 4, "pixel buffer size");
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(b) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Flip a bottom-up GL read-back (`glReadPixels` order) to top-down.
pub fn flip_rows(width: u32, height: u32, bottom_up: &[u8]) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut out = Vec::with_capacity(bottom_up.len());
    for row in (0..height as usize).rev() {
        out.extend_from_slice(&bottom_up[row * stride..(row + 1) * stride]);
    }
    out
}

/// Write a top-down RGBA frame to `path` as a PNG.
pub fn write_file(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    std::fs::File::create(path)?.write_all(&encode(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn encodes_a_valid_structure() {
        let px = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 1, 2, 3, 4];
        let png = encode(2, 2, &px);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // 2 rows of (filter byte + 8 pixel bytes) in one final stored block.
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap();
        assert_eq!(png[idat + 4 + 2], 1, "final stored block flag");
        // Large images use several stored blocks.
        let big = encode(200, 200, &vec![7; 200 * 200 * 4]);
        assert!(big.len() > 160_000);
    }

    #[test]
    fn flips_and_writes() {
        let up = [1, 1, 1, 1, 2, 2, 2, 2];
        assert_eq!(flip_rows(1, 2, &up), [2, 2, 2, 2, 1, 1, 1, 1]);
        let p = std::env::temp_dir().join(format!("bhpc-png-{}.png", std::process::id()));
        write_file(&p, 1, 1, &[9, 9, 9, 255]).unwrap();
        assert_eq!(&std::fs::read(&p).unwrap()[..4], b"\x89PNG");
        std::fs::remove_file(&p).unwrap();
        assert!(write_file(std::path::Path::new("/nonexistent-dir/x.png"), 1, 1, &[0; 4]).is_err());
    }
}
