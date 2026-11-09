//! Character-column decoding for the hex viewer's supported encodings.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CharacterEncoding {
    #[default]
    Ascii,
    Windows1251,
    Windows1252,
    Koi8R,
    Koi8U,
    Ibm866,
    Utf8,
    Utf16Le,
    Utf16Be,
}
impl CharacterEncoding {
    pub(super) const ALL: [(Self, &'static str); 9] = [
        (Self::Ascii, "ASCII"),
        (Self::Windows1251, "Windows-1251"),
        (Self::Windows1252, "Windows-1252"),
        (Self::Koi8R, "KOI8-R"),
        (Self::Koi8U, "KOI8-U"),
        (Self::Ibm866, "IBM866"),
        (Self::Utf8, "UTF-8"),
        (Self::Utf16Le, "UTF-16 LE"),
        (Self::Utf16Be, "UTF-16 BE"),
    ];
    pub(super) fn from_bom(bytes: &[u8]) -> Option<Self> {
        // Check longer unsupported signatures before their UTF-16 prefix.
        if bytes.starts_with(&[0xff, 0xfe, 0, 0]) || bytes.starts_with(&[0, 0, 0xfe, 0xff]) {
            None
        } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            Some(Self::Utf8)
        } else if bytes.starts_with(&[0xff, 0xfe]) {
            Some(Self::Utf16Le)
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            Some(Self::Utf16Be)
        } else {
            None
        }
    }
    pub(super) fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|(encoding, _)| *encoding == self)
            .unwrap();
        Self::ALL[(index + 1) % Self::ALL.len()].0
    }
    pub(super) fn label(self) -> &'static str {
        Self::ALL
            .into_iter()
            .find(|(mode, _)| *mode == self)
            .unwrap()
            .1
    }
    #[cfg(test)]
    pub(super) fn glyph_at(self, bytes: &[u8], index: usize) -> char {
        self.glyph_at_offset(bytes, index, 0)
    }
    pub(super) fn glyph_at_offset(self, bytes: &[u8], index: usize, context_offset: u64) -> char {
        if matches!(self, Self::Utf16Le | Self::Utf16Be) {
            let unit = |start: usize| {
                bytes.get(start..start + 2).map(|b| {
                    if self == Self::Utf16Le {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                })
            };
            for start in index.saturating_sub(3)..=index {
                if !(context_offset + start as u64).is_multiple_of(2) {
                    continue;
                }
                let Some(first) = unit(start) else {
                    continue;
                };
                let (scalar, length) = match first {
                    0xd800..=0xdbff => {
                        let Some(second @ 0xdc00..=0xdfff) = unit(start + 2) else {
                            continue;
                        };
                        (
                            0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second)
                                - 0xdc00,
                            4,
                        )
                    }
                    0xdc00..=0xdfff => continue,
                    _ => (u32::from(first), 2),
                };
                if index >= start + length {
                    continue;
                }
                return if index == start {
                    visible_character(char::from_u32(scalar).unwrap())
                } else {
                    '›'
                };
            }
            return '.';
        }
        if self != Self::Utf8 {
            return self.glyph(bytes[index]);
        }
        for start in index.saturating_sub(3)..=index {
            let length = match bytes[start] {
                0x00..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => continue,
            };
            if index >= start + length {
                continue;
            }
            if let Some(sequence) = bytes.get(start..start + length)
                && let Ok(text) = std::str::from_utf8(sequence)
            {
                return if index == start {
                    visible_character(text.chars().next().unwrap())
                } else {
                    '›'
                };
            }
        }
        '.'
    }
    pub(super) fn glyph(self, byte: u8) -> char {
        let encoding = match self {
            Self::Utf16Le | Self::Utf16Be => return '.',
            Self::Utf8 | Self::Ascii => {
                return if (0x20..=0x7e).contains(&byte) {
                    char::from(byte)
                } else {
                    '.'
                };
            }
            Self::Windows1251 => encoding_rs::WINDOWS_1251,
            Self::Windows1252 => encoding_rs::WINDOWS_1252,
            Self::Koi8R => encoding_rs::KOI8_R,
            Self::Koi8U => encoding_rs::KOI8_U,
            Self::Ibm866 => encoding_rs::IBM866,
        };
        static TABLES: [std::sync::OnceLock<[char; 256]>; 5] =
            [const { std::sync::OnceLock::new() }; 5];
        TABLES[self as usize - 1].get_or_init(|| {
            std::array::from_fn(|index| {
                let bytes = [index as u8];
                let (text, errors) = encoding.decode_without_bom_handling(&bytes);
                let ch = text.chars().next().unwrap_or('.');
                if errors { '.' } else { visible_character(ch) }
            })
        })[usize::from(byte)]
    }
}

fn visible_character(ch: char) -> char {
    if ch.is_control()
        || (0xfdd0..=0xfdef).contains(&u32::from(ch))
        || u32::from(ch) & 0xffff >= 0xfffe
        || ch == char::REPLACEMENT_CHARACTER
        || (ch.is_whitespace() && ch != ' ')
        || unicode_normalization::char::is_combining_mark(ch)
        || matches!(ch, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'
            | '\u{e0000}'..='\u{e007f}')
    {
        '.'
    } else {
        ch
    }
}
