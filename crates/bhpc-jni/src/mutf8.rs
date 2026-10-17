//! Java strings are UTF-16; JNI hands them to native code as "modified UTF-8".

/// Encode UTF-16 units as modified UTF-8 (NUL as `C0 80`, surrogates as 3-byte sequences).
pub fn encode(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len());
    for &u in units {
        match u {
            1..=0x7f => out.push(u as u8),
            0 | 0x80..=0x7ff => {
                out.push(0xc0 | (u >> 6) as u8);
                out.push(0x80 | (u & 0x3f) as u8);
            }
            _ => {
                out.push(0xe0 | (u >> 12) as u8);
                out.push(0x80 | ((u >> 6) & 0x3f) as u8);
                out.push(0x80 | (u & 0x3f) as u8);
            }
        }
    }
    out
}

/// Decode modified UTF-8 (also accepts standard 4-byte UTF-8) into UTF-16 units. Malformed bytes
/// become U+FFFD.
pub fn decode(bytes: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let (len, init) = match b {
            0..=0x7f => (1, b as u32),
            0xc0..=0xdf => (2, (b & 0x1f) as u32),
            0xe0..=0xef => (3, (b & 0x0f) as u32),
            0xf0..=0xf7 => (4, (b & 0x07) as u32),
            _ => (0, 0),
        };
        let ok = len > 0 && i + len <= bytes.len() && bytes[i + 1..i + len].iter().all(|c| c & 0xc0 == 0x80);
        if !ok {
            out.push(0xfffd);
            i += 1;
            continue;
        }
        let cp = bytes[i + 1..i + len].iter().fold(init, |a, c| a << 6 | (c & 0x3f) as u32);
        i += len;
        match char::from_u32(cp) {
            Some(c) => {
                let mut buf = [0u16; 2];
                out.extend_from_slice(c.encode_utf16(&mut buf));
            }
            // A lone surrogate encoded as 3 bytes is legal in modified UTF-8.
            None if cp <= 0xffff => out.push(cp as u16),
            None => out.push(0xfffd),
        }
    }
    out
}

/// UTF-16 units of a Rust string.
pub fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Lossy conversion of UTF-16 units to a Rust string.
pub fn to_string(u: &[u16]) -> String {
    String::from_utf16_lossy(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for s in ["", "hello", "héllo", "日本語", "a\u{0}b", "😀 emoji"] {
            let u = units(s);
            let enc = encode(&u);
            assert!(!enc.contains(&0), "NUL must not appear");
            assert_eq!(to_string(&decode(&enc)), s);
        }
        assert_eq!(encode(&[0]), [0xc0, 0x80]);
        // Surrogate pair is encoded as two 3-byte sequences.
        assert_eq!(encode(&units("😀")).len(), 6);
    }

    #[test]
    fn decode_edge_cases() {
        assert_eq!(decode(&[0xf0, 0x9f, 0x98, 0x80]), units("😀"));
        assert_eq!(decode(&[0xff]), [0xfffd]);
        assert_eq!(decode(&[0xe0, 0x80]), [0xfffd, 0xfffd]);
        assert_eq!(decode(&[0xc3, 0x28]), [0xfffd, 0x28]);
        assert_eq!(decode(&[0xed, 0xa0, 0x80]), [0xd800]);
        assert_eq!(decode(&[0xf7, 0xbf, 0xbf, 0xbf]), [0xfffd]);
    }
}
