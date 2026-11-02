//! Streaming exact-byte search in either direction, independent of page size.
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_PATTERN: usize = 256;
pub const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Hex,
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1251,
    Windows1252,
    Koi8R,
    Koi8U,
    Ibm866,
    Mixed,
}
impl InputMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hex => "Hex bytes",
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Windows1251 => "Windows-1251",
            Self::Windows1252 => "Windows-1252",
            Self::Koi8R => "KOI8-R",
            Self::Koi8U => "KOI8-U",
            Self::Ibm866 => "IBM866",
            Self::Mixed => "Mixed text/hex",
        }
    }
    pub fn parse(self, input: &str) -> Result<Vec<u8>, &'static str> {
        match self {
            Self::Hex => parse(input),
            Self::Mixed => parse_mixed(input),
            Self::Utf8 if input.is_empty() => Err("Enter UTF-8 text to search for."),
            Self::Utf8 if input.len() > MAX_PATTERN => Err("Search is limited to 256 UTF-8 bytes."),
            Self::Utf8 => Ok(input.as_bytes().to_vec()),
            Self::Windows1251 | Self::Windows1252 | Self::Koi8R | Self::Koi8U | Self::Ibm866 => {
                if input.is_empty() {
                    return Err("Enter text to search in the selected codepage.");
                }
                if input.chars().count() > MAX_PATTERN {
                    return Err("Search is limited to 256 encoded bytes.");
                }
                let encoding = match self {
                    Self::Windows1251 => encoding_rs::WINDOWS_1251,
                    Self::Windows1252 => encoding_rs::WINDOWS_1252,
                    Self::Koi8R => encoding_rs::KOI8_R,
                    Self::Koi8U => encoding_rs::KOI8_U,
                    Self::Ibm866 => encoding_rs::IBM866,
                    _ => unreachable!(),
                };
                let (bytes, _, replaced) = encoding.encode(input);
                if replaced {
                    return Err("Some characters cannot be represented in the selected codepage.");
                }
                if bytes.len() > MAX_PATTERN {
                    return Err("Search is limited to 256 encoded bytes.");
                }
                Ok(bytes.into_owned())
            }
            Self::Utf16Le | Self::Utf16Be => {
                if input.is_empty() {
                    return Err("Enter UTF-16 text to search for.");
                }
                let mut bytes = Vec::new();
                for unit in input.encode_utf16() {
                    if bytes.len() == MAX_PATTERN {
                        return Err("Search is limited to 256 UTF-16 bytes.");
                    }
                    bytes.extend_from_slice(&if self == Self::Utf16Le {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    });
                }
                Ok(bytes)
            }
        }
    }
}

/// Strict mixed UTF-8 literals and hexadecimal byte constants, bounded before encoding.
fn parse_mixed(input: &str) -> Result<Vec<u8>, &'static str> {
    if input.chars().count() > 1024 {
        return Err("Mixed search input is limited to 1024 characters.");
    }
    let mut chars = input.chars().peekable();
    let mut bytes = Vec::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_whitespace() {
            chars.next();
            continue;
        }
        if ch == '"' {
            chars.next();
            loop {
                let Some(mut ch) = chars.next() else {
                    return Err("Close the quoted UTF-8 literal with a double quote.");
                };
                if ch == '"' {
                    break;
                }
                if ch == '\\' {
                    ch = match chars.next() {
                        Some('"') => '"',
                        Some('\\') => '\\',
                        Some('n') => '\n',
                        Some('r') => '\r',
                        Some('t') => '\t',
                        Some('0') => '\0',
                        _ => {
                            return Err(
                                "Quoted literals support only \\\", \\\\, \\n, \\r, \\t and \\0 escapes.",
                            );
                        }
                    };
                }
                let mut buffer = [0; 4];
                let encoded = ch.encode_utf8(&mut buffer).as_bytes();
                if bytes.len() + encoded.len() > MAX_PATTERN {
                    return Err("Search is limited to 256 encoded bytes.");
                }
                bytes.extend_from_slice(encoded);
            }
            if chars.peek().is_some_and(|ch| !ch.is_ascii_whitespace()) {
                return Err("Separate quoted literals and byte constants with whitespace.");
            }
        } else {
            let mut token = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_ascii_whitespace() {
                    break;
                }
                token.push(ch);
                chars.next();
            }
            let hex = token
                .strip_prefix("0x")
                .or_else(|| token.strip_prefix("0X"))
                .unwrap_or(&token);
            if hex.is_empty() || !hex.bytes().all(|ch| ch.is_ascii_hexdigit()) {
                return Err("Use hexadecimal byte constants or double-quoted UTF-8 literals.");
            }
            let byte = u64::from_str_radix(hex, 16)
                .ok()
                .and_then(|byte| u8::try_from(byte).ok())
                .ok_or("Each hexadecimal constant must fit one byte (00 to FF).");
            if bytes.len() == MAX_PATTERN {
                return Err("Search is limited to 256 encoded bytes.");
            }
            bytes.push(byte?);
        }
    }
    if bytes.is_empty() {
        return Err("Enter at least one byte or a nonempty quoted literal.");
    }
    Ok(bytes)
}

pub fn parse(input: &str) -> Result<Vec<u8>, &'static str> {
    let mut nibbles = Vec::new();
    for byte in input.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return Err("Use hexadecimal byte pairs, for example DE AD BE EF."),
        };
        if nibbles.len() == MAX_PATTERN * 2 {
            return Err("Search is limited to 256 bytes.");
        }
        nibbles.push(nibble);
    }
    if nibbles.is_empty() || !nibbles.len().is_multiple_of(2) {
        return Err("Enter one or more complete hexadecimal byte pairs.");
    }
    Ok(nibbles
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| pair[0] * 16 + pair[1])
        .collect())
}

struct Matcher {
    pattern: Vec<u8>,
    prefix: Vec<usize>,
    matched: usize,
}
impl Matcher {
    fn new(pattern: Vec<u8>) -> Self {
        let mut prefix = vec![0; pattern.len()];
        let mut length = 0;
        for index in 1..pattern.len() {
            while length > 0 && pattern[index] != pattern[length] {
                length = prefix[length - 1];
            }
            if pattern[index] == pattern[length] {
                length += 1;
            }
            prefix[index] = length;
        }
        Self {
            pattern,
            prefix,
            matched: 0,
        }
    }
    fn push(&mut self, byte: u8) -> bool {
        while self.matched > 0 && byte != self.pattern[self.matched] {
            self.matched = self.prefix[self.matched - 1];
        }
        if byte == self.pattern[self.matched] {
            self.matched += 1;
        }
        if self.matched == self.pattern.len() {
            self.matched = self.prefix[self.matched - 1];
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy)]
pub struct Options {
    /// Inclusive candidate offset; None means just beyond the directional edge.
    pub start: Option<u64>,
    pub direction: Direction,
    pub wrap: bool,
}

/// Search each candidate at most once. The two byte windows can overlap by
/// pattern length minus one to recognize matches crossing the split point.
pub fn find_options(
    reader: &mut (impl Read + Seek),
    size: u64,
    pattern: &[u8],
    options: Options,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
) -> Result<Option<(u64, bool)>, String> {
    if pattern.is_empty() || pattern.len() > MAX_PATTERN {
        return Err("Invalid hex search pattern length.".into());
    }
    if cancelled() {
        return Err("Hex search cancelled".into());
    }
    if pattern.len() as u64 > size {
        return Ok(None);
    }
    let end = size - pattern.len() as u64 + 1;
    let split = match options.direction {
        Direction::Forward => options.start.unwrap_or(end).min(end),
        Direction::Backward => options.start.map_or(0, |start| start.min(end - 1) + 1),
    };
    let (first, second) = match options.direction {
        Direction::Forward => (split..end, 0..split),
        Direction::Backward => (0..split, split..end),
    };
    if let Some(offset) = find_range(
        reader,
        first,
        pattern,
        options.direction,
        cancelled,
        progress,
    )? {
        return Ok(Some((offset, false)));
    }
    if cancelled() {
        return Err("Hex search cancelled".into());
    }
    if options.wrap {
        return find_range(
            reader,
            second,
            pattern,
            options.direction,
            cancelled,
            progress,
        )
        .map(|found| found.map(|offset| (offset, true)));
    }
    Ok(None)
}

#[cfg(test)]
fn find(
    reader: &mut (impl Read + Seek),
    size: u64,
    start: u64,
    pattern: &[u8],
    direction: Direction,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
) -> Result<Option<u64>, String> {
    find_options(
        reader,
        size,
        pattern,
        Options {
            start: Some(start),
            direction,
            wrap: false,
        },
        cancelled,
        progress,
    )
    .map(|found| found.map(|(offset, _)| offset))
}

fn find_range(
    reader: &mut (impl Read + Seek),
    candidates: std::ops::Range<u64>,
    pattern: &[u8],
    direction: Direction,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
) -> Result<Option<u64>, String> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let low = candidates.start;
    let high = candidates.end - 1 + pattern.len() as u64;
    let mut needle = pattern.to_vec();
    if direction == Direction::Backward {
        needle.reverse();
    }
    let mut matcher = Matcher::new(needle);
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut cursor = match direction {
        Direction::Forward => low,
        Direction::Backward => high,
    };
    loop {
        if cancelled() || !crate::io_budget::background_checkpoint(cancelled) {
            return Err("Hex search cancelled".into());
        }
        let (offset, length) = match direction {
            Direction::Forward => (cursor, (high - cursor).min(CHUNK_BYTES as u64) as usize),
            Direction::Backward => {
                let length = (cursor - low).min(CHUNK_BYTES as u64) as usize;
                (cursor - length as u64, length)
            }
        };
        if length == 0 {
            return Ok(None);
        }
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|error| error.to_string())?;
        let mut read = 0;
        while read < length {
            if cancelled() {
                return Err("Hex search cancelled".into());
            }
            match reader.read(&mut buffer[read..length]) {
                Ok(0) => return Err("The file changed during hex search. Reload and retry.".into()),
                Ok(count) => {
                    read += count;
                    progress.fetch_add(count as u64, Ordering::Relaxed);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        for index in 0..length {
            if index.is_multiple_of(4096) && cancelled() {
                return Err("Hex search cancelled".into());
            }
            let source_index = if direction == Direction::Forward {
                index
            } else {
                length - index - 1
            };
            if matcher.push(buffer[source_index]) {
                let position = offset + source_index as u64;
                return Ok(Some(match direction {
                    Direction::Forward => position + 1 - pattern.len() as u64,
                    Direction::Backward => position,
                }));
            }
        }
        cursor = match direction {
            Direction::Forward => offset + length as u64,
            Direction::Backward => offset,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(data: &[u8], start: u64, pattern: &[u8], direction: Direction) -> Option<u64> {
        find(
            &mut Cursor::new(data),
            data.len() as u64,
            start,
            pattern,
            direction,
            &|| false,
            &AtomicU64::new(0),
        )
        .unwrap()
    }

    #[test]
    fn cyclic_search_agrees_with_naive_candidate_order_including_empty_edges() {
        for size in 0..=7 {
            for bits in 0..1_usize << size {
                let data = (0..size)
                    .map(|n| ((bits >> n) & 1) as u8)
                    .collect::<Vec<_>>();
                for pattern in [&[0][..], &[1][..], &[0, 1][..], &[1, 1, 1][..]] {
                    let hits = data
                        .windows(pattern.len())
                        .enumerate()
                        .filter_map(|(i, p)| (p == pattern).then_some(i as u64))
                        .collect::<Vec<_>>();
                    for start in std::iter::once(None).chain((0..size as u64 + 2).map(Some)) {
                        for direction in [Direction::Forward, Direction::Backward] {
                            for wrap in [false, true] {
                                let ordered = if direction == Direction::Forward {
                                    hits.clone()
                                } else {
                                    hits.iter().rev().copied().collect()
                                };
                                let first = ordered.iter().copied().find(|offset| {
                                    start.is_some_and(|start| match direction {
                                        Direction::Forward => *offset >= start,
                                        Direction::Backward => *offset <= start,
                                    })
                                });
                                let expected = first.map(|n| (n, false)).or_else(|| {
                                    wrap.then(|| ordered.first().map(|n| (*n, true))).flatten()
                                });
                                let actual = find_options(
                                    &mut Cursor::new(&data),
                                    data.len() as u64,
                                    pattern,
                                    Options {
                                        start,
                                        direction,
                                        wrap,
                                    },
                                    &|| false,
                                    &AtomicU64::new(0),
                                )
                                .unwrap();
                                assert_eq!(
                                    actual, expected,
                                    "{data:?}, {pattern:?}, {start:?}, {direction:?}, wrap={wrap}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn wrapped_windows_keep_context_without_joining_file_edges_or_rescanning_candidates() {
        let mut data = vec![0; CHUNK_BYTES * 3 + 17];
        let pattern = b"ABABA";
        for offset in [CHUNK_BYTES - 2, CHUNK_BYTES * 2 + 3] {
            data[offset..offset + pattern.len()].copy_from_slice(pattern);
        }
        for (direction, start, expected) in [
            (Direction::Forward, data.len() as u64, CHUNK_BYTES - 2),
            (Direction::Backward, 0, CHUNK_BYTES * 2 + 3),
            (
                Direction::Forward,
                (CHUNK_BYTES - 1) as u64,
                CHUNK_BYTES * 2 + 3,
            ),
        ] {
            let progress = AtomicU64::new(0);
            let found = find_options(
                &mut Cursor::new(&data),
                data.len() as u64,
                pattern,
                Options {
                    start: Some(start),
                    direction,
                    wrap: true,
                },
                &|| false,
                &progress,
            )
            .unwrap()
            .unwrap();
            assert_eq!(found.0, expected as u64);
            assert!(progress.load(Ordering::Relaxed) < data.len() as u64 + pattern.len() as u64);
        }
        for direction in [Direction::Forward, Direction::Backward] {
            let progress = AtomicU64::new(0);
            assert!(
                find_options(
                    &mut Cursor::new(b"BA"),
                    2,
                    b"AB",
                    Options {
                        start: Some(1),
                        direction,
                        wrap: true
                    },
                    &|| false,
                    &progress
                )
                .unwrap()
                .is_none()
            );
            progress.store(0, Ordering::Relaxed);
            assert!(
                find_options(
                    &mut Cursor::new(&data),
                    data.len() as u64,
                    b"missing",
                    Options {
                        start: Some(CHUNK_BYTES as u64),
                        direction,
                        wrap: true
                    },
                    &|| false,
                    &progress
                )
                .unwrap()
                .is_none()
            );
            assert!(progress.load(Ordering::Relaxed) <= data.len() as u64 + 7);
        }
    }

    #[test]
    fn cancellation_between_passes_prevents_reading_the_wrapped_window() {
        let data = vec![0; CHUNK_BYTES * 2];
        for direction in [Direction::Forward, Direction::Backward] {
            let progress = AtomicU64::new(0);
            let start = if direction == Direction::Forward {
                CHUNK_BYTES as u64
            } else {
                CHUNK_BYTES as u64 - 1
            };
            let result = find_options(
                &mut Cursor::new(&data),
                data.len() as u64,
                &[1],
                Options {
                    start: Some(start),
                    direction,
                    wrap: true,
                },
                &|| progress.load(Ordering::Relaxed) >= CHUNK_BYTES as u64,
                &progress,
            );
            assert!(result.unwrap_err().contains("cancelled"));
            assert_eq!(progress.load(Ordering::Relaxed), CHUNK_BYTES as u64);
        }
    }

    #[test]
    fn legacy_cyrillic_codepages_match_independent_known_vectors_and_strict_limits() {
        assert_eq!(
            InputMode::Koi8R.parse("Привет ё").unwrap(),
            [0xf0, 0xd2, 0xc9, 0xd7, 0xc5, 0xd4, 0x20, 0xa3]
        );
        assert_eq!(
            InputMode::Koi8U.parse("іІїЇєЄґҐ").unwrap(),
            [0xa6, 0xb6, 0xa7, 0xb7, 0xa4, 0xb4, 0xad, 0xbd]
        );
        assert_eq!(
            InputMode::Ibm866.parse("Привет Ёё").unwrap(),
            [0x8f, 0xe0, 0xa8, 0xa2, 0xa5, 0xe2, 0x20, 0xf0, 0xf1]
        );
        assert!(InputMode::Koi8R.parse("і").is_err());
        for mode in [InputMode::Koi8R, InputMode::Koi8U, InputMode::Ibm866] {
            assert_eq!(mode.parse(" \0 ").unwrap(), b" \0 ");
            assert_eq!(mode.parse(&"я".repeat(256)).unwrap().len(), 256);
            assert!(mode.parse(&"я".repeat(257)).is_err());
            assert!(mode.parse("").is_err());
            assert!(mode.parse("🌍").is_err());
            assert!(mode.parse("€").is_err());
            assert!(mode.parse("е\u{301}").is_err());
        }
    }

    #[test]
    fn mixed_literals_and_hex_constants_preserve_utf8_escapes_and_numeric_hex_semantics() {
        assert_eq!(
            InputMode::Mixed
                .parse(r#""String" 34 0xBB 012 "more text""#)
                .unwrap(),
            [b"String".as_slice(), &[0x34, 0xbb, 0x12], b"more text"].concat()
        );
        assert_eq!(
            InputMode::Mixed.parse(r#"" ёж🌍 " 00 f 0XFF"#).unwrap(),
            [" ёж🌍 ".as_bytes(), &[0, 15, 255]].concat()
        );
        assert_eq!(
            InputMode::Mixed.parse(r#""\"\\\n\r\t\0""#).unwrap(),
            b"\"\\\n\r\t\0"
        );
        assert_eq!(InputMode::Mixed.parse(r#""" 0000ff """#).unwrap(), [255]);
        assert_eq!(InputMode::Mixed.parse("10").unwrap(), [0x10]);
        assert!(InputMode::Hex.parse(r#""A""#).is_err());
    }

    #[test]
    fn mixed_search_rejects_partial_malformed_and_over_budget_input() {
        for input in [
            "",
            " ",
            r#""""#,
            r#""unfinished"#,
            r#""x\q""#,
            "\"x\\",
            r#""A"42"#,
            "AB\"A\"",
            "0x",
            "GG",
            "100",
            "-1",
            "Привет",
        ] {
            assert!(InputMode::Mixed.parse(input).is_err(), "{input:?}");
        }
        assert_eq!(
            InputMode::Mixed
                .parse(&format!("\"{}\"", "🌍".repeat(64)))
                .unwrap()
                .len(),
            256
        );
        assert!(
            InputMode::Mixed
                .parse(&format!("\"{}\"", "🌍".repeat(65)))
                .is_err()
        );
        assert_eq!(
            InputMode::Mixed.parse(&"FF ".repeat(256)).unwrap().len(),
            256
        );
        assert!(InputMode::Mixed.parse(&"FF ".repeat(257)).is_err());
        assert!(InputMode::Mixed.parse(&" ".repeat(1025)).is_err());
    }

    #[test]
    fn windows_codepages_encode_exact_bytes_without_lossy_replacement_or_normalization() {
        assert_eq!(
            InputMode::Windows1251.parse("Привет ё€\0").unwrap(),
            [0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2, 0x20, 0xb8, 0x88, 0]
        );
        assert_eq!(
            InputMode::Windows1252.parse("café €“”\0").unwrap(),
            [0x63, 0x61, 0x66, 0xe9, 0x20, 0x80, 0x93, 0x94, 0]
        );
        for mode in [InputMode::Windows1251, InputMode::Windows1252] {
            assert!(mode.parse("").is_err());
            assert_eq!(mode.parse(" A ").unwrap(), b" A ");
            assert_eq!(mode.parse(&"A".repeat(256)).unwrap().len(), 256);
            assert!(mode.parse(&"A".repeat(257)).is_err());
            assert!(mode.parse("🌍").is_err());
            assert!(mode.parse("e\u{301}").is_err());
            assert!(mode.parse("\u{feff}").is_err());
        }
        assert!(InputMode::Windows1252.parse("Привет").is_err());
        assert!(InputMode::Windows1251.parse("café").is_err());
    }

    #[test]
    fn utf16_modes_encode_surrogate_pairs_nul_and_bom_with_strict_encoded_limits() {
        assert_eq!(
            InputMode::Utf16Le.parse("Aя🌍\0").unwrap(),
            [0x41, 0, 0x4f, 4, 0x3c, 0xd8, 0x0d, 0xdf, 0, 0]
        );
        assert_eq!(
            InputMode::Utf16Be.parse("Aя🌍\0").unwrap(),
            [0, 0x41, 4, 0x4f, 0xd8, 0x3c, 0xdf, 0x0d, 0, 0]
        );
        assert_eq!(InputMode::Utf16Le.parse("\u{feff}").unwrap(), [0xff, 0xfe]);
        assert_eq!(InputMode::Utf16Be.parse("\u{feff}").unwrap(), [0xfe, 0xff]);
        for mode in [InputMode::Utf16Le, InputMode::Utf16Be] {
            assert!(mode.parse("").is_err());
            assert_eq!(mode.parse(&"A".repeat(128)).unwrap().len(), 256);
            assert!(mode.parse(&"A".repeat(129)).is_err());
            assert_eq!(mode.parse(&"🌍".repeat(64)).unwrap().len(), 256);
            assert!(mode.parse(&"🌍".repeat(65)).is_err());
            assert!(mode.parse(&("🌍".repeat(63) + "AAA")).is_err());
            assert_ne!(mode.parse("é"), mode.parse("e\u{301}"));
        }
    }

    #[test]
    fn text_mode_preserves_exact_utf8_whitespace_and_byte_limits() {
        for text in ["Привет🌍", " a\t\n ", "DE AD", "\0", "é", "e\u{301}"] {
            assert_eq!(InputMode::Utf8.parse(text).unwrap(), text.as_bytes());
        }
        assert!(InputMode::Utf8.parse("").is_err());
        assert_eq!(InputMode::Utf8.parse(&"🌍".repeat(64)).unwrap().len(), 256);
        assert!(InputMode::Utf8.parse(&"🌍".repeat(65)).is_err());
        assert_ne!(
            InputMode::Utf8.parse("é"),
            InputMode::Utf8.parse("e\u{301}")
        );
        assert_eq!(InputMode::default(), InputMode::Hex);
        assert_eq!(InputMode::Hex.parse("de ad"), Ok(vec![0xde, 0xad]));
        assert!(InputMode::Hex.parse("Привет").is_err());
    }

    #[test]
    fn parser_accepts_case_and_spacing_but_not_ambiguous_patterns() {
        assert_eq!(parse("De ad\tBE\nef"), Ok(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(parse("00ff10"), Ok(vec![0, 255, 16]));
        assert_eq!(parse(&"ff".repeat(MAX_PATTERN)).unwrap().len(), MAX_PATTERN);
        for input in ["", " ", "a", "aa b", "0xff", "gg", "é", "aa?b", "-1"] {
            assert!(parse(input).is_err(), "{input}");
        }
        assert!(parse(&"ff".repeat(MAX_PATTERN + 1)).is_err());
    }

    #[test]
    fn matches_cross_page_and_read_chunk_boundaries_in_both_directions() {
        let mut data = vec![0x22; 3 * CHUNK_BYTES + 17];
        let pattern = [0xde, 0xad, 0xbe, 0xef, 0, 255];
        let offsets = [4094, CHUNK_BYTES - 2, 2 * CHUNK_BYTES - 2];
        for offset in offsets {
            data[offset..offset + pattern.len()].copy_from_slice(&pattern);
        }
        for start in [
            0,
            4094,
            4095,
            (CHUNK_BYTES - 2) as u64,
            CHUNK_BYTES as u64,
            data.len() as u64,
            u64::MAX,
        ] {
            assert_eq!(
                run(&data, start, &pattern, Direction::Forward),
                offsets
                    .into_iter()
                    .find(|offset| *offset as u64 >= start)
                    .map(|offset| offset as u64)
            );
            assert_eq!(
                run(&data, start, &pattern, Direction::Backward),
                offsets
                    .into_iter()
                    .rev()
                    .find(|offset| *offset as u64 <= start)
                    .map(|offset| offset as u64)
            );
        }
    }

    #[test]
    fn repeated_prefixes_overlapping_matches_empty_and_eof_are_correct() {
        assert_eq!(run(b"aaaaa", 0, b"aaa", Direction::Forward), Some(0));
        assert_eq!(run(b"aaaaa", 1, b"aaa", Direction::Forward), Some(1));
        assert_eq!(run(b"aaaaa", 2, b"aaa", Direction::Backward), Some(2));
        assert_eq!(run(b"aaaaa", 1, b"aaa", Direction::Backward), Some(1));
        assert_eq!(run(b"aaaaa", 5, b"aaa", Direction::Forward), None);
        assert_eq!(run(b"", 0, b"a", Direction::Backward), None);
        assert_eq!(run(b"abc", 0, b"abcd", Direction::Forward), None);
        assert_eq!(run(b"abc", 9, b"abcd", Direction::Backward), None);
        let mut data = vec![b'a'; 3 * CHUNK_BYTES];
        data.push(b'b');
        let mut needle = vec![b'a'; MAX_PATTERN - 1];
        needle.push(b'b');
        let expected = (data.len() - needle.len()) as u64;
        assert_eq!(run(&data, 0, &needle, Direction::Forward), Some(expected));
        assert_eq!(
            run(&data, u64::MAX, &needle, Direction::Backward),
            Some(expected)
        );
    }

    #[test]
    fn streaming_search_agrees_with_a_naive_oracle_on_varied_binary_data() {
        let mut seed = 0x987654321u64;
        for trial in 0..64 {
            let mut data = vec![0u8; 16 + trial];
            for byte in &mut data {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *byte = (seed >> 32) as u8 % 5;
            }
            let pattern = if trial % 2 == 0 {
                data[trial / 4..trial / 4 + 1 + trial % 7].to_vec()
            } else {
                vec![4, 3, 2, 1]
            };
            for start in [0, 1, (trial / 2) as u64, data.len() as u64, u64::MAX] {
                let matches: Vec<_> = data
                    .windows(pattern.len())
                    .enumerate()
                    .filter(|(_, bytes)| *bytes == pattern)
                    .map(|(offset, _)| offset as u64)
                    .collect();
                assert_eq!(
                    run(&data, start, &pattern, Direction::Forward),
                    matches.iter().copied().find(|offset| *offset >= start)
                );
                assert_eq!(
                    run(&data, start, &pattern, Direction::Backward),
                    matches
                        .iter()
                        .copied()
                        .rev()
                        .find(|offset| *offset <= start)
                );
            }
        }
    }

    #[test]
    fn interrupted_and_fragmented_reads_preserve_the_search_stream() {
        struct Partial {
            cursor: Cursor<Vec<u8>>,
            interrupted: bool,
        }
        impl Seek for Partial {
            fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
                self.cursor.seek(position)
            }
        }
        impl Read for Partial {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                let length = buffer.len().min(3);
                self.cursor.read(&mut buffer[..length])
            }
        }
        for direction in [Direction::Forward, Direction::Backward] {
            let data = b"123456789abcdefghijk".to_vec();
            let size = data.len() as u64;
            let mut reader = Partial {
                cursor: Cursor::new(data),
                interrupted: false,
            };
            let progress = AtomicU64::new(0);
            assert_eq!(
                find(
                    &mut reader,
                    size,
                    if direction == Direction::Forward {
                        0
                    } else {
                        size
                    },
                    b"89abc",
                    direction,
                    &|| false,
                    &progress
                )
                .unwrap(),
                Some(7)
            );
            assert_eq!(progress.load(Ordering::Relaxed), size);
        }
    }

    #[test]
    fn cancellation_short_reads_and_invalid_lengths_are_explicit_errors() {
        let data = vec![0u8; 4 * CHUNK_BYTES];
        for direction in [Direction::Forward, Direction::Backward] {
            let progress = AtomicU64::new(0);
            let start = if direction == Direction::Forward {
                0
            } else {
                data.len() as u64
            };
            let result = find(
                &mut Cursor::new(&data),
                data.len() as u64,
                start,
                &[1, 2],
                direction,
                &|| progress.load(Ordering::Relaxed) >= 2 * CHUNK_BYTES as u64,
                &progress,
            );
            assert!(result.unwrap_err().contains("cancelled"));
            assert_eq!(progress.load(Ordering::Relaxed), 2 * CHUNK_BYTES as u64);
        }
        let progress = AtomicU64::new(0);
        assert!(
            find(
                &mut Cursor::new(b"abc"),
                9,
                0,
                b"z",
                Direction::Forward,
                &|| false,
                &progress
            )
            .unwrap_err()
            .contains("changed")
        );
        assert!(
            find(
                &mut Cursor::new(b"abc"),
                3,
                0,
                &[],
                Direction::Forward,
                &|| false,
                &progress
            )
            .is_err()
        );
        assert!(
            find(
                &mut Cursor::new(b"abc"),
                3,
                0,
                &[0; MAX_PATTERN + 1],
                Direction::Forward,
                &|| false,
                &progress
            )
            .is_err()
        );
    }
}
