//! Fresh, cancellable checksum calculation and explicit expected-digest comparison.
use blake2::{Blake2b512, Blake2s256, Digest as _};
use sha2::{Digest, Sha256, Sha384, Sha512};
use sha3::{Sha3_256, Sha3_512};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumRow {
    pub path: PathBuf,
    pub content_hash: Option<u64>,
    pub blake3_hex: Option<String>,
    pub sha256_hex: Option<String>,
    pub sha384_hex: Option<String>,
    pub sha512_hex: Option<String>,
    pub sha3_256_hex: Option<String>,
    pub sha3_512_hex: Option<String>,
    pub blake2s_hex: Option<String>,
    pub blake2b_hex: Option<String>,
    pub crc32_hex: Option<String>,
    pub md5_hex: Option<String>,
    pub sha1_hex: Option<String>,
    pub adler32_hex: Option<String>,
    pub error: Option<String>,
    pub skipped: Option<String>,
}
impl ChecksumRow {
    pub fn display_digests(&self) -> String {
        let width = Algorithm::ALL
            .iter()
            .map(|algorithm| algorithm.label().len())
            .max()
            .unwrap_or(8);
        let mut lines = Algorithm::ALL
            .into_iter()
            .map(|algorithm| {
                format!(
                    "{:<width$} {}",
                    algorithm.label(),
                    algorithm.digest(self).unwrap_or("-")
                )
            })
            .collect::<Vec<_>>();
        lines.push(format!(
            "{:<width$} {}",
            "Content",
            self.content_hash
                .map(|value| format!("{value:016x}"))
                .unwrap_or_else(|| "-".into())
        ));
        lines.join("\n")
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumReport {
    pub rows: Vec<ChecksumRow>,
    pub cancelled: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpectedMatch {
    Empty,
    Invalid,
    Unavailable,
    Match,
    Mismatch,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Algorithm {
    #[default]
    Sha256,
    Sha384,
    Sha512,
    Sha3_256,
    Sha3_512,
    Blake2s,
    Blake2b,
    Blake3,
    Crc32,
    Md5,
    Sha1,
    Adler32,
}
impl Algorithm {
    pub const ALL: [Self; 12] = [
        Self::Sha256,
        Self::Sha384,
        Self::Sha512,
        Self::Sha3_256,
        Self::Sha3_512,
        Self::Blake2s,
        Self::Blake2b,
        Self::Blake3,
        Self::Crc32,
        Self::Md5,
        Self::Sha1,
        Self::Adler32,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Sha256 => "SHA-256",
            Self::Sha384 => "SHA-384",
            Self::Sha512 => "SHA-512",
            Self::Sha3_256 => "SHA3-256",
            Self::Sha3_512 => "SHA3-512",
            Self::Blake2s => "BLAKE2s-256",
            Self::Blake2b => "BLAKE2b-512",
            Self::Blake3 => "BLAKE3",
            Self::Crc32 => "CRC32",
            Self::Md5 => "MD5",
            Self::Sha1 => "SHA-1",
            Self::Adler32 => "Adler-32",
        }
    }
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Sha256 | Self::Blake3 | Self::Sha3_256 | Self::Blake2s => 64,
            Self::Sha384 => 96,
            Self::Sha512 | Self::Sha3_512 | Self::Blake2b => 128,
            Self::Crc32 | Self::Adler32 => 8,
            Self::Md5 => 32,
            Self::Sha1 => 40,
        }
    }
    pub fn digest(self, row: &ChecksumRow) -> Option<&str> {
        let value = match self {
            Self::Sha256 => row.sha256_hex.as_deref(),
            Self::Sha384 => row.sha384_hex.as_deref(),
            Self::Sha512 => row.sha512_hex.as_deref(),
            Self::Sha3_256 => row.sha3_256_hex.as_deref(),
            Self::Sha3_512 => row.sha3_512_hex.as_deref(),
            Self::Blake2s => row.blake2s_hex.as_deref(),
            Self::Blake2b => row.blake2b_hex.as_deref(),
            Self::Blake3 => row.blake3_hex.as_deref(),
            Self::Crc32 => row.crc32_hex.as_deref(),
            Self::Md5 => row.md5_hex.as_deref(),
            Self::Sha1 => row.sha1_hex.as_deref(),
            Self::Adler32 => row.adler32_hex.as_deref(),
        };
        value.filter(|value| valid_digest(value, self.hex_len()))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DigestEncoding {
    #[default]
    Hex,
    Base64,
}
impl DigestEncoding {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hex => "Hex",
            Self::Base64 => "Base64",
        }
    }
}
impl ChecksumReport {
    /// A single completed, valid digest; never guess an algorithm from its length.
    pub fn single_digest(&self, algorithm: Algorithm) -> Option<&str> {
        if self.cancelled || self.rows.len() != 1 {
            return None;
        }
        let row = &self.rows[0];
        if row.error.is_some() || row.skipped.is_some() {
            return None;
        }
        algorithm.digest(row)
    }
    /// Standard padded Base64 of the digest bytes in displayed hex order.
    /// The same single-result/completion checks apply as hexadecimal copying.
    pub fn single_digest_base64(&self, algorithm: Algorithm) -> Option<String> {
        use base64::Engine;
        let hex = self.single_digest(algorithm)?;
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for offset in (0..hex.len()).step_by(2) {
            bytes.push(u8::from_str_radix(&hex[offset..offset + 2], 16).ok()?);
        }
        Some(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
    pub fn successful(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.error.is_none() && row.skipped.is_none() && row.sha256_hex.is_some())
            .count()
    }
    pub fn summary_line(&self) -> String {
        let skipped = self.rows.iter().filter(|row| row.skipped.is_some()).count();
        let failed = self.rows.iter().filter(|row| row.error.is_some()).count();
        let ok = self.successful();
        let mut line = format!("Calculated {ok} file(s)");
        if failed > 0 {
            line.push_str(&format!(", {failed} failed"));
        }
        if skipped > 0 {
            line.push_str(&format!(", {skipped} skipped"));
        }
        let unavailable = self.rows.len().saturating_sub(ok + skipped + failed);
        if unavailable > 0 {
            line.push_str(&format!(", {unavailable} unavailable"));
        }
        if self.cancelled {
            line.push_str(", cancelled");
        }
        line
    }
    pub fn detailed_text(&self) -> String {
        self.rows
            .iter()
            .map(|row| {
                let path = row.path.to_string_lossy().escape_debug().to_string();
                if let Some(error) = &row.error {
                    return format!("{path}: ERROR {error}");
                }
                if let Some(reason) = &row.skipped {
                    return format!("{path}: skipped ({reason})");
                }
                format!(
                    "{path}: content={} blake3={} sha256={} sha384={} sha512={} sha3_256={} sha3_512={} blake2s={} blake2b={} crc32={} md5={} sha1={} adler32={}",
                    row.content_hash
                        .map(|hash| format!("{hash:016x}"))
                        .unwrap_or_else(|| "-".into()),
                    row.blake3_hex.as_deref().unwrap_or("-"),
                    row.sha256_hex.as_deref().unwrap_or("-"),
                    row.sha384_hex.as_deref().unwrap_or("-"),
                    row.sha512_hex.as_deref().unwrap_or("-"),
                    row.sha3_256_hex.as_deref().unwrap_or("-"),
                    row.sha3_512_hex.as_deref().unwrap_or("-"),
                    row.blake2s_hex.as_deref().unwrap_or("-"),
                    row.blake2b_hex.as_deref().unwrap_or("-"),
                    row.crc32_hex.as_deref().unwrap_or("-"),
                    row.md5_hex.as_deref().unwrap_or("-"),
                    row.sha1_hex.as_deref().unwrap_or("-"),
                    row.adler32_hex.as_deref().unwrap_or("-")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[cfg(test)]
    pub fn compare_expected(&self, expected: &str) -> ExpectedMatch {
        self.compare_expected_algorithm(expected, Algorithm::Sha256)
    }
    pub fn compare_expected_algorithm(
        &self,
        expected: &str,
        algorithm: Algorithm,
    ) -> ExpectedMatch {
        let expected = expected.trim();
        if expected.is_empty() {
            return ExpectedMatch::Empty;
        }
        if !valid_digest(expected, algorithm.hex_len()) {
            return ExpectedMatch::Invalid;
        }
        let Some(actual) = self.single_digest(algorithm) else {
            return ExpectedMatch::Unavailable;
        };
        if actual.eq_ignore_ascii_case(expected) {
            ExpectedMatch::Match
        } else {
            ExpectedMatch::Mismatch
        }
    }
    pub fn compare_expected_encoded(
        &self,
        expected: &str,
        algorithm: Algorithm,
        encoding: DigestEncoding,
    ) -> ExpectedMatch {
        if encoding == DigestEncoding::Hex {
            return self.compare_expected_algorithm(expected, algorithm);
        }
        use base64::Engine;
        let expected = expected.trim();
        if expected.is_empty() {
            return ExpectedMatch::Empty;
        }
        let bytes = algorithm.hex_len() / 2;
        if expected.len() != bytes.div_ceil(3) * 4 {
            return ExpectedMatch::Invalid;
        }
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(expected) else {
            return ExpectedMatch::Invalid;
        };
        if decoded.len() != bytes {
            return ExpectedMatch::Invalid;
        }
        let Some(actual) = self.single_digest_base64(algorithm) else {
            return ExpectedMatch::Unavailable;
        };
        if actual == expected {
            ExpectedMatch::Match
        } else {
            ExpectedMatch::Mismatch
        }
    }
    /// GNU untagged binary-mode SHA format or b3sum two-space format, with filename escaping.
    /// Only successfully completed files are exported; no hashes of skipped rows.
    #[cfg(test)]
    pub fn sha256_list(&self) -> Result<String, String> {
        self.checksum_list(Algorithm::Sha256)
    }

    /// Common directory of the completed records for the explicitly chosen algorithm.
    /// Pure lexical components; no filesystem canonicalization or extra reads.
    pub fn list_base(&self, algorithm: Algorithm) -> Result<PathBuf, String> {
        let mut base: Option<PathBuf> = None;
        for row in &self.rows {
            if row.error.is_some() || row.skipped.is_some() || algorithm.digest(row).is_none() {
                continue;
            }
            validate_relative_source(&row.path)?;
            let parent = row
                .path
                .parent()
                .ok_or("A checksum record has no parent directory")?;
            if let Some(base) = &mut base {
                while !parent.starts_with(base.as_path()) {
                    if !base.pop() {
                        return Err("Checksum records have no common directory".into());
                    }
                }
            } else {
                base = Some(parent.to_path_buf());
            }
        }
        base.ok_or_else(|| format!("No completed {} checksums to copy", algorithm.label()))
    }

    pub fn checksum_list(&self, algorithm: Algorithm) -> Result<String, String> {
        self.checksum_list_at(algorithm, None)
    }

    pub fn checksum_list_at(
        &self,
        algorithm: Algorithm,
        base: Option<&Path>,
    ) -> Result<String, String> {
        if let Some(base) = base
            && (!base.is_absolute()
                || base
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)))
        {
            return Err(
                "Relative checksum lists need an absolute base without parent traversal".into(),
            );
        }
        if algorithm == Algorithm::Crc32 {
            return self.sfv_list(base);
        }
        let mut lines = Vec::new();
        for row in &self.rows {
            if row.error.is_some() || row.skipped.is_some() {
                continue;
            }
            let Some(hash) = algorithm.digest(row) else {
                continue;
            };
            let path = list_path(&row.path, base)?;
            if algorithm == Algorithm::Blake3 && path.contains('�') {
                return Err("b3sum checkfiles cannot represent paths containing the Unicode replacement character".into());
            }
            let escaped = path.contains(['\\', '\n', '\r']);
            let path = path
                .replace('\\', "\\\\")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            // b3sum uses two spaces, not GNU binary mode's space + star.
            let marker = if algorithm == Algorithm::Blake3 {
                " "
            } else {
                "*"
            };
            lines.push(format!(
                "{}{hash} {marker}{path}\n",
                if escaped { "\\" } else { "" }
            ));
        }
        if lines.is_empty() {
            return Err(format!(
                "No completed {} checksums to copy",
                algorithm.label()
            ));
        }
        Ok(lines.concat())
    }
    /// SFV has no filename escape language. Refuse ambiguous names, never rewrite them.
    fn sfv_list(&self, base: Option<&Path>) -> Result<String, String> {
        let mut text = String::new();
        for row in &self.rows {
            if row.error.is_some() || row.skipped.is_some() {
                continue;
            }
            let Some(hash) = Algorithm::Crc32.digest(row) else {
                continue;
            };
            let path = list_path(&row.path, base)?;
            if path.contains(['\0', '\r', '\n', '\\']) {
                return Err("SFV paths cannot contain NUL, line breaks or backslashes".into());
            }
            text.push_str(&format!("{path} {}\n", hash.to_ascii_uppercase()));
        }
        if text.is_empty() {
            return Err("No completed CRC32 checksums to copy".into());
        }
        Ok(text)
    }
}
fn validate_relative_source(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        || path.to_str().is_none_or(|text| text.contains('\0'))
    {
        return Err("Relative checksum records need absolute UTF-8 file paths without NUL or parent traversal".into());
    }
    Ok(())
}

fn list_path(path: &Path, base: Option<&Path>) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or("A path is not UTF-8; it cannot be copied losslessly as a checksum list")?;
    if !path.is_absolute() || text.contains('\0') {
        return Err("Checksum lists require absolute paths without NUL characters".into());
    }
    let Some(base) = base else {
        return Ok(text.to_owned());
    };
    validate_relative_source(path)?;
    let relative = path
        .strip_prefix(base)
        .map_err(|_| "A checksum record is outside the displayed base directory")?;
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return Err("A checksum record has no relative filename".into());
    }
    // Protect literal '-' from stdin handling and ';name' from SFV comment syntax.
    Ok(format!(
        "./{}",
        relative
            .to_str()
            .ok_or("A relative checksum path is not UTF-8")?
    ))
}

fn valid_digest(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
pub fn verify_paths(paths: impl IntoIterator<Item = PathBuf>) -> ChecksumReport {
    ChecksumReport {
        rows: paths
            .into_iter()
            .map(|path| verify_one(&path, &|| false))
            .collect(),
        cancelled: false,
    }
}
pub fn verify_paths_cancellable(
    paths: Vec<PathBuf>,
    cancelled: &crate::workload::CancellationToken,
    completed: &std::sync::atomic::AtomicUsize,
) -> ChecksumReport {
    let mut report = ChecksumReport::default();
    for path in paths {
        if cancelled.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let row = verify_one(&path, &|| cancelled.is_cancelled());
        if cancelled.is_cancelled() {
            report.cancelled = true;
            break;
        }
        report.rows.push(row);
        completed.fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    report
}
pub fn verify_one(path: &Path, cancelled: &dyn Fn() -> bool) -> ChecksumRow {
    let mut row = ChecksumRow {
        path: path.into(),
        ..Default::default()
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) => {
            row.error = Some(error.to_string());
            return row;
        }
    };
    if meta.is_dir() {
        row.skipped = Some("directories have no content checksum".into());
        return row;
    }
    if meta.file_type().is_symlink() {
        row.skipped = Some("symlinks are not hashed".into());
        return row;
    }
    if !meta.is_file() {
        row.skipped = Some("only regular files are hashed".into());
        return row;
    }
    match digests(path, &meta, cancelled) {
        Ok(digests) => {
            row.content_hash = Some(content_hash_of(&digests.blake3));
            row.blake3_hex = Some(blake3::Hash::from(digests.blake3).to_hex().to_string());
            row.sha256_hex = Some(digests.sha256);
            row.sha384_hex = Some(digests.sha384);
            row.sha512_hex = Some(digests.sha512);
            row.sha3_256_hex = Some(digests.sha3_256);
            row.sha3_512_hex = Some(digests.sha3_512);
            row.blake2s_hex = Some(digests.blake2s);
            row.blake2b_hex = Some(digests.blake2b);
            row.crc32_hex = Some(digests.crc32);
            row.md5_hex = Some(digests.md5);
            row.sha1_hex = Some(digests.sha1);
            row.adler32_hex = Some(digests.adler32);
        }
        Err(error) => row.error = Some(error.to_string()),
    }
    row
}
fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "File changed while checksums were calculated; refresh and retry",
    )
}
struct FileDigests {
    blake3: [u8; 32],
    sha256: String,
    sha384: String,
    sha512: String,
    sha3_256: String,
    sha3_512: String,
    blake2s: String,
    blake2b: String,
    crc32: String,
    md5: String,
    sha1: String,
    adler32: String,
}
fn digests(
    path: &Path,
    expected: &std::fs::Metadata,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<FileDigests> {
    use crate::path_identity::PathIdentity;
    let cancel = || {
        if cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Checksum calculation cancelled",
            ))
        } else {
            Ok(())
        }
    };
    cancel()?;
    let before = PathIdentity::from_metadata(path.into(), expected);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || !before.same_shallow_binding(&PathIdentity::from_metadata(path.into(), &opened))
    {
        return Err(changed());
    }
    let mut remaining = opened.len();
    let mut buffer = [0u8; 64 * 1024];
    let mut blake = blake3::Hasher::new();
    let mut sha = Sha256::new();
    let mut sha384 = Sha384::new();
    let mut sha512 = Sha512::new();
    let mut sha3_256 = Sha3_256::new();
    let mut sha3_512 = Sha3_512::new();
    let mut blake2s = Blake2s256::new();
    let mut blake2b = Blake2b512::new();
    let mut crc32 = crc32fast::Hasher::new();
    let mut md5 = md5::Md5::new();
    let mut sha1 = sha1::Sha1::new();
    let mut adler32 = adler2::Adler32::new();
    loop {
        cancel()?;
        let limit = remaining.saturating_add(1).min(buffer.len() as u64) as usize;
        let count = match file.read(&mut buffer[..limit]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        if count as u64 > remaining {
            return Err(changed());
        }
        remaining -= count as u64;
        blake.update(&buffer[..count]);
        sha.update(&buffer[..count]);
        sha384.update(&buffer[..count]);
        sha512.update(&buffer[..count]);
        sha3_256.update(&buffer[..count]);
        sha3_512.update(&buffer[..count]);
        blake2s.update(&buffer[..count]);
        blake2b.update(&buffer[..count]);
        crc32.update(&buffer[..count]);
        md5.update(&buffer[..count]);
        sha1.update(&buffer[..count]);
        adler32.write_slice(&buffer[..count]);
    }
    let after = file.metadata()?;
    if remaining != 0
        || !before.same_shallow_binding(&PathIdentity::from_metadata(path.into(), &after))
        || !before.same_shallow_binding(&PathIdentity::observe(path)?)
    {
        return Err(changed());
    }
    cancel()?;
    let sha = sha.finalize();
    let hex = sha.iter().map(|byte| format!("{byte:02x}")).collect();
    let hex384 = sha384
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let hex512 = sha512
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(FileDigests {
        adler32: format!("{:08x}", adler32.checksum()),
        sha1: sha1
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        blake3: *blake.finalize().as_bytes(),
        blake2s: blake2s
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        blake2b: blake2b
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        sha256: hex,
        sha384: hex384,
        sha512: hex512,
        sha3_256: sha3_256
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        sha3_512: sha3_512
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        crc32: format!("{:08x}", crc32.finalize()),
        md5: md5
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}
fn content_hash_of(blake3: &[u8; 32]) -> u64 {
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&blake3[..8]);
    u64::from_be_bytes(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const ABC_SHA384: &str = "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7";
    const ABC_SHA512: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";

    #[test]
    fn sha3_streams_python_vectors_compares_encodings_and_exports_lists() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../docs/qualification/sha3-checksums-2026-11-03/python-oracle.json"
        ))
        .unwrap();
        let temp = TempDir::new();
        for vector in vectors.as_array().unwrap() {
            let size = vector["size"].as_u64().unwrap() as usize;
            let data = if size == 3 {
                b"abc".to_vec()
            } else {
                (0..size).map(|i| ((i * 37 + 11) % 256) as u8).collect()
            };
            let path = temp.file("data.bin", "");
            std::fs::write(&path, &data).unwrap();
            let report = verify_paths([path]);
            for (algorithm, key, encoded) in [
                (Algorithm::Sha3_256, "sha3_256", "base64_256"),
                (Algorithm::Sha3_512, "sha3_512", "base64_512"),
            ] {
                let expected = vector[key].as_str().unwrap();
                assert_eq!(report.single_digest(algorithm), Some(expected));
                assert!(report.rows[0].display_digests().lines().any(|line| {
                    line.strip_prefix(algorithm.label())
                        .is_some_and(|value| value.trim_start() == expected)
                }));
                assert_eq!(
                    report.single_digest_base64(algorithm).as_deref(),
                    vector[encoded].as_str()
                );
                assert_eq!(
                    report.compare_expected_algorithm(&expected.to_uppercase(), algorithm),
                    ExpectedMatch::Match
                );
                assert_eq!(
                    report.compare_expected_encoded(
                        vector[encoded].as_str().unwrap(),
                        algorithm,
                        DigestEncoding::Base64
                    ),
                    ExpectedMatch::Match
                );
                assert_eq!(
                    report.compare_expected_algorithm(&"0".repeat(algorithm.hex_len()), algorithm),
                    ExpectedMatch::Mismatch
                );
                let list = report
                    .checksum_list_at(algorithm, Some(temp.path()))
                    .unwrap();
                assert!(list.contains(expected));
                let manifest = temp.file("list", &list);
                let checked = crate::checksum_list::verify(
                    &manifest,
                    Some(algorithm),
                    &|| false,
                    &std::sync::atomic::AtomicUsize::new(0),
                    &std::sync::atomic::AtomicUsize::new(0),
                );
                assert_eq!(checked.rows[0].status, crate::checksum_list::Status::Match);
                assert!(
                    report
                        .detailed_text()
                        .contains(&format!("{key}={expected}"))
                );
                let mut cancelled = report.clone();
                cancelled.cancelled = true;
                assert!(cancelled.single_digest(algorithm).is_none());
            }
        }
    }

    #[test]
    fn blake2_streams_python_vectors_compares_encodings_and_exports_lists() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../docs/qualification/blake2-checksums-2026-11-03/python-oracle.json"
        ))
        .unwrap();
        let temp = TempDir::new();
        for vector in vectors.as_array().unwrap() {
            let size = vector["size"].as_u64().unwrap() as usize;
            let data = if size == 3 {
                b"abc".to_vec()
            } else {
                (0..size).map(|i| ((i * 37 + 11) % 256) as u8).collect()
            };
            let path = temp.file("data.bin", "");
            std::fs::write(&path, &data).unwrap();
            let report = verify_paths([path]);
            for (algorithm, key, encoded) in [
                (Algorithm::Blake2s, "blake2s", "base64_256"),
                (Algorithm::Blake2b, "blake2b", "base64_512"),
            ] {
                let expected = vector[key].as_str().unwrap();
                assert_eq!(report.single_digest(algorithm), Some(expected));
                assert!(report.rows[0].display_digests().lines().any(|line| {
                    line.strip_prefix(algorithm.label())
                        .is_some_and(|value| value.trim_start() == expected)
                }));
                assert_eq!(
                    report.single_digest_base64(algorithm).as_deref(),
                    vector[encoded].as_str()
                );
                assert_eq!(
                    report.compare_expected_algorithm(&expected.to_uppercase(), algorithm),
                    ExpectedMatch::Match
                );
                assert_eq!(
                    report.compare_expected_encoded(
                        vector[encoded].as_str().unwrap(),
                        algorithm,
                        DigestEncoding::Base64
                    ),
                    ExpectedMatch::Match
                );
                assert_eq!(
                    report.compare_expected_algorithm(&"0".repeat(algorithm.hex_len()), algorithm),
                    ExpectedMatch::Mismatch
                );
                let list = report
                    .checksum_list_at(algorithm, Some(temp.path()))
                    .unwrap();
                assert!(list.contains(expected));
                let manifest = temp.file("list", &list);
                let checked = crate::checksum_list::verify(
                    &manifest,
                    Some(algorithm),
                    &|| false,
                    &std::sync::atomic::AtomicUsize::new(0),
                    &std::sync::atomic::AtomicUsize::new(0),
                );
                assert_eq!(checked.rows[0].status, crate::checksum_list::Status::Match);
                assert!(
                    report
                        .detailed_text()
                        .contains(&format!("{key}={expected}"))
                );
                let mut cancelled = report.clone();
                cancelled.cancelled = true;
                assert!(cancelled.single_digest(algorithm).is_none());
            }
        }
    }

    #[test]
    fn expected_base64_is_explicit_strict_bounded_and_case_sensitive() {
        let temp = TempDir::new();
        let mut report = verify_paths([temp.file("data", "abc")]);
        assert_eq!(DigestEncoding::default(), DigestEncoding::Hex);
        for algorithm in Algorithm::ALL {
            let encoded = report.single_digest_base64(algorithm).unwrap();
            assert_eq!(
                report.compare_expected_encoded(
                    &format!("  {encoded}\n"),
                    algorithm,
                    DigestEncoding::Base64
                ),
                ExpectedMatch::Match
            );
            let mut changed = encoded.clone().into_bytes();
            changed[0] = if changed[0] == b'A' { b'B' } else { b'A' };
            assert_eq!(
                report.compare_expected_encoded(
                    std::str::from_utf8(&changed).unwrap(),
                    algorithm,
                    DigestEncoding::Base64
                ),
                ExpectedMatch::Mismatch
            );
            assert_eq!(
                report.compare_expected_encoded(
                    report.single_digest(algorithm).unwrap(),
                    algorithm,
                    DigestEncoding::Hex
                ),
                ExpectedMatch::Match
            );
        }
        for invalid in [
            "024d0127",
            "Ak0BJw",
            "Ak0BJw===",
            "Ak0BJx==",
            "Ak0B Jw==",
            "Ak0B\nJw==",
            "____Jw==",
            "éééé",
            &"A".repeat(100000),
        ] {
            assert_eq!(
                report.compare_expected_encoded(
                    invalid,
                    Algorithm::Adler32,
                    DigestEncoding::Base64
                ),
                ExpectedMatch::Invalid,
                "{invalid:.20}"
            );
        }
        assert_eq!(
            report.compare_expected_encoded("ak0BJw==", Algorithm::Adler32, DigestEncoding::Base64),
            ExpectedMatch::Mismatch
        );
        assert_eq!(
            report.compare_expected_encoded("Ak0BJw==", Algorithm::Adler32, DigestEncoding::Hex),
            ExpectedMatch::Invalid
        );
        assert_eq!(
            report.compare_expected_encoded("  ", Algorithm::Adler32, DigestEncoding::Base64),
            ExpectedMatch::Empty
        );
        report.cancelled = true;
        assert_eq!(
            report.compare_expected_encoded("Ak0BJw==", Algorithm::Adler32, DigestEncoding::Base64),
            ExpectedMatch::Unavailable
        );
    }

    #[test]
    fn base64_digest_encodes_bytes_for_every_algorithm_and_refuses_unavailable_results() {
        let temp = TempDir::new();
        let mut report = verify_paths([temp.file("data", "abc")]);
        for (algorithm, expected) in [
            (
                Algorithm::Sha256,
                "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=",
            ),
            (
                Algorithm::Sha384,
                "ywB1P0WjXou1oD1pmsZQBycsMqsO3tFjGotgWkP/W+2AhgcroefMI1i67KE0yCWn",
            ),
            (
                Algorithm::Sha512,
                "3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==",
            ),
            (
                Algorithm::Blake3,
                "ZDezrDhGUTP/tjt1JzqNtUjFWEZdedsD/TWcbNW9nYU=",
            ),
            (Algorithm::Crc32, "NSRBwg=="),
            (Algorithm::Md5, "kAFQmDzST7DWlj99KOF/cg=="),
            (Algorithm::Sha1, "qZk+NkcGgWq6PiVxeFDCbJzQ2J0="),
            (Algorithm::Adler32, "Ak0BJw=="),
        ] {
            assert_eq!(
                report.single_digest_base64(algorithm).as_deref(),
                Some(expected)
            );
        }
        report.rows[0].sha256_hex = report.rows[0].sha256_hex.as_ref().map(|v| v.to_uppercase());
        assert_eq!(
            report.single_digest_base64(Algorithm::Sha256).as_deref(),
            Some("ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=")
        );
        for value in [
            None,
            Some("z".repeat(64)),
            Some("f".repeat(63)),
            Some("é".repeat(32)),
        ] {
            report.rows[0].sha256_hex = value;
            assert!(report.single_digest_base64(Algorithm::Sha256).is_none());
        }
        report.rows[0].sha256_hex = Some("00".repeat(32));
        report.cancelled = true;
        assert!(report.single_digest_base64(Algorithm::Sha256).is_none());
        report.cancelled = false;
        report.rows[0].error = Some("changed".into());
        assert!(report.single_digest_base64(Algorithm::Sha256).is_none());
        report.rows[0].error = None;
        report.rows[0].skipped = Some("symlink".into());
        assert!(report.single_digest_base64(Algorithm::Sha256).is_none());
        report.rows[0].skipped = None;
        report.rows.push(report.rows[0].clone());
        assert!(report.single_digest_base64(Algorithm::Sha256).is_none());
        report.rows.clear();
        assert!(report.single_digest_base64(Algorithm::Sha256).is_none());
    }

    #[test]
    fn adler32_streams_known_vectors_and_block_boundaries_with_exact_export() {
        let temp = TempDir::new();
        for (bytes, expected) in [
            (b"".as_slice(), "00000001"),
            (b"abc".as_slice(), "024d0127"),
            (b"Wikipedia".as_slice(), "11e60398"),
        ] {
            let path = temp.file("data", "");
            std::fs::write(&path, bytes).unwrap();
            let report = verify_paths([path.clone()]);
            assert_eq!(report.single_digest(Algorithm::Adler32), Some(expected));
            assert_eq!(
                report.compare_expected_algorithm(&expected.to_uppercase(), Algorithm::Adler32),
                ExpectedMatch::Match
            );
            assert_eq!(
                report.compare_expected_algorithm("00000000", Algorithm::Adler32),
                ExpectedMatch::Mismatch
            );
            assert!(
                report
                    .detailed_text()
                    .contains(&format!("adler32={expected}"))
            );
            assert_eq!(
                report
                    .checksum_list_at(Algorithm::Adler32, Some(temp.path()))
                    .unwrap(),
                format!("{expected} *./data\n")
            );
        }
        for size in [65535, 65536, 65537, 700001] {
            let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            // Independent mathematical definition, no adler2 or chunked state.
            let a = (1 + bytes.iter().map(|&v| u64::from(v)).sum::<u64>()) % 65521;
            let b = (size as u64
                + bytes
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (size - i) as u64 * u64::from(v))
                    .sum::<u64>())
                % 65521;
            let expected = format!("{:08x}", (b << 16) | a);
            let path = temp.file("boundary", "");
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                verify_paths([path]).single_digest(Algorithm::Adler32),
                Some(expected.as_str())
            );
        }
        let mut report = verify_paths([temp.file("final", "abc")]);
        report.cancelled = true;
        assert!(report.single_digest(Algorithm::Adler32).is_none());
    }

    #[test]
    fn sha1_legacy_vectors_streaming_exports_and_explicit_comparison() {
        let temp = TempDir::new();
        for (name, bytes, expected) in [
            (
                "empty",
                Vec::new(),
                "da39a3ee5e6b4b0d3255bfef95601890afd80709",
            ),
            (
                "abc",
                b"abc".to_vec(),
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                "million",
                vec![b'a'; 1_000_000],
                "34aa973cd4c4daa4f61eeb2bdbad27316534016f",
            ),
        ] {
            let path = temp.path().join(name);
            std::fs::write(&path, &bytes).unwrap();
            let report = verify_paths([path.clone()]);
            assert_eq!(report.single_digest(Algorithm::Sha1), Some(expected));
            assert_eq!(
                report.compare_expected_algorithm(&expected.to_uppercase(), Algorithm::Sha1),
                ExpectedMatch::Match
            );
            assert_eq!(
                report.compare_expected_algorithm(&"0".repeat(40), Algorithm::Sha1),
                ExpectedMatch::Mismatch
            );
            assert_eq!(
                report.compare_expected_algorithm(ABC_SHA, Algorithm::Sha1),
                ExpectedMatch::Invalid
            );
            assert!(report.detailed_text().contains(&format!("sha1={expected}")));
            assert_eq!(
                report.checksum_list(Algorithm::Sha1).unwrap(),
                format!("{expected} *{}\n", path.display())
            );
            #[cfg(target_os = "macos")]
            {
                let output = std::process::Command::new("/usr/bin/shasum")
                    .args(["-a", "1"])
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert!(
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .starts_with(expected)
                );
            }
        }
        assert_eq!(Algorithm::default(), Algorithm::Sha256);
    }

    #[test]
    fn md5_vectors_streaming_explicit_comparison_and_system_interoperability() {
        let temp = TempDir::new();
        for (name, data, expected) in [
            ("empty", String::new(), "d41d8cd98f00b204e9800998ecf8427e"),
            ("abc", "abc".into(), "900150983cd24fb0d6963f7d28e17f72"),
            (
                "digits",
                "123456789".into(),
                "25f9e794323b453885f5181f1b624d0b",
            ),
            (
                "million-a",
                "a".repeat(1_000_000),
                "7707d6ae4e027c70eea2a935c2296f21",
            ),
        ] {
            let path = temp.file(name, &data);
            let report = verify_paths([path.clone()]);
            assert_eq!(report.single_digest(Algorithm::Md5), Some(expected));
            assert_eq!(
                report.compare_expected_algorithm(&expected.to_uppercase(), Algorithm::Md5),
                ExpectedMatch::Match
            );
            assert_eq!(
                report.compare_expected_algorithm(&"0".repeat(32), Algorithm::Md5),
                ExpectedMatch::Mismatch
            );
            assert_eq!(
                report.compare_expected_algorithm(ABC_SHA, Algorithm::Md5),
                ExpectedMatch::Invalid
            );
            assert_eq!(
                report.compare_expected_algorithm(expected, Algorithm::Sha256),
                ExpectedMatch::Invalid
            );
            assert_eq!(
                report.compare_expected_algorithm(&"g".repeat(32), Algorithm::Md5),
                ExpectedMatch::Invalid
            );
            assert!(report.detailed_text().contains(&format!("md5={expected}")));
            assert_eq!(
                report.checksum_list(Algorithm::Md5).unwrap(),
                format!("{expected} *{}\n", path.display())
            );
            #[cfg(target_os = "macos")]
            {
                let output = std::process::Command::new("/sbin/md5")
                    .arg("-q")
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), expected);
            }
        }
    }

    #[test]
    fn md5_lists_escape_names_and_exclude_unavailable_results() {
        let temp = TempDir::new();
        let paths = [
            temp.file("space name", "abc"),
            temp.file("slash\\name", "abc"),
            temp.file("line\nname", "abc"),
            temp.file("carriage\rname", "abc"),
        ];
        let mut report = verify_paths(paths.clone());
        let digest = "900150983cd24fb0d6963f7d28e17f72";
        let listing = report.checksum_list(Algorithm::Md5).unwrap();
        assert_eq!(listing.lines().count(), paths.len());
        for (index, path) in paths.iter().enumerate() {
            let escaped = path
                .to_str()
                .unwrap()
                .replace('\\', "\\\\")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            let marker = if index == 0 { "" } else { "\\" };
            assert!(listing.contains(&format!("{marker}{digest} *{escaped}\n")));
        }
        report.rows[0].error = Some("failed".into());
        report.rows[1].skipped = Some("skipped".into());
        report.rows[2].md5_hex = Some("bad".into());
        report.rows[3].md5_hex = None;
        assert!(report.checksum_list(Algorithm::Md5).is_err());
        report.rows.truncate(1);
        assert!(report.single_digest(Algorithm::Md5).is_none());
        report.rows[0].error = None;
        report.cancelled = true;
        assert!(report.single_digest(Algorithm::Md5).is_none());
        assert_eq!(
            report.compare_expected_algorithm(digest, Algorithm::Md5),
            ExpectedMatch::Unavailable
        );
    }

    #[test]
    fn crc32_vectors_comparison_and_sfv_interoperability() {
        let temp = TempDir::new();
        for (name, data, expected) in [
            ("abc", "abc", "352441c2"),
            ("empty", "", "00000000"),
            ("digits", "123456789", "cbf43926"),
        ] {
            let report = verify_paths([temp.file(name, data)]);
            assert_eq!(report.single_digest(Algorithm::Crc32), Some(expected));
            assert_eq!(
                report.compare_expected_algorithm(&expected.to_uppercase(), Algorithm::Crc32),
                ExpectedMatch::Match
            );
            assert_eq!(
                report.compare_expected_algorithm("ffffffff", Algorithm::Crc32),
                ExpectedMatch::Mismatch
            );
            assert_eq!(
                report.compare_expected_algorithm(ABC_SHA, Algorithm::Crc32),
                ExpectedMatch::Invalid
            );
            assert!(
                report
                    .detailed_text()
                    .contains(&format!("crc32={expected}"))
            );
        }
        let paths = [
            temp.file("plain", "abc"),
            temp.file("with spaces", ""),
            temp.file("é", "123456789"),
            temp.file("large", &"abc".repeat(30000)),
            temp.file("tab\tname", "abc"),
            temp.file("trailing ", "abc"),
        ];
        let report = verify_paths(paths.clone());
        let sfv = report.checksum_list(Algorithm::Crc32).unwrap();
        assert!(sfv.starts_with(&format!("{} 352441C2\n", paths[0].display())));
        assert_eq!(sfv.lines().count(), paths.len());
        if let Some(executable) = std::env::var_os("COMMANDER_RHASH_VERIFY") {
            let manifest = temp.path().join("checksums.sfv");
            std::fs::write(&manifest, &sfv).unwrap();
            let result = std::process::Command::new(&executable)
                .arg("--check")
                .arg(&manifest)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            std::fs::write(&paths[0], "changed").unwrap();
            let result = std::process::Command::new(executable)
                .arg("--check")
                .arg(&manifest)
                .output()
                .unwrap();
            assert!(!result.status.success());
        }
    }

    #[test]
    fn sfv_refuses_ambiguous_paths_and_omits_incomplete_rows_without_sha_fallback() {
        let temp = TempDir::new();
        let mut report = verify_paths([temp.file("good", "abc")]);
        report.rows.extend([
            ChecksumRow {
                path: "/data/failed".into(),
                crc32_hex: Some("12345678".into()),
                error: Some("changed".into()),
                ..Default::default()
            },
            ChecksumRow {
                path: "/data/bad".into(),
                crc32_hex: Some("1234".into()),
                ..Default::default()
            },
            ChecksumRow {
                path: "/data/skipped".into(),
                crc32_hex: Some("12345678".into()),
                skipped: Some("symlink".into()),
                ..Default::default()
            },
        ]);
        report.cancelled = true;
        assert_eq!(
            report.checksum_list(Algorithm::Crc32).unwrap(),
            format!("{} 352441C2\n", report.rows[0].path.display())
        );
        for path in [
            "relative",
            "/data/line\nbreak",
            "/data/carriage\rname",
            "/data/nul\0name",
            "/data/back\\slash",
        ] {
            report.rows[0].path = path.into();
            assert!(report.checksum_list(Algorithm::Crc32).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            report.rows[0].path =
                std::ffi::OsString::from_vec(b"/data/invalid\xff".to_vec()).into();
            assert!(report.checksum_list(Algorithm::Crc32).is_err());
        }
        report.rows[0].crc32_hex = None;
        assert!(
            report
                .checksum_list(Algorithm::Crc32)
                .unwrap_err()
                .contains("No completed CRC32")
        );
    }

    #[test]
    fn relative_lists_survive_moving_the_directory_and_protect_special_filenames() {
        let temp = TempDir::new();
        let source = temp.dir("source");
        std::fs::create_dir(source.join("nested")).unwrap();
        let paths = [
            source.join("plain"),
            source.join("nested/two"),
            source.join("-"),
            source.join(";comment"),
            source.join("with space"),
            source.join("ёж"),
            source.join("trailing "),
        ];
        for path in &paths {
            std::fs::write(path, "abc").unwrap();
        }
        let report = verify_paths(paths);
        let algorithms = Algorithm::ALL;
        for algorithm in algorithms {
            let base = report.list_base(algorithm).unwrap();
            assert_eq!(base, source);
            let text = report.checksum_list_at(algorithm, Some(&base)).unwrap();
            assert!(!text.contains(source.to_str().unwrap()));
            assert!(
                text.contains("./nested/two")
                    && text.contains("./-")
                    && text.contains("./;comment")
            );
            std::fs::write(source.join(format!("{}.sums", algorithm.label())), text).unwrap();
        }
        let moved = temp.path().join("moved");
        std::fs::rename(&source, &moved).unwrap();
        let verify = |should_match: bool| {
            for algorithm in algorithms {
                let command = match algorithm {
                    #[cfg(target_os = "macos")]
                    Algorithm::Sha1 | Algorithm::Sha256 | Algorithm::Sha384 | Algorithm::Sha512 => {
                        let bits = match algorithm {
                            Algorithm::Sha1 => "1",
                            Algorithm::Sha256 => "256",
                            Algorithm::Sha384 => "384",
                            _ => "512",
                        };
                        let mut command = std::process::Command::new("/usr/bin/shasum");
                        command.args(["-a", bits, "-c"]);
                        Some(command)
                    }
                    Algorithm::Blake3 => std::env::var_os("COMMANDER_B3SUM_VERIFY").map(|exe| {
                        let mut command = std::process::Command::new(exe);
                        command.arg("--check");
                        command
                    }),
                    Algorithm::Crc32
                    | Algorithm::Md5
                    | Algorithm::Adler32
                    | Algorithm::Sha3_256
                    | Algorithm::Sha3_512
                    | Algorithm::Blake2s
                    | Algorithm::Blake2b => std::env::var_os("COMMANDER_RHASH_VERIFY").map(|exe| {
                        let mut command = std::process::Command::new(exe);
                        command.arg("--check");
                        command
                    }),
                    #[cfg(not(target_os = "macos"))]
                    _ => None,
                };
                if let Some(mut command) = command {
                    let result = command
                        .current_dir(&moved)
                        .arg(moved.join(format!("{}.sums", algorithm.label())))
                        .output()
                        .unwrap();
                    assert_eq!(
                        result.status.success(),
                        should_match,
                        "{}: {} {}",
                        algorithm.label(),
                        String::from_utf8_lossy(&result.stdout),
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
            }
        };
        verify(true);
        std::fs::write(moved.join("-"), "changed").unwrap();
        verify(false);
    }

    #[test]
    fn relative_list_base_uses_components_selected_algorithm_and_completed_rows_only() {
        let row = |path: &str| ChecksumRow {
            path: path.into(),
            sha256_hex: Some("a".repeat(64)),
            ..Default::default()
        };
        let mut report = ChecksumReport {
            rows: vec![
                row("/data/a/one"),
                row("/data/ab/two"),
                ChecksumRow {
                    path: "/elsewhere/bad".into(),
                    sha256_hex: Some("a".repeat(64)),
                    error: Some("changed".into()),
                    ..Default::default()
                },
                ChecksumRow {
                    path: "/elsewhere/crc".into(),
                    crc32_hex: Some("12345678".into()),
                    ..Default::default()
                },
            ],
            cancelled: true,
        };
        assert_eq!(
            report.list_base(Algorithm::Sha256).unwrap(),
            Path::new("/data")
        );
        assert_eq!(
            report.list_base(Algorithm::Crc32).unwrap(),
            Path::new("/elsewhere")
        );
        let text = report
            .checksum_list_at(Algorithm::Sha256, Some(Path::new("/data")))
            .unwrap();
        assert!(text.contains("*./a/one") && text.contains("*./ab/two"));
        assert!(!text.contains("elsewhere"));
        for base in ["relative", "/data/a", "/data/../data", "/unrelated"] {
            assert!(
                report
                    .checksum_list_at(Algorithm::Sha256, Some(Path::new(base)))
                    .is_err()
            );
        }
        report.rows[0].path = "/data/../escape".into();
        assert!(report.list_base(Algorithm::Sha256).is_err());
        assert!(
            report
                .checksum_list_at(Algorithm::Sha256, Some(Path::new("/")))
                .is_err()
        );
        assert!(report.checksum_list(Algorithm::Sha256).is_ok()); // Preserve absolute-mode contract.
        report.rows[0].path = "/".into();
        assert!(report.list_base(Algorithm::Sha256).is_err());
        report.rows.clear();
        assert!(report.list_base(Algorithm::Sha256).is_err());
    }

    #[test]
    fn extended_sha_known_vectors_and_selected_length_validation() {
        let temp = TempDir::new();
        for (name, data, sha384, sha512) in [
            ("abc", "abc", ABC_SHA384, ABC_SHA512),
            (
                "empty",
                "",
                "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b",
                "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
            ),
        ] {
            let report = verify_paths([temp.file(name, data)]);
            for (algorithm, expected) in [(Algorithm::Sha384, sha384), (Algorithm::Sha512, sha512)]
            {
                assert_eq!(report.single_digest(algorithm), Some(expected));
                assert_eq!(
                    report.compare_expected_algorithm(
                        &format!(" {} ", expected.to_uppercase()),
                        algorithm
                    ),
                    ExpectedMatch::Match
                );
                assert_eq!(
                    report.compare_expected_algorithm(&"0".repeat(algorithm.hex_len()), algorithm),
                    ExpectedMatch::Mismatch
                );
                assert_eq!(
                    report.compare_expected_algorithm(&"g".repeat(algorithm.hex_len()), algorithm),
                    ExpectedMatch::Invalid
                );
                assert_eq!(
                    report.compare_expected_algorithm(ABC_SHA, algorithm),
                    ExpectedMatch::Invalid
                );
            }
            assert_eq!(
                report.compare_expected_algorithm(sha384, Algorithm::Sha512),
                ExpectedMatch::Invalid
            );
            assert!(
                report
                    .detailed_text()
                    .contains(&format!("sha384={sha384} sha512={sha512}"))
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extended_sha_lists_verify_with_system_shasum_across_chunks_and_escaped_paths() {
        let temp = TempDir::new();
        let paths = [
            temp.file("abc", "abc"),
            temp.file("large", &"abc".repeat(30000)),
            temp.file("back\\slash", "abc"),
            temp.file("line\nbreak", "abc"),
            temp.file("é", ""),
        ];
        let report = verify_paths(paths);
        for (algorithm, bits) in [(Algorithm::Sha384, "384"), (Algorithm::Sha512, "512")] {
            let listing = report.checksum_list(algorithm).unwrap();
            let manifest = temp.path().join(format!("checksums.sha{bits}"));
            std::fs::write(&manifest, &listing).unwrap();
            let result = std::process::Command::new("/usr/bin/shasum")
                .args(["-a", bits, "-c"])
                .arg(&manifest)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    fn extended_sha_lists_omit_unavailable_rows_and_keep_algorithm_identity() {
        let report = ChecksumReport {
            rows: vec![
                ChecksumRow {
                    path: "/data/good".into(),
                    sha384_hex: Some(ABC_SHA384.into()),
                    sha512_hex: Some(ABC_SHA512.into()),
                    ..Default::default()
                },
                ChecksumRow {
                    path: "/data/failed".into(),
                    error: Some("changed".into()),
                    sha512_hex: Some(ABC_SHA512.into()),
                    ..Default::default()
                },
                ChecksumRow {
                    path: "/data/malformed".into(),
                    sha384_hex: Some(ABC_SHA.into()),
                    ..Default::default()
                },
            ],
            cancelled: true,
        };
        assert!(report.single_digest(Algorithm::Sha512).is_none());
        assert_eq!(
            report.checksum_list(Algorithm::Sha384).unwrap(),
            format!("{ABC_SHA384} */data/good\n")
        );
        assert_eq!(
            report.checksum_list(Algorithm::Sha512).unwrap(),
            format!("{ABC_SHA512} */data/good\n")
        );
        assert!(report.checksum_list(Algorithm::Blake3).is_err());
    }

    #[test]
    fn blake3_lists_preserve_paths_and_verify_with_b3sum_when_requested() {
        let temp = TempDir::new();
        let paths = [
            temp.file("abc", "abc"),
            temp.file("two  spaces", ""),
            temp.file("back\\slash", "abc"),
            temp.file("line\nbreak", "abc"),
            temp.file("carriage\rreturn", "abc"),
            temp.file("*é", &"abc".repeat(30000)),
        ];
        let mut report = verify_paths(paths.clone());
        report.rows.push(ChecksumRow {
            path: temp.path().join("failed"),
            blake3_hex: report.rows[0].blake3_hex.clone(),
            error: Some("changed".into()),
            ..Default::default()
        });
        report.rows.push(ChecksumRow {
            path: temp.path().join("malformed"),
            blake3_hex: Some("g".repeat(64)),
            ..Default::default()
        });
        report.cancelled = true; // Already completed rows remain exportable.
        let listing = report.checksum_list(Algorithm::Blake3).unwrap();
        assert_eq!(listing.lines().count(), paths.len());
        let hash = report.rows[0].blake3_hex.as_deref().unwrap();
        assert!(listing.starts_with(&format!("{hash}  {}\n", paths[0].display())));
        assert!(listing.contains("back\\\\slash\n"));
        assert!(listing.contains("line\\nbreak\n"));
        assert!(listing.contains("carriage\\rreturn\n"));
        assert!(!listing.contains("failed") && !listing.contains("malformed"));
        if let Some(executable) = std::env::var_os("COMMANDER_B3SUM_VERIFY") {
            let manifest = temp.path().join("checksums.b3");
            std::fs::write(&manifest, &listing).unwrap();
            let result = std::process::Command::new(&executable)
                .arg("--check")
                .arg(&manifest)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&result.stdout)
                    .matches(": OK")
                    .count(),
                paths.len()
            );
            // A copied manifest must detect changed payload, not just parse.
            std::fs::write(&paths[0], "changed").unwrap();
            let result = std::process::Command::new(executable)
                .arg("--check")
                .arg(&manifest)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert!(String::from_utf8_lossy(&result.stdout).contains(": FAILED"));
        }
    }

    #[test]
    fn blake3_lists_reject_ambiguous_paths_without_sha_fallback() {
        let temp = TempDir::new();
        let mut report = verify_paths([temp.file("abc", "abc")]);
        let hash = report.rows[0].blake3_hex.clone();
        report.rows.push(ChecksumRow {
            path: "/data/replacement�".into(),
            blake3_hex: hash,
            ..Default::default()
        });
        assert!(
            report
                .checksum_list(Algorithm::Blake3)
                .unwrap_err()
                .contains("replacement")
        );
        report.rows.truncate(1);
        report.rows[0].blake3_hex = None;
        assert!(
            report
                .checksum_list(Algorithm::Blake3)
                .unwrap_err()
                .contains("No completed BLAKE3")
        );
        assert!(report.checksum_list(Algorithm::Sha256).is_ok());
        report.rows[0].blake3_hex = Some("0".repeat(64));
        report.rows[0].path = "relative".into();
        assert!(report.checksum_list(Algorithm::Blake3).is_err());
        report.rows[0].path = "/data/nul\0suffix".into();
        assert!(report.checksum_list(Algorithm::Blake3).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            report.rows[0].path =
                std::ffi::OsString::from_vec(b"/data/invalid\xff".to_vec()).into();
            assert!(
                report
                    .checksum_list(Algorithm::Blake3)
                    .unwrap_err()
                    .contains("not UTF-8")
            );
        }
    }

    #[test]
    fn explicit_algorithm_comparison_never_guesses_from_equal_digest_lengths() {
        let temp = TempDir::new();
        temp.file("abc", "abc");
        let report = verify_paths([temp.path().join("abc")]);
        let blake = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";
        assert_eq!(report.single_digest(Algorithm::Blake3), Some(blake));
        assert_eq!(report.single_digest(Algorithm::Sha256), Some(ABC_SHA));
        assert_eq!(
            report.compare_expected_algorithm(
                &format!(" {} ", blake.to_uppercase()),
                Algorithm::Blake3
            ),
            ExpectedMatch::Match
        );
        assert_eq!(
            report.compare_expected_algorithm(ABC_SHA, Algorithm::Blake3),
            ExpectedMatch::Mismatch
        );
        assert_eq!(
            report.compare_expected_algorithm(blake, Algorithm::Sha256),
            ExpectedMatch::Mismatch
        );
        assert_eq!(
            report.compare_expected_algorithm("bad", Algorithm::Blake3),
            ExpectedMatch::Invalid
        );
        assert_eq!(
            report.compare_expected_algorithm("", Algorithm::Blake3),
            ExpectedMatch::Empty
        );
    }

    #[test]
    fn digest_copy_and_comparison_reject_incomplete_or_ambiguous_reports() {
        let row = ChecksumRow {
            blake3_hex: Some("a".repeat(64)),
            sha256_hex: Some("b".repeat(64)),
            sha384_hex: Some("c".repeat(96)),
            sha512_hex: Some("d".repeat(128)),
            crc32_hex: Some("a".repeat(8)),
            md5_hex: Some("e".repeat(32)),
            ..Default::default()
        };
        let mut report = ChecksumReport {
            rows: vec![row.clone()],
            cancelled: false,
        };
        for algorithm in [
            Algorithm::Sha256,
            Algorithm::Sha384,
            Algorithm::Sha512,
            Algorithm::Blake3,
            Algorithm::Crc32,
            Algorithm::Md5,
        ] {
            assert!(report.single_digest(algorithm).is_some());
            report.cancelled = true;
            assert!(report.single_digest(algorithm).is_none());
            report.cancelled = false;
        }
        for bad_row in [
            ChecksumRow {
                error: Some("changed".into()),
                ..row.clone()
            },
            ChecksumRow {
                skipped: Some("folder".into()),
                ..row.clone()
            },
            ChecksumRow {
                blake3_hex: Some("z".repeat(64)),
                sha256_hex: None,
                sha384_hex: Some("c".repeat(64)),
                sha512_hex: None,
                crc32_hex: None,
                md5_hex: None,
                ..row.clone()
            },
        ] {
            report.rows = vec![bad_row];
            for algorithm in [
                Algorithm::Sha256,
                Algorithm::Sha384,
                Algorithm::Sha512,
                Algorithm::Blake3,
                Algorithm::Crc32,
                Algorithm::Md5,
            ] {
                assert!(report.single_digest(algorithm).is_none());
                assert_eq!(
                    report.compare_expected_algorithm(&"a".repeat(algorithm.hex_len()), algorithm),
                    ExpectedMatch::Unavailable
                );
            }
        }
        report.rows = vec![row.clone(), row];
        assert!(report.single_digest(Algorithm::Blake3).is_none());
        report.rows.clear();
        assert!(report.single_digest(Algorithm::Blake3).is_none());
    }

    #[test]
    fn sha256_known_vectors_and_expected_digest_comparison() {
        let temp = TempDir::new();
        let abc = temp.file("abc", "abc");
        let empty = temp.file("empty", "");
        let report = verify_paths([abc]);
        assert_eq!(report.rows[0].sha256_hex.as_deref(), Some(ABC_SHA));
        assert_eq!(
            report.compare_expected(&format!(" {} ", ABC_SHA.to_uppercase())),
            ExpectedMatch::Match
        );
        assert_eq!(
            report.compare_expected(&"0".repeat(64)),
            ExpectedMatch::Mismatch
        );
        assert_eq!(report.compare_expected("bad"), ExpectedMatch::Invalid);
        assert_eq!(
            report.compare_expected(&"g".repeat(64)),
            ExpectedMatch::Invalid
        );
        assert_eq!(report.compare_expected(""), ExpectedMatch::Empty);
        assert_eq!(
            verify_paths([empty]).rows[0].sha256_hex.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        let million = temp.file("million", &"a".repeat(1_000_000));
        assert_eq!(
            verify_paths([million]).rows[0].sha256_hex.as_deref(),
            Some("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0")
        );
        let cancelled = ChecksumReport {
            cancelled: true,
            ..report.clone()
        };
        assert_eq!(
            cancelled.compare_expected(ABC_SHA),
            ExpectedMatch::Unavailable
        );
    }

    #[test]
    fn cancellation_and_changed_file_never_publish_partial_digests() {
        let temp = TempDir::new();
        let path = temp.file("large", &"a".repeat(200_000));
        let ticks = std::cell::Cell::new(0);
        let row = verify_one(&path, &|| {
            ticks.set(ticks.get() + 1);
            ticks.get() >= 3
        });
        assert!(row.error.as_deref().unwrap().contains("cancelled"));
        assert!(
            row.sha256_hex.is_none()
                && row.blake3_hex.is_none()
                && row.sha384_hex.is_none()
                && row.sha512_hex.is_none()
                && row.sha3_256_hex.is_none()
                && row.sha3_512_hex.is_none()
                && row.blake2s_hex.is_none()
                && row.blake2b_hex.is_none()
                && row.crc32_hex.is_none()
        );
        let ticks = std::cell::Cell::new(0);
        let row = verify_one(&path, &|| {
            ticks.set(ticks.get() + 1);
            if ticks.get() == 3 {
                std::fs::rename(&path, temp.path().join("original")).unwrap();
                std::fs::write(&path, "new file").unwrap();
            }
            false
        });
        assert!(row.error.as_deref().unwrap().contains("changed"));
        assert!(
            row.sha256_hex.is_none()
                && row.blake3_hex.is_none()
                && row.sha384_hex.is_none()
                && row.sha512_hex.is_none()
                && row.sha3_256_hex.is_none()
                && row.sha3_512_hex.is_none()
                && row.blake2s_hex.is_none()
                && row.blake2b_hex.is_none()
                && row.crc32_hex.is_none()
        );
    }

    #[test]
    fn growing_and_truncated_files_never_publish_digests() {
        use std::io::Write;
        let temp = TempDir::new();
        for grow in [true, false] {
            let path = temp.file(
                if grow { "growing" } else { "truncated" },
                &"a".repeat(200_000),
            );
            let ticks = std::cell::Cell::new(0);
            let row = verify_one(&path, &|| {
                ticks.set(ticks.get() + 1);
                if ticks.get() == 3 {
                    if grow {
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .unwrap()
                            .write_all(b"extra")
                            .unwrap();
                    } else {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .open(&path)
                            .unwrap()
                            .set_len(0)
                            .unwrap();
                    }
                }
                false
            });
            assert!(row.error.as_deref().unwrap().contains("changed"));
            assert!(
                row.sha256_hex.is_none()
                    && row.blake3_hex.is_none()
                    && row.sha384_hex.is_none()
                    && row.sha512_hex.is_none()
                    && row.sha3_256_hex.is_none()
                    && row.sha3_512_hex.is_none()
                    && row.blake2s_hex.is_none()
                    && row.blake2b_hex.is_none()
                    && row.crc32_hex.is_none()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn checksum_open_rejects_symlink_substitution_and_special_files() {
        let temp = TempDir::new();
        let path = temp.file("selected", "abc");
        let target = temp.file("target", "secret");
        let changed = std::cell::Cell::new(false);
        let row = verify_one(&path, &|| {
            if !changed.replace(true) {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(&target, &path).unwrap();
            }
            false
        });
        assert!(row.error.is_some());
        assert!(row.sha256_hex.is_none());
        let fifo = temp.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let row = verify_one(&fifo, &|| false);
        assert!(row.skipped.as_deref().unwrap().contains("regular"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sha256_list_round_trips_through_system_shasum_with_escaped_names() {
        let temp = TempDir::new();
        let files = ["normal file", "back\\slash", "line\nbreak", "é"];
        let report = verify_paths(files.iter().map(|name| temp.file(name, "abc")));
        let listing = report.sha256_list().unwrap();
        let manifest = temp.path().join("checksums.sha256");
        std::fs::write(&manifest, &listing).unwrap();
        let result = std::process::Command::new("/usr/bin/shasum")
            .args(["-a", "256", "--strict", "-c"])
            .arg(&manifest)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "stdout={} stderr={} manifest={listing:?}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn sha256_list_escapes_carriage_return_and_rejects_unavailable_rows() {
        let temp = TempDir::new();
        let path = temp.file("carriage\rreturn", "abc");
        let report = verify_paths([path.clone(), temp.dir("folder")]);
        assert_eq!(report.compare_expected(ABC_SHA), ExpectedMatch::Unavailable);
        assert_eq!(
            report.sha256_list().unwrap(),
            format!(
                "\\{ABC_SHA} *{}\n",
                path.to_str().unwrap().replace('\r', "\\r")
            )
        );
        assert!(ChecksumReport::default().sha256_list().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn checksum_list_rejects_non_utf8_paths_without_lossy_export() {
        use std::os::unix::ffi::OsStringExt;
        let temp = TempDir::new();
        let path = temp.path().join(std::ffi::OsString::from_vec(vec![0xff]));
        let report = ChecksumReport {
            rows: vec![ChecksumRow {
                path,
                sha256_hex: Some(ABC_SHA.into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(report.successful(), 1);
        assert!(report.sha256_list().unwrap_err().contains("UTF-8"));
    }

    #[test]
    fn identical_files_share_hashes() {
        let temp = TempDir::new();
        let a = temp.file("a.bin", "payload");
        let b = temp.file("b.bin", "payload");
        let report = verify_paths([a, b]);
        assert_eq!(report.rows.len(), 2);
        assert!(report.rows.iter().all(|row| row.error.is_none()));
        assert_eq!(report.rows[0].content_hash, report.rows[1].content_hash);
        assert_eq!(report.rows[0].blake3_hex, report.rows[1].blake3_hex);
    }

    #[test]
    fn folders_are_skipped_not_failed() {
        let temp = TempDir::new();
        let file = temp.file("a.bin", "payload");
        let folder = temp.dir("Photos");
        let missing = temp.path().join("gone.bin");
        let report = verify_paths([file, folder, missing]);
        assert_eq!(report.rows[1].error, None);
        assert!(report.rows[1].skipped.is_some());
        assert!(report.rows[2].error.is_some());
        assert_eq!(
            report.summary_line(),
            "Calculated 1 file(s), 1 failed, 1 skipped"
        );
        assert!(report.detailed_text().contains("Photos: skipped ("));
        // A file plus a folder is a clean report.
        let report = verify_paths([temp.path().join("a.bin"), temp.path().join("Photos")]);
        assert!(report.rows.iter().all(|row| row.error.is_none()));
        assert_eq!(report.summary_line(), "Calculated 1 file(s), 1 skipped");
    }

    #[test]
    fn content_hash_matches_the_streaming_content_hash() {
        let temp = TempDir::new();
        let path = temp.file("a.bin", "some payload bytes");
        let report = verify_paths([path.clone()]);
        let row = &report.rows[0];
        assert_eq!(row.error, None);
        assert_eq!(row.content_hash, crate::fs_util::content_hash(&path));
        assert!(row.content_hash.is_some());
    }

    #[test]
    fn blake3_hex_is_the_lowercase_digest_of_the_content() {
        let temp = TempDir::new();
        let path = temp.file("a.bin", "some payload bytes");
        let report = verify_paths([path]);
        let expected = blake3::hash(b"some payload bytes").to_hex().to_string();
        assert_eq!(
            report.rows[0].blake3_hex.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(expected.len(), 64);
    }

    fn row(name: &str) -> ChecksumRow {
        ChecksumRow {
            path: PathBuf::from(name),
            content_hash: None,
            blake3_hex: None,
            sha256_hex: None,
            sha384_hex: None,
            sha512_hex: None,
            sha3_256_hex: None,
            sha3_512_hex: None,
            blake2s_hex: None,
            blake2b_hex: None,
            crc32_hex: None,
            md5_hex: None,
            sha1_hex: None,
            adler32_hex: None,
            error: None,
            skipped: None,
        }
    }

    #[test]
    fn detailed_text_formats_ok_error_skipped_and_unknown_rows() {
        let report = ChecksumReport {
            rows: vec![
                ChecksumRow {
                    content_hash: Some(0xab),
                    blake3_hex: Some("cafe".to_string()),
                    ..row("/data/ok.bin")
                },
                ChecksumRow {
                    error: Some("permission denied".to_string()),
                    // An error wins over any partial hash.
                    content_hash: Some(1),
                    ..row("/data/bad.bin")
                },
                ChecksumRow {
                    skipped: Some("symlinks are not hashed".to_string()),
                    ..row("/data/link")
                },
                row("/data/unknown.bin"),
            ],
            ..Default::default()
        };
        assert_eq!(
            report.detailed_text(),
            "/data/ok.bin: content=00000000000000ab blake3=cafe sha256=- sha384=- sha512=- sha3_256=- sha3_512=- blake2s=- blake2b=- crc32=- md5=- sha1=- adler32=-\n\
             /data/bad.bin: ERROR permission denied\n\
             /data/link: skipped (symlinks are not hashed)\n\
             /data/unknown.bin: content=- blake3=- sha256=- sha384=- sha512=- sha3_256=- sha3_512=- blake2s=- blake2b=- crc32=- md5=- sha1=- adler32=-"
        );
        // A path without a file name (the root) falls back to the full path.
        let root = ChecksumReport {
            rows: vec![row("/")],
            ..Default::default()
        };
        assert_eq!(
            root.detailed_text(),
            "/: content=- blake3=- sha256=- sha384=- sha512=- sha3_256=- sha3_512=- blake2s=- blake2b=- crc32=- md5=- sha1=- adler32=-"
        );
    }

    #[test]
    fn summary_line_for_empty_and_failed_only_reports() {
        assert_eq!(
            ChecksumReport::default().summary_line(),
            "Calculated 0 file(s)"
        );
        assert_eq!(ChecksumReport::default().detailed_text(), "");
        let report = ChecksumReport {
            rows: vec![
                row("/a"),
                ChecksumRow {
                    error: Some("gone".to_string()),
                    ..row("/b")
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            report.summary_line(),
            "Calculated 0 file(s), 1 failed, 1 unavailable"
        );
    }

    #[test]
    fn missing_paths_report_the_os_error_and_folders_explain_the_skip() {
        let temp = TempDir::new();
        let missing = temp.path().join("gone.bin");
        let folder = temp.dir("Photos");
        let report = verify_paths([missing.clone(), folder]);
        let expected = std::fs::symlink_metadata(&missing).unwrap_err().to_string();
        assert_eq!(report.rows[0].error.as_deref(), Some(expected.as_str()));
        assert_eq!(report.rows[0].skipped, None);
        assert_eq!(
            report.rows[1].skipped.as_deref(),
            Some("directories have no content checksum")
        );
        assert_eq!(report.rows[1].content_hash, None);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_without_following_them() {
        let temp = TempDir::new();
        let target = temp.file("target.bin", "payload");
        let link = temp.path().join("link.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let dangling = temp.path().join("dangling.bin");
        std::os::unix::fs::symlink(temp.path().join("nowhere"), &dangling).unwrap();
        let report = verify_paths([link, dangling]);
        for row in &report.rows {
            assert_eq!(row.error, None);
            assert_eq!(row.blake3_hex, None);
            assert_eq!(row.skipped.as_deref(), Some("symlinks are not hashed"));
        }
        assert_eq!(report.summary_line(), "Calculated 0 file(s), 2 skipped");
    }
}
