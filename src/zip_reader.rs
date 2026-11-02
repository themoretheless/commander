//! ZIP member decoding with a bounded LZMA dictionary and verified payloads.
//! Other codecs retain zip-rs decoding. Never reread an untrusted LZMA header
//! after validating it: the exact properties consumed here feed liblzma.
use std::io::{self, Cursor, Read, Seek};

const MAX_LZMA_MEMORY: u64 = 64 * 1024 * 1024;
type LzmaInput<'a, R> = std::io::Chain<Cursor<[u8; 13]>, zip::read::ZipFile<'a, R>>;
struct Lzma<'a, R: Read> {
    decoder: xz2::read::XzDecoder<LzmaInput<'a, R>>,
    crc: crc32fast::Hasher,
    expected_crc: u32,
    size: u64,
    decoded: u64,
    finished: bool,
}
enum Inner<'a, R: Read> {
    Standard(zip::read::ZipFile<'a, R>),
    Lzma(Box<Lzma<'a, R>>),
}
pub struct Member<'a, R: Read> {
    inner: Inner<'a, R>,
}
impl<'a, R: Read> std::ops::Deref for Member<'a, R> {
    type Target = zip::read::ZipFile<'a, R>;
    fn deref(&self) -> &Self::Target {
        match &self.inner {
            Inner::Standard(entry) => entry,
            Inner::Lzma(member) => member.decoder.get_ref().get_ref().1,
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
            Inner::Lzma(member) => {
                if member.finished {
                    return Ok(0);
                }
                let count = member.decoder.read(bytes)?;
                member.decoded = member
                    .decoded
                    .checked_add(count as u64)
                    .ok_or_else(|| io::Error::other("LZMA decoded size overflow"))?;
                if member.decoded > member.size {
                    return Err(io::Error::other("LZMA member exceeds declared size"));
                }
                member.crc.update(&bytes[..count]);
                if count == 0 {
                    if member.decoded != member.size
                        || member.crc.clone().finalize() != member.expected_crc
                    {
                        return Err(io::Error::other("LZMA member size or CRC mismatch"));
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
    if u64::from(dictionary) > MAX_LZMA_MEMORY {
        return Err(io::Error::other(
            "ZIP LZMA dictionary exceeds 64 MiB decoder memory limit",
        ));
    }
    Ok(())
}
/// The decoder enforces the full memory budget, including probability tables;
/// header inspection rejects excessive dictionaries before decoder construction.
pub fn open<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
) -> io::Result<Member<'_, R>> {
    let lzma = archive
        .by_index_data(index)
        .map_err(io::Error::other)?
        .compression()
        == zip::CompressionMethod::LZMA;
    if !lzma {
        return Ok(Member {
            inner: Inner::Standard(archive.by_index(index).map_err(io::Error::other)?),
        });
    }
    let mut raw = archive.by_index_raw(index).map_err(io::Error::other)?;
    if raw.encrypted() {
        return Err(io::Error::other(
            "Encrypted ZIP LZMA member requires a password",
        ));
    }
    let mut header = [0; 9];
    raw.read_exact(&mut header)?;
    validate_properties(&header)?;
    let expected_crc = raw.crc32();
    let size = raw.size();
    // Bit 1 promises an end marker. Otherwise LZMA may end at the declared size.
    let stream_size = if raw.flags().as_u16() & 2 != 0 {
        u64::MAX
    } else {
        size
    };
    let mut alone = [0; 13];
    alone[..5].copy_from_slice(&header[4..9]);
    alone[5..].copy_from_slice(&stream_size.to_le_bytes());
    let stream =
        xz2::stream::Stream::new_lzma_decoder(MAX_LZMA_MEMORY).map_err(io::Error::other)?;
    let decoder = xz2::read::XzDecoder::new_stream(Cursor::new(alone).chain(raw), stream);
    Ok(Member {
        inner: Inner::Lzma(Box::new(Lzma {
            decoder,
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
        header[5..].copy_from_slice(&((MAX_LZMA_MEMORY as u32) + 1).to_le_bytes());
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
