//! ZIP LZMA/XZ/Zstandard/PPMd decoding with bounded decoder memory and verified payloads.
//! Other codecs retain zip-rs decoding. Never reread an untrusted LZMA header
//! after validating it: the exact properties consumed here feed liblzma.
use std::io::{self, Cursor, Read, Seek};

const MAX_DECODER_MEMORY: u64 = 64 * 1024 * 1024;
type BoundedInput<'a, R> = std::io::Chain<Cursor<[u8; 13]>, zip::read::ZipFile<'a, R>>;
// ppmd-rust treats UnexpectedEof as a successful partial read. ZIP PPMd
// requires its end marker: preserve premature input exhaustion as an error.
struct PpmdInput<'a, R: Read>(zip::read::ZipFile<'a, R>);
impl<R: Read> Read for PpmdInput<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        match self.0.read(bytes) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ZIP PPMd compressed data ended before the end marker",
            )),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                Err(io::Error::new(io::ErrorKind::InvalidData, e))
            }
            result => result,
        }
    }
}
enum Decoder<'a, R: Read> {
    Ppmd(Box<ppmd_rust::Ppmd8Decoder<PpmdInput<'a, R>>>),
    LzmaXz(xz2::read::XzDecoder<BoundedInput<'a, R>>),
    Zstd(zstd::stream::read::Decoder<'static, std::io::BufReader<zip::read::ZipFile<'a, R>>>),
}
impl<R: Read> Read for Decoder<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Ppmd(d) => d.read(bytes),
            Self::LzmaXz(d) => d.read(bytes),
            Self::Zstd(d) => d.read(bytes).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("Zstandard decode (64 MiB maximum window): {error}"),
                )
            }),
        }
    }
}
impl<'a, R: Read> Decoder<'a, R> {
    fn entry(&self) -> &zip::read::ZipFile<'a, R> {
        match self {
            Self::Ppmd(d) => &d.get_ref().0,
            Self::LzmaXz(d) => d.get_ref().get_ref().1,
            Self::Zstd(d) => d.get_ref().get_ref(),
        }
    }
}
struct Decoded<'a, R: Read> {
    decoder: Decoder<'a, R>,
    codec: &'static str,
    crc: crc32fast::Hasher,
    expected_crc: u32,
    size: u64,
    decoded: u64,
    finished: bool,
}
enum Inner<'a, R: Read> {
    Standard(zip::read::ZipFile<'a, R>),
    Bounded(Box<Decoded<'a, R>>),
}
pub struct Member<'a, R: Read> {
    inner: Inner<'a, R>,
}
impl<'a, R: Read> std::ops::Deref for Member<'a, R> {
    type Target = zip::read::ZipFile<'a, R>;
    fn deref(&self) -> &Self::Target {
        match &self.inner {
            Inner::Standard(entry) => entry,
            Inner::Bounded(member) => member.decoder.entry(),
        }
    }
}
impl<R: Read> Read for Member<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        match &mut self.inner {
            Inner::Standard(entry) => entry.read(bytes),
            Inner::Bounded(member) => {
                if member.finished {
                    return Ok(0);
                }
                let count = member.decoder.read(bytes)?;
                member.decoded = member.decoded.checked_add(count as u64).ok_or_else(|| {
                    io::Error::other(format!("{} decoded size overflow", member.codec))
                })?;
                if member.decoded > member.size {
                    return Err(io::Error::other(format!(
                        "{} member exceeds declared size",
                        member.codec
                    )));
                }
                member.crc.update(&bytes[..count]);
                if count == 0 {
                    if member.decoded != member.size
                        || member.crc.clone().finalize() != member.expected_crc
                    {
                        return Err(io::Error::other(format!(
                            "{} member size or CRC mismatch",
                            member.codec
                        )));
                    }
                    member.finished = true;
                }
                Ok(count)
            }
        }
    }
}
fn validate_properties(header: &[u8; 9]) -> io::Result<()> {
    if u16::from_le_bytes([header[2], header[3]]) != 5 || header[4] >= 225 {
        return Err(io::Error::other("Invalid ZIP LZMA properties"));
    }
    let dictionary = u32::from_le_bytes(header[5..9].try_into().unwrap());
    if u64::from(dictionary) > MAX_DECODER_MEMORY {
        return Err(io::Error::other(
            "ZIP LZMA dictionary exceeds 64 MiB decoder memory limit",
        ));
    }
    Ok(())
}
fn ppmd_parameters(header: [u8; 2]) -> io::Result<(u32, u32, ppmd_rust::RestoreMethod)> {
    let value = u16::from_le_bytes(header);
    let order = u32::from((value & 15) + 1);
    let arena = (u32::from((value >> 4) & 255) + 1) * 1024 * 1024;
    let restoration = (value >> 12) & 15;
    if !(2..=16).contains(&order) || restoration > 1 {
        return Err(io::Error::other(
            "Unsupported ZIP PPMd order or restoration method",
        ));
    }
    if u64::from(arena) > MAX_DECODER_MEMORY {
        return Err(io::Error::other(
            "ZIP PPMd model arena exceeds 64 MiB limit",
        ));
    }
    Ok((order, arena, restoration.into()))
}
/// LZMA/XZ enforce a liblzma memory budget, including probability tables.
/// Zstandard enforces a maximum back-reference window separately.
pub fn open<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
) -> io::Result<Member<'_, R>> {
    let method = archive
        .by_index_data(index)
        .map_err(io::Error::other)?
        .compression();
    let zstd =
        method == zip::CompressionMethod::ZSTD || method == zip::CompressionMethod::ZSTD_DEPRECATED;
    let ppmd = method == zip::CompressionMethod::PPMD;
    if method != zip::CompressionMethod::LZMA
        && method != zip::CompressionMethod::XZ
        && !zstd
        && !ppmd
    {
        return Ok(Member {
            inner: Inner::Standard(archive.by_index(index).map_err(io::Error::other)?),
        });
    }
    let mut raw = archive.by_index_raw(index).map_err(io::Error::other)?;
    let codec = if method == zip::CompressionMethod::LZMA {
        "LZMA"
    } else if ppmd {
        "PPMd"
    } else if zstd {
        "Zstandard"
    } else {
        "XZ"
    };
    if raw.encrypted() {
        return Err(io::Error::other(format!(
            "Encrypted ZIP {codec} member requires a password"
        )));
    }
    let expected_crc = raw.crc32();
    let size = raw.size();
    let decoder = if ppmd {
        let mut header = [0; 2];
        raw.read_exact(&mut header)?;
        let (order, arena, restoration) = ppmd_parameters(header)?;
        Decoder::Ppmd(Box::new(
            ppmd_rust::Ppmd8Decoder::new(PpmdInput(raw), order, arena, restoration)
                .map_err(io::Error::other)?,
        ))
    } else if zstd {
        let mut decoder = zstd::stream::read::Decoder::new(raw)?;
        // Cap each frame's back-reference window, including concatenated frames.
        // This is a window bound, not a claim about total process memory.
        decoder.window_log_max(26)?;
        Decoder::Zstd(decoder)
    } else {
        let (prefix, stream) = if method == zip::CompressionMethod::LZMA {
            let mut header = [0; 9];
            raw.read_exact(&mut header)?;
            validate_properties(&header)?;
            // Bit 1 promises an end marker; otherwise the declared size suffices.
            let stream_size = if raw.flags().as_u16() & 2 != 0 {
                u64::MAX
            } else {
                size
            };
            let mut alone = [0; 13];
            alone[..5].copy_from_slice(&header[4..9]);
            alone[5..].copy_from_slice(&stream_size.to_le_bytes());
            (
                Cursor::new(alone),
                xz2::stream::Stream::new_lzma_decoder(MAX_DECODER_MEMORY)
                    .map_err(io::Error::other)?,
            )
        } else {
            // XZ is self-framed. Enforce the budget for every block/stream,
            // including later concatenated streams, inside the decoder itself.
            let mut empty = Cursor::new([0; 13]);
            empty.set_position(13);
            (
                empty,
                xz2::stream::Stream::new_stream_decoder(
                    MAX_DECODER_MEMORY,
                    xz2::stream::CONCATENATED,
                )
                .map_err(io::Error::other)?,
            )
        };
        Decoder::LzmaXz(xz2::read::XzDecoder::new_stream(prefix.chain(raw), stream))
    };
    Ok(Member {
        inner: Inner::Bounded(Box::new(Decoded {
            decoder,
            codec,
            crc: crc32fast::Hasher::new(),
            expected_crc,
            size,
            decoded: 0,
            finished: false,
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn ppmd_parameters_bound_model_arena_before_decoder_creation() {
        for (order, mib, restore, valid) in [
            (2, 1, 0, true),
            (16, 64, 1, true),
            (6, 65, 0, false),
            (6, 256, 0, false),
            (1, 1, 0, false),
            (6, 1, 2, false),
            (6, 1, 15, false),
        ] {
            let word = ((order - 1) as u16) | (((mib - 1) as u16) << 4) | ((restore as u16) << 12);
            let result = ppmd_parameters(word.to_le_bytes());
            assert_eq!(result.is_ok(), valid, "{order}/{mib}/{restore}");
            if let Ok((actual_order, bytes, _)) = result {
                assert_eq!(actual_order, order);
                assert_eq!(bytes, mib * 1024 * 1024);
            }
        }
    }
    #[test]
    fn lzma_cached_properties_cannot_be_replaced_before_first_decode() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("normal.zipx");
        std::fs::write(
            &path,
            include_bytes!("../docs/qualification/zip-lzma-2026-11-03/fixtures/normal.zipx"),
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let offset = archive.by_index_raw(0).unwrap().data_start().unwrap();
        let mut member = open(&mut archive, 0).unwrap();
        let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        writer.seek(std::io::SeekFrom::Start(offset + 5)).unwrap();
        writer.write_all(&u32::MAX.to_le_bytes()).unwrap();
        writer.flush().unwrap();
        let mut result = Vec::new();
        member.read_to_end(&mut result).unwrap();
        assert_eq!(
            result,
            include_bytes!("../docs/qualification/zip-lzma-2026-11-03/fixtures/expected.txt")
        );
    }
    #[test]
    fn lzma_properties_reject_oversized_dictionary_and_invalid_property_bytes() {
        let mut header = [9, 4, 5, 0, 93, 0, 0, 128, 0];
        assert!(validate_properties(&header).is_ok());
        header[5..].copy_from_slice(&((MAX_DECODER_MEMORY as u32) + 1).to_le_bytes());
        assert!(
            validate_properties(&header)
                .unwrap_err()
                .to_string()
                .contains("64 MiB")
        );
        header[5..].copy_from_slice(&4096u32.to_le_bytes());
        header[4] = 225;
        assert!(validate_properties(&header).is_err());
        header[4] = 93;
        header[2] = 4;
        assert!(validate_properties(&header).is_err());
    }
}
