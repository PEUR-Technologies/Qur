//! Text encodings, Notepad++ style: UTF-8 (with or without a BOM), UTF-16
//! LE/BE, and Windows-1252 as the fallback that reads any bytes. Hand-written
//! (no encoding crate in the build): UTF-16 comes from `char::decode_utf16`,
//! Windows-1252 is a 32-entry table.
//!
//! Detection: a BOM wins; then UTF-16 without a BOM when the first 8 KB look
//! like ASCII text with every other byte zero; then UTF-8 if the whole file
//! is valid; else Windows-1252. A file is *binary* when its first 8 KB hold a
//! NUL byte and it isn't UTF-16.

/// How much of a file detection and the binary check look at.
pub const SNIFF_LEN: usize = 8 << 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
    /// Also reads Latin-1: the five bytes 1252 leaves undefined map to the
    /// C1 controls (U+0081…), so every byte decodes and saves back the same.
    Windows1252,
}

/// The choices the encoding pickers offer: (encoding, write a BOM).
pub const CHOICES: [(Encoding, bool); 5] = [
    (Encoding::Utf8, false),
    (Encoding::Utf8, true),
    (Encoding::Utf16Le, true),
    (Encoding::Utf16Be, true),
    (Encoding::Windows1252, false),
];

/// "UTF-8", "UTF-8 BOM", "UTF-16 LE", "UTF-16 LE (no BOM)", "Windows-1252".
pub fn label(enc: Encoding, bom: bool) -> &'static str {
    match (enc, bom) {
        (Encoding::Utf8, false) => "UTF-8",
        (Encoding::Utf8, true) => "UTF-8 BOM",
        (Encoding::Utf16Le, true) => "UTF-16 LE",
        (Encoding::Utf16Le, false) => "UTF-16 LE (no BOM)",
        (Encoding::Utf16Be, true) => "UTF-16 BE",
        (Encoding::Utf16Be, false) => "UTF-16 BE (no BOM)",
        (Encoding::Windows1252, _) => "Windows-1252",
    }
}

const BOM_UTF8: &[u8] = &[0xEF, 0xBB, 0xBF];
const BOM_LE: &[u8] = &[0xFF, 0xFE];
const BOM_BE: &[u8] = &[0xFE, 0xFF];

impl Encoding {
    fn bom_bytes(self) -> &'static [u8] {
        match self {
            Encoding::Utf8 => BOM_UTF8,
            Encoding::Utf16Le => BOM_LE,
            Encoding::Utf16Be => BOM_BE,
            Encoding::Windows1252 => &[],
        }
    }

    pub fn is_utf16(self) -> bool {
        matches!(self, Encoding::Utf16Le | Encoding::Utf16Be)
    }
}

/// The encoding and BOM detected for a whole file's bytes.
pub fn detect(bytes: &[u8]) -> (Encoding, bool) {
    detect_impl(bytes, false)
}

/// `detect` for the first bytes of a file only (a large file): a UTF-8
/// sequence cut off at the end still counts as UTF-8.
pub fn guess(prefix: &[u8]) -> (Encoding, bool) {
    detect_impl(prefix, true)
}

fn detect_impl(bytes: &[u8], prefix: bool) -> (Encoding, bool) {
    if bytes.starts_with(BOM_UTF8) {
        return (Encoding::Utf8, true);
    }
    if bytes.starts_with(BOM_LE) {
        return (Encoding::Utf16Le, true);
    }
    if bytes.starts_with(BOM_BE) {
        return (Encoding::Utf16Be, true);
    }
    if let Some(enc) = utf16_without_bom(&bytes[..bytes.len().min(SNIFF_LEN)]) {
        return (enc, false);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (Encoding::Utf8, false);
    }
    // A prefix cut in the middle of a UTF-8 sequence is still UTF-8.
    if let Err(e) = std::str::from_utf8(bytes)
        && prefix
        && e.error_len().is_none()
        && e.valid_up_to() + 4 > bytes.len()
    {
        return (Encoding::Utf8, false);
    }
    (Encoding::Windows1252, false)
}

/// ASCII-heavy UTF-16 without a BOM: in most two-byte pairs one byte is
/// zero, always the same one.
fn utf16_without_bom(sample: &[u8]) -> Option<Encoding> {
    let pairs = sample.len() / 2;
    if pairs < 2 || !sample.contains(&0) {
        return None;
    }
    let (mut le, mut be) = (0, 0);
    for p in sample.chunks_exact(2) {
        match (p[0] == 0, p[1] == 0) {
            (false, true) => le += 1,
            (true, false) => be += 1,
            _ => {}
        }
    }
    // 70% of pairs one way and almost none the other.
    if le * 10 >= pairs * 7 && be * 20 <= pairs {
        Some(Encoding::Utf16Le)
    } else if be * 10 >= pairs * 7 && le * 20 <= pairs {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

/// A NUL byte in the first 8 KB of something that isn't UTF-16.
pub fn is_binary(prefix: &[u8]) -> bool {
    let sample = &prefix[..prefix.len().min(SNIFF_LEN)];
    sample.contains(&0) && !detect(sample).0.is_utf16()
}

/// Read the first 8 KB of a file and say whether it's binary.
pub fn file_is_binary(path: &std::path::Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut buf = vec![0u8; SNIFF_LEN];
    let mut f = std::fs::File::open(path)?;
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(is_binary(&buf[..n]))
}

/// Decode `bytes` as `enc`, dropping a leading BOM of that encoding. The
/// flag is false when something didn't decode (shown as U+FFFD), so saving
/// back wouldn't give the same bytes.
pub fn decode(bytes: &[u8], enc: Encoding) -> (String, bool) {
    let bytes = bytes.strip_prefix(enc.bom_bytes()).unwrap_or(bytes);
    match enc {
        Encoding::Utf8 => match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_string(), true),
            Err(_) => (String::from_utf8_lossy(bytes).into_owned(), false),
        },
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let le = enc == Encoding::Utf16Le;
            let units = bytes.chunks_exact(2).map(|p| if le { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) });
            let mut ok = bytes.len() % 2 == 0;
            let mut s = String::with_capacity(bytes.len() / 2);
            for c in char::decode_utf16(units) {
                s.push(c.unwrap_or_else(|_| {
                    ok = false;
                    char::REPLACEMENT_CHARACTER
                }));
            }
            if bytes.len() % 2 == 1 {
                s.push(char::REPLACEMENT_CHARACTER);
            }
            (s, ok)
        }
        Encoding::Windows1252 => (bytes.iter().map(|&b| cp1252_char(b)).collect(), true),
    }
}

/// `text` in `enc`, with its BOM when `bom` (UTF-8 and UTF-16). Err names the
/// first char Windows-1252 has no byte for.
pub fn encode(text: &str, enc: Encoding, bom: bool) -> Result<Vec<u8>, char> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if bom {
        out.extend_from_slice(enc.bom_bytes());
    }
    encode_into(text, enc, &mut out)?;
    Ok(out)
}

/// Append `text` in `enc` (no BOM): for writing a rope chunk by chunk.
pub fn encode_into(text: &str, enc: Encoding, out: &mut Vec<u8>) -> Result<(), char> {
    match enc {
        Encoding::Utf8 => out.extend_from_slice(text.as_bytes()),
        Encoding::Utf16Le => {
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        Encoding::Utf16Be => {
            for u in text.encode_utf16() {
                out.extend_from_slice(&u.to_be_bytes());
            }
        }
        Encoding::Windows1252 => {
            for c in text.chars() {
                out.push(cp1252_byte(c).ok_or(c)?);
            }
        }
    }
    Ok(())
}

/// The first char of `text` that `enc` can't store (only Windows-1252 has any).
pub fn unencodable(text: &str, enc: Encoding) -> Option<char> {
    match enc {
        Encoding::Windows1252 => text.chars().find(|&c| cp1252_byte(c).is_none()),
        _ => None,
    }
}

/// Windows-1252's 0x80..=0x9F; 0 marks the five undefined bytes.
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0, 0x017D, 0, //
    0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

pub fn cp1252_char(b: u8) -> char {
    match b {
        0x80..=0x9F => match CP1252_HIGH[(b - 0x80) as usize] {
            0 => char::from(b),
            u => char::from_u32(u as u32).unwrap_or(char::REPLACEMENT_CHARACTER),
        },
        _ => char::from(b),
    }
}

fn cp1252_byte(c: char) -> Option<u8> {
    let u = c as u32;
    match u {
        0..=0x7F | 0xA0..=0xFF => Some(u as u8),
        0x80..=0x9F => (CP1252_HIGH[(u - 0x80) as usize] == 0).then_some(u as u8),
        _ => CP1252_HIGH.iter().position(|&x| x as u32 == u && x != 0).map(|i| 0x80 + i as u8),
    }
}

/// Decode one line's bytes for display (large files): UTF-8 when it's valid
/// or `enc` says so, else Windows-1252.
pub fn decode_line(bytes: &[u8], enc: Encoding) -> String {
    match enc {
        Encoding::Windows1252 => bytes.iter().map(|&b| cp1252_char(b)).collect(),
        Encoding::Utf16Le | Encoding::Utf16Be => decode(bytes, enc).0,
        Encoding::Utf8 => match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => String::from_utf8_lossy(bytes).into_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_boms_and_utf8() {
        assert_eq!(detect(b"\xEF\xBB\xBFhi"), (Encoding::Utf8, true));
        assert_eq!(detect(b"\xFF\xFEh\0i\0"), (Encoding::Utf16Le, true));
        assert_eq!(detect(b"\xFE\xFF\0h\0i"), (Encoding::Utf16Be, true));
        assert_eq!(detect("héllo ✓".as_bytes()), (Encoding::Utf8, false));
        assert_eq!(detect(b""), (Encoding::Utf8, false));
        // Invalid UTF-8 (a Latin-1 é) falls back to Windows-1252.
        assert_eq!(detect(b"caf\xE9"), (Encoding::Windows1252, false));
        // A prefix cut inside a multi-byte char is still UTF-8; a whole file isn't.
        assert_eq!(guess(&"aé".as_bytes()[..2]), (Encoding::Utf8, false));
        assert_eq!(detect(&"aé".as_bytes()[..2]), (Encoding::Windows1252, false));
    }

    #[test]
    fn detects_utf16_without_bom() {
        let le: Vec<u8> = "hello world\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let be: Vec<u8> = "hello world\n".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert_eq!(detect(&le), (Encoding::Utf16Le, false));
        assert_eq!(detect(&be), (Encoding::Utf16Be, false));
        assert!(!is_binary(&le));
        assert_eq!(decode(&le, Encoding::Utf16Le), ("hello world\n".to_string(), true));
    }

    #[test]
    fn binary_needs_a_nul() {
        assert!(is_binary(b"\x7FELF\x02\x01\x01\0\0\0\0"));
        assert!(!is_binary(b"plain text\n"));
        assert!(!is_binary(b"caf\xE9 au lait"));
        // A PNG header: NUL bytes, not UTF-16-shaped.
        assert!(is_binary(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x01\0"));
        // A NUL after the first 8 KB doesn't count.
        let mut late = vec![b'a'; SNIFF_LEN];
        late.push(0);
        assert!(!is_binary(&late));
    }

    #[test]
    fn round_trips_every_encoding() {
        let text = "Grüße, “quotes” — €5 ™\nline 2\n";
        for (enc, bom) in CHOICES {
            let bytes = encode(text, enc, bom).unwrap();
            assert_eq!(detect(&bytes), (enc, bom), "{}", label(enc, bom));
            assert_eq!(decode(&bytes, enc), (text.to_string(), true), "{}", label(enc, bom));
        }
    }

    #[test]
    fn utf16_surrogate_pairs() {
        let text = "emoji 😀 and 𝄞";
        for enc in [Encoding::Utf16Le, Encoding::Utf16Be] {
            let bytes = encode(text, enc, true).unwrap();
            assert_eq!(decode(&bytes, enc), (text.to_string(), true));
        }
    }

    #[test]
    fn invalid_sequences_are_lossy() {
        // A lone high surrogate.
        let (s, ok) = decode(&[0x3D, 0xD8, b'a', 0], Encoding::Utf16Le);
        assert!(!ok);
        assert_eq!(s, "\u{FFFD}a");
        // An odd byte at the end.
        let (s, ok) = decode(&[b'a', 0, b'b'], Encoding::Utf16Le);
        assert!(!ok);
        assert_eq!(s, "a\u{FFFD}");
        // Bad UTF-8 decoded as UTF-8 (Reopen with Encoding).
        let (s, ok) = decode(b"a\xFFb", Encoding::Utf8);
        assert!(!ok);
        assert_eq!(s, "a\u{FFFD}b");
    }

    #[test]
    fn windows_1252_reads_and_writes_every_byte() {
        let all: Vec<u8> = (0..=255).collect();
        let (s, ok) = decode(&all, Encoding::Windows1252);
        assert!(ok);
        assert_eq!(s.chars().count(), 256);
        assert_eq!(encode(&s, Encoding::Windows1252, false).unwrap(), all);
        assert_eq!(cp1252_char(0x80), '€');
        assert_eq!(cp1252_char(0x93), '“');
        assert_eq!(cp1252_char(0x81), '\u{81}');
        assert_eq!(cp1252_char(0xE9), 'é');
    }

    #[test]
    fn windows_1252_refuses_what_it_cant_store() {
        assert_eq!(encode("ok €", Encoding::Windows1252, false), Ok(b"ok \x80".to_vec()));
        assert_eq!(encode("日本", Encoding::Windows1252, false), Err('日'));
        assert_eq!(unencodable("abc ✓", Encoding::Windows1252), Some('✓'));
        assert_eq!(unencodable("abc ✓", Encoding::Utf16Be), None);
    }

    #[test]
    fn labels() {
        assert_eq!(label(Encoding::Utf8, true), "UTF-8 BOM");
        assert_eq!(label(Encoding::Utf16Le, false), "UTF-16 LE (no BOM)");
        assert_eq!(label(Encoding::Windows1252, false), "Windows-1252");
    }
}
