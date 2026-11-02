//! Signature-based text decoding shared by search and its persisted index.
//! Callers own byte limits, EOF proof and cancellation.
use std::borrow::Cow;

#[derive(Clone, Copy)]
enum Encoding {
    Utf8,
    Utf16(bool),
    Utf32(bool),
}

fn signature(bytes: &[u8]) -> (Encoding, &[u8]) {
    // Longer signatures precede UTF-16LE's shared FF FE prefix.
    for (bom, encoding) in [
        (&[0xff, 0xfe, 0x00, 0x00][..], Encoding::Utf32(true)),
        (&[0x00, 0x00, 0xfe, 0xff][..], Encoding::Utf32(false)),
        (&[0xff, 0xfe][..], Encoding::Utf16(true)),
        (&[0xfe, 0xff][..], Encoding::Utf16(false)),
        (&[0xef, 0xbb, 0xbf][..], Encoding::Utf8),
    ] {
        if let Some(payload) = bytes.strip_prefix(bom) {
            return (encoding, payload);
        }
    }
    (Encoding::Utf8, bytes)
}

/// Inspect only the encoded head. Zero code units remain binary; structural
/// zero bytes in BOM-marked UTF-16/32 do not. Unmarked data keeps the NUL rule.
pub fn binary_head(bytes: &[u8]) -> bool {
    let (encoding, payload) = signature(&bytes[..bytes.len().min(8192)]);
    match encoding {
        Encoding::Utf8 => payload.contains(&0),
        Encoding::Utf16(_) => payload.as_chunks::<2>().0.contains(&[0; 2]),
        Encoding::Utf32(_) => payload.as_chunks::<4>().0.contains(&[0; 4]),
    }
}

/// Decode the complete supplied prefix, with replacement for malformed units.
/// An incomplete tail produces U+FFFD, preserving bounded-match uncertainty.
pub fn decode(bytes: &[u8]) -> Cow<'_, str> {
    let (encoding, payload) = signature(bytes);
    match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(payload),
        Encoding::Utf16(little) => {
            let (units, remainder) = payload.as_chunks::<2>();
            let units = units.iter().map(|unit| {
                if little {
                    u16::from_le_bytes(*unit)
                } else {
                    u16::from_be_bytes(*unit)
                }
            });
            let mut text = String::with_capacity(payload.len());
            for character in char::decode_utf16(units) {
                text.push(character.unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            if !remainder.is_empty() {
                text.push(char::REPLACEMENT_CHARACTER);
            }
            Cow::Owned(text)
        }
        Encoding::Utf32(little) => {
            let (units, remainder) = payload.as_chunks::<4>();
            let mut text = String::with_capacity(payload.len());
            for unit in units {
                let value = if little {
                    u32::from_le_bytes(*unit)
                } else {
                    u32::from_be_bytes(*unit)
                };
                text.push(char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            if !remainder.is_empty() {
                text.push(char::REPLACEMENT_CHARACTER);
            }
            Cow::Owned(text)
        }
    }
}

#[cfg(test)]
pub fn fixture(text: &str, width: usize, little: bool) -> Vec<u8> {
    let mut bytes = match (width, little) {
        (1, _) => vec![0xef, 0xbb, 0xbf],
        (2, true) => vec![0xff, 0xfe],
        (2, false) => vec![0xfe, 0xff],
        (4, true) => vec![0xff, 0xfe, 0, 0],
        (4, false) => vec![0, 0, 0xfe, 0xff],
        _ => unreachable!(),
    };
    match width {
        1 => bytes.extend_from_slice(text.as_bytes()),
        2 => {
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
        }
        4 => {
            for character in text.chars() {
                let unit = u32::from(character);
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
        }
        _ => unreachable!(),
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_search_decoder_decodes_both_orders_and_preserves_payload_bom() {
        for width in [1, 2, 4] {
            for little in [false, true] {
                for text in ["", "READY\r\nкот café 🐱", "\u{feff}second"] {
                    let bytes = fixture(text, width, little);
                    assert!(!binary_head(&bytes));
                    assert_eq!(decode(&bytes), text);
                }
                assert!(binary_head(&fixture("a\0b", width, little)));
            }
        }
        for bytes in [
            &[0xff, 0xfe, 0x41, 0x00][..],
            &[0xfe, 0xff, 0x00, 0x41][..],
            &[0xff, 0xfe, 0, 0, 0x41, 0, 0, 0][..],
            &[0, 0, 0xfe, 0xff, 0, 0, 0, 0x41][..],
        ] {
            assert_eq!(decode(bytes), "A");
            assert!(!binary_head(bytes));
        }
        assert!(binary_head(b"R\0E\0A\0D\0Y\0"));
        assert_eq!(decode(b"caf\xff"), "caf\u{fffd}");
    }

    #[test]
    fn bom_search_decoder_malformed_units_remain_lossy_and_prefixes_do_not_panic() {
        assert_eq!(decode(&[0xff, 0xfe, 0, 0xd8]), "\u{fffd}");
        assert_eq!(decode(&[0xfe, 0xff, 0]), "\u{fffd}");
        assert_eq!(decode(&[0, 0, 0xfe, 0xff, 0, 0x11, 0, 0]), "\u{fffd}");
        for width in [1, 2, 4] {
            for little in [false, true] {
                let bytes = fixture("кот 🐱", width, little);
                for end in 0..=bytes.len() {
                    let _ = decode(&bytes[..end]);
                    let _ = binary_head(&bytes[..end]);
                }
            }
        }
    }
}
