//! ZIP LZMA/XZ/Zstandard/PPMd decoding with bounded decoder memory and verified payloads.
//! Other codecs retain zip-rs decoding. Never reread an untrusted LZMA header
//! after validating it: the exact properties consumed here feed liblzma.
use std::io::{self, Cursor, Read, Seek};

mod aes;
mod zipcrypto;

const MAX_DECODER_MEMORY: u64 = 64 * 1024 * 1024;
type BoundedInput<'a, R> = std::io::Chain<Cursor<[u8; 13]>, CompressedInput<'a, R>>;

/// Keeps original metadata alongside an optionally decrypted compressed stream.
/// The cryptographic header is consumed before any codec reads its properties.
struct CompressedInput<'a, R: Read> {
    entry: zip::read::ZipFile<'a, R>,
    crypto: Crypto,
    check_crc: bool,
}
enum Crypto {
    Plain,
    Legacy(zipcrypto::Keys),
    Aes(Box<aes::State>),
}
impl<'a, R: Read> CompressedInput<'a, R> {
    fn new(mut entry: zip::read::ZipFile<'a, R>, password: Option<&[u8]>) -> io::Result<Self> {
        let aes = aes::settings(&entry)?;
        if aes.is_some() && !entry.encrypted() {
            return Err(io::Error::other("ZIP AES member has no encryption flag"));
        }
        let crypto = if entry.encrypted() {
            let password = password
                .ok_or_else(|| io::Error::other("Encrypted ZIP member requires a password"))?;
            if entry.flags().as_u16() & (1 << 6) != 0 {
                return Err(io::Error::other("ZIP strong encryption is not supported"));
            }
            if let Some(settings) = aes {
                let total = entry.compressed_size();
                return Ok(Self {
                    crypto: Crypto::Aes(Box::new(aes::State::new(
                        &mut entry, password, total, settings,
                    )?)),
                    entry,
                    check_crc: settings.check_crc,
                });
            }
            let verifier = if entry.flags().as_u16() & 8 != 0 {
                (entry
                    .last_modified()
                    .ok_or_else(|| {
                        io::Error::other("Encrypted ZIP member has no valid DOS timestamp")
                    })?
                    .timepart()
                    >> 8) as u8
            } else {
                (entry.crc32() >> 24) as u8
            };
            let mut keys = zipcrypto::Keys::new(password);
            let mut header = [0; 12];
            entry.read_exact(&mut header)?;
            keys.decrypt(&mut header);
            if header[11] != verifier {
                return Err(io::Error::other("Invalid ZIP password"));
            }
            Crypto::Legacy(keys)
        } else {
            Crypto::Plain
        };
        Ok(Self {
            entry,
            crypto,
            check_crc: true,
        })
    }
}
impl<R: Read> CompressedInput<'_, R> {
    fn finish(&mut self) -> io::Result<()> {
        if let Crypto::Aes(state) = &mut self.crypto {
            state.finish(&mut self.entry)?;
        }
        Ok(())
    }
}
impl<'a, R: Read> std::ops::Deref for CompressedInput<'a, R> {
    type Target = zip::read::ZipFile<'a, R>;
    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}
impl<R: Read> Read for CompressedInput<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Crypto::Aes(state) = &mut self.crypto {
            return state.read(&mut self.entry, bytes);
        }
        let count = self.entry.read(bytes)?;
        if let Crypto::Legacy(keys) = &mut self.crypto {
            keys.decrypt(&mut bytes[..count]);
        }
        Ok(count)
    }
}
// ppmd-rust treats UnexpectedEof as a successful partial read. ZIP PPMd
// requires its end marker: preserve premature input exhaustion as an error.
struct PpmdInput<'a, R: Read>(CompressedInput<'a, R>);
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
    Zstd(zstd::stream::read::Decoder<'static, std::io::BufReader<CompressedInput<'a, R>>>),
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
    fn finish(&mut self) -> io::Result<()> {
        match self {
            Self::Ppmd(d) => d.get_mut().0.finish(),
            Self::LzmaXz(d) => d.get_mut().get_mut().1.finish(),
            Self::Zstd(d) => d.get_mut().get_mut().finish(),
        }
    }
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
    expected_crc: Option<u32>,
    size: u64,
    decoded: u64,
    finished: bool,
}
struct Standard<'a, R: Read> {
    entry: zip::read::ZipFile<'a, R>,
    decoded: u64,
}
enum Inner<'a, R: Read> {
    Standard(Standard<'a, R>),
    Bounded(Box<Decoded<'a, R>>),
}
pub struct Member<'a, R: Read> {
    inner: Inner<'a, R>,
}
impl<'a, R: Read> std::ops::Deref for Member<'a, R> {
    type Target = zip::read::ZipFile<'a, R>;
    fn deref(&self) -> &Self::Target {
        match &self.inner {
            Inner::Standard(member) => &member.entry,
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
            Inner::Standard(member) => {
                let count = member.entry.read(bytes)?;
                member.decoded = member
                    .decoded
                    .checked_add(count as u64)
                    .ok_or_else(|| io::Error::other("ZIP decoded size overflow"))?;
                let size = member.entry.size();
                if member.decoded > size || (count == 0 && member.decoded != size) {
                    return Err(io::Error::other("ZIP member size mismatch"));
                }
                Ok(count)
            }
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
                    member.decoder.finish()?;
                    if member.decoded != member.size
                        || member
                            .expected_crc
                            .is_some_and(|expected| member.crc.clone().finalize() != expected)
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
#[cfg(test)]
pub fn open<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
) -> io::Result<Member<'_, R>> {
    open_with_password(archive, index, None)
}

/// Password bytes are borrowed only while initializing the reader. They are
/// never stored in archive metadata or copied into diagnostics.
pub fn open_with_password<'a, R: Read + Seek>(
    archive: &'a mut zip::ZipArchive<R>,
    index: usize,
    password: Option<&[u8]>,
) -> io::Result<Member<'a, R>> {
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
            inner: Inner::Standard(Standard {
                entry: archive
                    .by_index_with_options(
                        index,
                        zip::read::ZipReadOptions::new().password(password),
                    )
                    .map_err(io::Error::other)?,
                decoded: 0,
            }),
        });
    }
    let raw = archive.by_index_raw(index).map_err(io::Error::other)?;
    let codec = if method == zip::CompressionMethod::LZMA {
        "LZMA"
    } else if ppmd {
        "PPMd"
    } else if zstd {
        "Zstandard"
    } else {
        "XZ"
    };
    let mut raw = CompressedInput::new(raw, password)?;
    let expected_crc = raw.check_crc.then(|| raw.crc32());
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

/// Declared central-directory encryption; does not authenticate payloads.
pub fn encryption_label<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> String {
    if entry.flags().as_u16() & 0x40 != 0 {
        return "Unknown (strong-encryption flag)".into();
    }
    match aes::settings(entry) {
        Ok(Some(settings)) if entry.encrypted() => settings.label(),
        Ok(Some(_)) => "Inconsistent (AES metadata without encrypted flag)".into(),
        Ok(None) if entry.encrypted() => "ZipCrypto".into(),
        Ok(None) => "None".into(),
        Err(_) => "Unknown (invalid AES metadata)".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    macro_rules! fixture {
        ($name:literal, $extension:literal) => {
            include_bytes!(concat!(
                "../",
                "docs/qualification/zip-password-2026-11-03/fixtures/",
                $name,
                $extension
            ))
            .as_slice()
        };
    }
    #[test]
    fn aes_custom_codec_memory_limits_still_apply_before_allocation() {
        for (name, bytes, message) in [
            ("lzma", fixture!("aes-lzma-memory", ".zipx"), "memory"),
            ("xz", fixture!("aes-xz-memory", ".zipx"), "memory"),
            ("zstd", fixture!("aes-zstd-memory", ".zipx"), "64 MiB"),
            ("ppmd", fixture!("aes-ppmd-memory", ".zipx"), "64 MiB"),
        ] {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let error = open_with_password(&mut archive, 0, Some(b"fixture-password"))
                .and_then(|mut member| io::copy(&mut member, &mut io::sink()))
                .expect_err(name)
                .to_string();
            assert!(error.contains(message), "{name}: {error}");
        }
    }
    #[test]
    fn aes_custom_codecs_all_strengths_versions_and_chunk_boundaries() {
        for (name, bytes, expected) in [
            (
                "aes-lzma-1-v1",
                fixture!("aes-lzma-1-v1", ".zipx"),
                fixture!("aes-lzma-1-v1", ".txt"),
            ),
            (
                "aes-lzma-1-v2",
                fixture!("aes-lzma-1-v2", ".zipx"),
                fixture!("aes-lzma-1-v2", ".txt"),
            ),
            (
                "aes-lzma-2-v1",
                fixture!("aes-lzma-2-v1", ".zipx"),
                fixture!("aes-lzma-2-v1", ".txt"),
            ),
            (
                "aes-lzma-2-v2",
                fixture!("aes-lzma-2-v2", ".zipx"),
                fixture!("aes-lzma-2-v2", ".txt"),
            ),
            (
                "aes-lzma-3-v1",
                fixture!("aes-lzma-3-v1", ".zipx"),
                fixture!("aes-lzma-3-v1", ".txt"),
            ),
            (
                "aes-lzma-3-v2",
                fixture!("aes-lzma-3-v2", ".zipx"),
                fixture!("aes-lzma-3-v2", ".txt"),
            ),
            (
                "aes-xz-1-v1",
                fixture!("aes-xz-1-v1", ".zipx"),
                fixture!("aes-xz-1-v1", ".txt"),
            ),
            (
                "aes-xz-1-v2",
                fixture!("aes-xz-1-v2", ".zipx"),
                fixture!("aes-xz-1-v2", ".txt"),
            ),
            (
                "aes-xz-2-v1",
                fixture!("aes-xz-2-v1", ".zipx"),
                fixture!("aes-xz-2-v1", ".txt"),
            ),
            (
                "aes-xz-2-v2",
                fixture!("aes-xz-2-v2", ".zipx"),
                fixture!("aes-xz-2-v2", ".txt"),
            ),
            (
                "aes-xz-3-v1",
                fixture!("aes-xz-3-v1", ".zipx"),
                fixture!("aes-xz-3-v1", ".txt"),
            ),
            (
                "aes-xz-3-v2",
                fixture!("aes-xz-3-v2", ".zipx"),
                fixture!("aes-xz-3-v2", ".txt"),
            ),
            (
                "aes-zstd-1-v1",
                fixture!("aes-zstd-1-v1", ".zipx"),
                fixture!("aes-zstd-1-v1", ".txt"),
            ),
            (
                "aes-zstd-1-v2",
                fixture!("aes-zstd-1-v2", ".zipx"),
                fixture!("aes-zstd-1-v2", ".txt"),
            ),
            (
                "aes-zstd-2-v1",
                fixture!("aes-zstd-2-v1", ".zipx"),
                fixture!("aes-zstd-2-v1", ".txt"),
            ),
            (
                "aes-zstd-2-v2",
                fixture!("aes-zstd-2-v2", ".zipx"),
                fixture!("aes-zstd-2-v2", ".txt"),
            ),
            (
                "aes-zstd-3-v1",
                fixture!("aes-zstd-3-v1", ".zipx"),
                fixture!("aes-zstd-3-v1", ".txt"),
            ),
            (
                "aes-zstd-3-v2",
                fixture!("aes-zstd-3-v2", ".zipx"),
                fixture!("aes-zstd-3-v2", ".txt"),
            ),
            (
                "aes-legacy-1-v1",
                fixture!("aes-legacy-1-v1", ".zipx"),
                fixture!("aes-legacy-1-v1", ".txt"),
            ),
            (
                "aes-legacy-1-v2",
                fixture!("aes-legacy-1-v2", ".zipx"),
                fixture!("aes-legacy-1-v2", ".txt"),
            ),
            (
                "aes-legacy-2-v1",
                fixture!("aes-legacy-2-v1", ".zipx"),
                fixture!("aes-legacy-2-v1", ".txt"),
            ),
            (
                "aes-legacy-2-v2",
                fixture!("aes-legacy-2-v2", ".zipx"),
                fixture!("aes-legacy-2-v2", ".txt"),
            ),
            (
                "aes-legacy-3-v1",
                fixture!("aes-legacy-3-v1", ".zipx"),
                fixture!("aes-legacy-3-v1", ".txt"),
            ),
            (
                "aes-legacy-3-v2",
                fixture!("aes-legacy-3-v2", ".zipx"),
                fixture!("aes-legacy-3-v2", ".txt"),
            ),
            (
                "aes-ppmd-1-v1",
                fixture!("aes-ppmd-1-v1", ".zipx"),
                fixture!("aes-ppmd-1-v1", ".txt"),
            ),
            (
                "aes-ppmd-1-v2",
                fixture!("aes-ppmd-1-v2", ".zipx"),
                fixture!("aes-ppmd-1-v2", ".txt"),
            ),
            (
                "aes-ppmd-2-v1",
                fixture!("aes-ppmd-2-v1", ".zipx"),
                fixture!("aes-ppmd-2-v1", ".txt"),
            ),
            (
                "aes-ppmd-2-v2",
                fixture!("aes-ppmd-2-v2", ".zipx"),
                fixture!("aes-ppmd-2-v2", ".txt"),
            ),
            (
                "aes-ppmd-3-v1",
                fixture!("aes-ppmd-3-v1", ".zipx"),
                fixture!("aes-ppmd-3-v1", ".txt"),
            ),
            (
                "aes-ppmd-3-v2",
                fixture!("aes-ppmd-3-v2", ".zipx"),
                fixture!("aes-ppmd-3-v2", ".txt"),
            ),
            (
                "aes-lzma-empty",
                fixture!("aes-lzma-empty", ".zipx"),
                fixture!("aes-lzma-empty", ".txt"),
            ),
            (
                "aes-xz-empty",
                fixture!("aes-xz-empty", ".zipx"),
                fixture!("aes-xz-empty", ".txt"),
            ),
            (
                "aes-ppmd-empty",
                fixture!("aes-ppmd-empty", ".zipx"),
                fixture!("aes-ppmd-empty", ".txt"),
            ),
            (
                "aes-descriptor",
                fixture!("aes-descriptor", ".zipx"),
                fixture!("aes-descriptor", ".txt"),
            ),
            (
                "aes-ae2-nonzero-crc",
                fixture!("aes-ae2-nonzero-crc", ".zipx"),
                fixture!("aes-ae2-nonzero-crc", ".txt"),
            ),
            (
                "aes-ppmd-tail",
                fixture!("aes-ppmd-tail", ".zipx"),
                fixture!("aes-ppmd-tail", ".txt"),
            ),
        ] {
            for password in [None, Some(b"wrong-password".as_slice())] {
                let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
                assert!(
                    open_with_password(&mut archive, 0, password).is_err(),
                    "{name}"
                );
            }
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut member =
                open_with_password(&mut archive, 0, Some(b"fixture-password")).unwrap();
            assert!(member.encrypted());
            assert_eq!(member.read(&mut []).unwrap(), 0);
            let mut decoded = Vec::new();
            let mut round = 0;
            loop {
                let mut buffer = [0; 4096];
                let length = [1, 17, 3, 4096][round % 4];
                round += 1;
                let count = member
                    .read(&mut buffer[..length])
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                if count == 0 {
                    break;
                }
                decoded.extend_from_slice(&buffer[..count]);
            }
            assert_eq!(decoded, expected, "{name}");
        }
    }
    #[test]
    fn aes_authentication_size_crc_and_late_eos_fail_closed() {
        for (name, bytes, message) in [
            (
                "aes-lzma-bad-tag",
                fixture!("aes-lzma-bad-tag", ".zipx"),
                "authentication",
            ),
            (
                "aes-xz-bad-tag",
                fixture!("aes-xz-bad-tag", ".zipx"),
                "authentication",
            ),
            (
                "aes-zstd-bad-tag",
                fixture!("aes-zstd-bad-tag", ".zipx"),
                "authentication",
            ),
            (
                "aes-legacy-bad-tag",
                fixture!("aes-legacy-bad-tag", ".zipx"),
                "authentication",
            ),
            (
                "aes-ppmd-bad-tag",
                fixture!("aes-ppmd-bad-tag", ".zipx"),
                "authentication",
            ),
            (
                "aes-ppmd-tail-bad-tag",
                fixture!("aes-ppmd-tail-bad-tag", ".zipx"),
                "authentication",
            ),
            (
                "aes-ae1-bad-crc",
                fixture!("aes-ae1-bad-crc", ".zipx"),
                "CRC",
            ),
            ("aes-bad-size", fixture!("aes-bad-size", ".zipx"), "size"),
            (
                "aes-short-header",
                fixture!("aes-short-header", ".zipx"),
                "",
            ),
        ] {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let result = open_with_password(&mut archive, 0, Some(b"fixture-password")).and_then(
                |mut member| {
                    let error = io::copy(&mut member, &mut io::sink()).expect_err(name);
                    if message == "authentication" {
                        for _ in 0..2 {
                            assert!(
                                member.read(&mut [0; 1]).is_err(),
                                "{name} became EOF after authentication failure"
                            );
                        }
                    }
                    Err::<(), _>(error)
                },
            );
            let error = result.unwrap_err().to_string();
            assert!(error.contains(message), "{name}: {error}");
            assert!(!error.contains("fixture-password"));
        }
        // Reading exactly the declared plaintext is insufficient to authenticate
        // a compressed entry: the next EOF read must still reject the bad tag.
        let mut archive =
            zip::ZipArchive::new(Cursor::new(fixture!("aes-ppmd-tail-bad-tag", ".zipx"))).unwrap();
        let mut member = open_with_password(&mut archive, 0, Some(b"fixture-password")).unwrap();
        let expected = fixture!("aes-ppmd-tail-bad-tag", ".txt");
        let mut decoded = vec![0; expected.len()];
        member.read_exact(&mut decoded).unwrap();
        assert_eq!(decoded, expected);
        assert!(
            member
                .read(&mut [0; 1])
                .unwrap_err()
                .to_string()
                .contains("authentication")
        );
    }
    #[test]
    fn aes_utf8_password_bytes_are_preserved() {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(fixture!("aes-utf8-password", ".zipx"))).unwrap();
        let mut member =
            open_with_password(&mut archive, 0, Some("fixture-пароль-ёж".as_bytes())).unwrap();
        let mut decoded = Vec::new();
        member.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, fixture!("aes-utf8-password", ".txt"));
    }
    #[test]
    fn password_custom_codecs_keep_metadata_and_decode_exact_bytes() {
        for (name, bytes, expected) in [
            ("lzma", fixture!("lzma", ".zipx"), fixture!("lzma", ".txt")),
            (
                "lzma-descriptor",
                fixture!("lzma-descriptor", ".zipx"),
                fixture!("lzma-descriptor", ".txt"),
            ),
            ("xz", fixture!("xz", ".zipx"), fixture!("xz", ".txt")),
            ("zstd", fixture!("zstd", ".zipx"), fixture!("zstd", ".txt")),
            (
                "zstd-legacy",
                fixture!("zstd-legacy", ".zipx"),
                fixture!("zstd-legacy", ".txt"),
            ),
            ("ppmd", fixture!("ppmd", ".zipx"), fixture!("ppmd", ".txt")),
        ] {
            for password in [None, Some(b"wrong-password".as_slice())] {
                let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
                assert!(
                    open_with_password(&mut archive, 0, password).is_err(),
                    "{name}"
                );
            }
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut member =
                open_with_password(&mut archive, 0, Some(b"fixture-password")).unwrap();
            assert!(member.encrypted());
            assert_eq!(member.name().unwrap(), "payload.txt");
            assert_eq!(member.read(&mut []).unwrap(), 0);
            let mut decoded = Vec::new();
            member.read_to_end(&mut decoded).unwrap();
            assert_eq!(decoded, expected, "{name}");
            assert_eq!(member.read(&mut [0; 3]).unwrap(), 0);
        }
    }
    #[test]
    fn password_custom_codecs_preserve_memory_and_payload_validation() {
        for (name, bytes, error) in [
            ("lzma-memory", fixture!("lzma-memory", ".zipx"), "memory"),
            ("xz-memory", fixture!("xz-memory", ".zipx"), "memory"),
            ("ppmd-memory", fixture!("ppmd-memory", ".zipx"), "64 MiB"),
            ("ppmd-no-eos", fixture!("ppmd-no-eos", ".zipx"), ""),
            ("ppmd-bad-crc", fixture!("ppmd-bad-crc", ".zipx"), "CRC"),
            ("ppmd-bad-size", fixture!("ppmd-bad-size", ".zipx"), "size"),
            ("xz-truncated", fixture!("xz-truncated", ".zipx"), ""),
            ("xz-short-header", fixture!("xz-short-header", ".zipx"), ""),
        ] {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let result = open_with_password(&mut archive, 0, Some(b"fixture-password"))
                .and_then(|mut member| io::copy(&mut member, &mut io::sink()));
            let message = result.expect_err(name).to_string();
            assert!(message.contains(error), "{name}: {message}");
            assert!(!message.contains("fixture-password"));
        }
    }
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
