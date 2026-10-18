//! Rewriting the naming table of a TrueType font.
//!
//! The bundled stand-in fonts (see `fonts`) answer for the Android system fonts. Apportable's CoreGraphics keys its font
//! database by the names *inside* the font (PostScript name `Roboto-Regular`, ...), and maps the iOS names the game
//! asks for (`Helvetica`) onto those entries, so a stand-in must carry the names of the font it replaces.

/// Name ids that are rewritten (the other records of the original table are kept: copyright, license, ...).
const REPLACED: [u16; 8] = [1, 2, 3, 4, 6, 16, 17, 25];

/// The names a font answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub family: String,
    pub style: String,
    pub full: String,
    pub postscript: String,
}

impl Names {
    /// Names of the Android font file `file` (`Roboto-BoldItalic.ttf` -> family `Roboto`, style `Bold Italic`).
    pub fn for_file(file: &str) -> Names {
        let stem = file.rsplit_once('.').map_or(file, |s| s.0);
        let (fam, style) = stem.split_once('-').unwrap_or((stem, "Regular"));
        let mut family = String::new();
        for (i, ch) in fam.chars().enumerate() {
            if i > 0 && ch.is_ascii_uppercase() {
                family.push(' ');
            }
            family.push(ch);
        }
        let mut st = String::new();
        for (i, ch) in style.chars().enumerate() {
            if i > 0 && ch.is_ascii_uppercase() {
                st.push(' ');
            }
            st.push(ch);
        }
        let full = if st == "Regular" { family.clone() } else { format!("{family} {st}") };
        Names { family, style: st, full, postscript: stem.to_string() }
    }
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// One name record: platform, encoding, language, name id and the raw string bytes.
struct Rec {
    platform: u16,
    encoding: u16,
    language: u16,
    id: u16,
    bytes: Vec<u8>,
}

fn parse_name_table(t: &[u8]) -> Option<Vec<Rec>> {
    let (count, base) = (be16(t, 2)? as usize, be16(t, 4)? as usize);
    let mut out = Vec::new();
    for i in 0..count {
        let r = 6 + 12 * i;
        let (len, off) = (be16(t, r + 8)? as usize, be16(t, r + 10)? as usize);
        out.push(Rec {
            platform: be16(t, r)?,
            encoding: be16(t, r + 2)?,
            language: be16(t, r + 4)?,
            id: be16(t, r + 6)?,
            bytes: t.get(base + off..base + off + len)?.to_vec(),
        });
    }
    Some(out)
}

fn utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn build_name_table(recs: &mut [Rec]) -> Vec<u8> {
    recs.sort_by_key(|r| (r.platform, r.encoding, r.language, r.id));
    let mut t = Vec::new();
    t.extend_from_slice(&0u16.to_be_bytes());
    t.extend_from_slice(&(recs.len() as u16).to_be_bytes());
    t.extend_from_slice(&((6 + 12 * recs.len()) as u16).to_be_bytes());
    let mut strings = Vec::new();
    for r in recs.iter() {
        for v in [r.platform, r.encoding, r.language, r.id, r.bytes.len() as u16, strings.len() as u16] {
            t.extend_from_slice(&v.to_be_bytes());
        }
        strings.extend_from_slice(&r.bytes);
    }
    t.extend_from_slice(&strings);
    t
}

/// A copy of the TrueType font `data` whose names are `names`; `None` if `data` is not a parsable single TrueType font.
///
/// The new `name` table is appended to the file and the table directory is pointed at it (table checksums are not
/// updated; FreeType does not verify them).
pub fn rename(data: &[u8], names: &Names) -> Option<Vec<u8>> {
    let tables = be16(data, 4)? as usize;
    let rec = (0..tables).map(|i| 12 + 16 * i).find(|r| data.get(*r..*r + 4) == Some(b"name"))?;
    let (off, len) = (be32(data, rec + 8)? as usize, be32(data, rec + 12)? as usize);
    let mut recs = parse_name_table(data.get(off..off + len)?)?;
    recs.retain(|r| !REPLACED.contains(&r.id));
    let unique = format!("{};{}", names.postscript, "bhpc");
    for (id, s) in [(1, &names.family), (2, &names.style), (3, &unique), (4, &names.full), (6, &names.postscript)] {
        recs.push(Rec { platform: 3, encoding: 1, language: 0x409, id, bytes: utf16be(s) });
        recs.push(Rec { platform: 1, encoding: 0, language: 0, id, bytes: s.as_bytes().to_vec() });
    }
    let table = build_name_table(&mut recs);
    let mut out = data.to_vec();
    while out.len() % 4 != 0 {
        out.push(0);
    }
    let new_off = out.len() as u32;
    out.extend_from_slice(&table);
    out[rec + 8..rec + 12].copy_from_slice(&new_off.to_be_bytes());
    out[rec + 12..rec + 16].copy_from_slice(&(table.len() as u32).to_be_bytes());
    Some(out)
}

/// The strings of the name table of `data` as `(name id, text)` for the Windows records (tests and diagnostics).
pub fn read_names(data: &[u8]) -> Option<Vec<(u16, String)>> {
    let tables = be16(data, 4)? as usize;
    let rec = (0..tables).map(|i| 12 + 16 * i).find(|r| data.get(*r..*r + 4) == Some(b"name"))?;
    let (off, len) = (be32(data, rec + 8)? as usize, be32(data, rec + 12)? as usize);
    let recs = parse_name_table(data.get(off..off + len)?)?;
    Some(
        recs.into_iter()
            .filter(|r| r.platform == 3)
            .map(|r| {
                let units: Vec<u16> =
                    r.bytes.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                (r.id, String::from_utf16_lossy(&units))
            })
            .collect(),
    )
}
