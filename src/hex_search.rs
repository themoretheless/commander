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

/// `start` is an inclusive bound on candidate match offsets. Backward scanning
/// includes enough right context to recognize a match beginning at that bound.
pub fn find(
    reader: &mut (impl Read + Seek),
    size: u64,
    start: u64,
    pattern: &[u8],
    direction: Direction,
    cancelled: &dyn Fn() -> bool,
    progress: &AtomicU64,
) -> Result<Option<u64>, String> {
    if pattern.is_empty() || pattern.len() > MAX_PATTERN {
        return Err("Invalid hex search pattern length.".into());
    }
    let mut needle = pattern.to_vec();
    if direction == Direction::Backward {
        needle.reverse();
    }
    let mut matcher = Matcher::new(needle);
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut cursor = match direction {
        Direction::Forward => start.min(size),
        Direction::Backward => start.saturating_add(pattern.len() as u64).min(size),
    };
    loop {
        if cancelled() || !crate::io_budget::background_checkpoint(cancelled) {
            return Err("Hex search cancelled".into());
        }
        let (offset, length) = match direction {
            Direction::Forward => (cursor, (size - cursor).min(CHUNK_BYTES as u64) as usize),
            Direction::Backward => {
                let length = cursor.min(CHUNK_BYTES as u64) as usize;
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
