//! Bounded checksum-list verification; all filesystem work runs on the hash lane.
use crate::checksum::Algorithm;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
const MAX_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 10_000;

#[derive(Debug, PartialEq, Eq)]
struct Record {
    name: String,
    expected: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Match,
    Mismatch,
    Error(String),
}
impl Status {
    pub fn label(&self) -> &str {
        match self {
            Self::Match => "MATCH",
            Self::Mismatch => "MISMATCH",
            Self::Error(_) => "ERROR",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Row {
    pub name: String,
    pub expected: String,
    pub actual: Option<String>,
    pub status: Status,
}
#[derive(Debug)]
pub struct Report {
    pub rows: Vec<Row>,
    pub total: usize,
    pub cancelled: bool,
    pub error: Option<String>,
    pub algorithm: Option<Algorithm>,
}
impl Report {
    pub fn summary(&self) -> String {
        if let Some(error) = &self.error {
            return format!("Could not verify list: {error}");
        }
        let matches = self
            .rows
            .iter()
            .filter(|row| row.status == Status::Match)
            .count();
        let mismatches = self
            .rows
            .iter()
            .filter(|row| row.status == Status::Mismatch)
            .count();
        format!(
            "{}{} of {} records: {} match, {} mismatch, {} failed",
            if self.cancelled {
                "Stopped: "
            } else {
                "Verified "
            },
            self.rows.len(),
            self.total,
            matches,
            mismatches,
            self.rows.len() - matches - mismatches
        )
    }
    pub fn text(&self) -> String {
        let mut text = format!(
            "{} checksum list\n{}\n",
            self.algorithm.map_or("Undetermined", Algorithm::label),
            self.summary()
        );
        for row in &self.rows {
            write_row(&mut text, row).expect("writing to a String cannot fail");
        }
        text
    }
}

pub fn write_row(output: &mut impl std::fmt::Write, row: &Row) -> std::fmt::Result {
    write!(
        output,
        "{} {} expected={} actual={}",
        row.status.label(),
        row.name.escape_debug(),
        row.expected,
        row.actual.as_deref().unwrap_or("-")
    )?;
    if let Status::Error(error) = &row.status {
        write!(output, " ({})", error.escape_debug())?;
    }
    writeln!(output)
}

fn parse(text: &str, algorithm: Algorithm) -> Result<Vec<Record>, String> {
    if text.len() > MAX_BYTES {
        return Err("Checksum lists are limited to 1 MiB".into());
    }
    let mut records = Vec::new();
    for (index, line) in text
        .trim_start_matches('\u{feff}')
        .split_terminator('\n')
        .enumerate()
    {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty()
            || line.starts_with('#')
            || (algorithm == Algorithm::Crc32 && line.starts_with(';'))
        {
            continue;
        }
        let error = || format!("Invalid {} record on line {}", algorithm.label(), index + 1);
        // SFV has literal backslashes; only an actual tagged CRC32 record escapes them.
        let raw = line.strip_prefix('\\').unwrap_or(line);
        let escaped = line.starts_with('\\')
            && (algorithm != Algorithm::Crc32 || split_tagged(raw).is_some());
        let body = if escaped { raw } else { line };
        let (expected, name) = if let Some((tag_algorithm, name, expected)) = split_tagged(body) {
            if tag_algorithm != algorithm {
                return Err(error());
            }
            (expected.to_owned(), name.to_owned())
        } else {
            parse_untagged(body, algorithm).ok_or_else(error)?
        };
        let name = if escaped {
            let mut output = String::new();
            let mut chars = name.chars();
            while let Some(c) = chars.next() {
                output.push(if c == '\\' {
                    match chars.next() {
                        Some('\\') => '\\',
                        Some('n') => '\n',
                        Some('r') => '\r',
                        _ => return Err(error()),
                    }
                } else {
                    c
                });
            }
            output
        } else {
            name
        };
        if expected.len() != algorithm.hex_len()
            || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
            || name.is_empty()
            || name.contains('\0')
        {
            return Err(error());
        }
        if records.len() == MAX_RECORDS {
            return Err("Checksum lists are limited to 10000 records".into());
        }
        records.push(Record { name, expected });
    }
    if records.is_empty() {
        return Err("No checksum records in the selected format".into());
    }
    Ok(records)
}

fn tagged_algorithm(tag: &str) -> Option<Algorithm> {
    match tag.to_ascii_uppercase().as_str() {
        "SHA256" | "SHA-256" | "SHA2-256" => Some(Algorithm::Sha256),
        "SHA384" | "SHA-384" | "SHA2-384" => Some(Algorithm::Sha384),
        "SHA512" | "SHA-512" | "SHA2-512" => Some(Algorithm::Sha512),
        "MD5" => Some(Algorithm::Md5),
        "SHA1" | "SHA-1" => Some(Algorithm::Sha1),
        "BLAKE3" => Some(Algorithm::Blake3),
        "CRC32" => Some(Algorithm::Crc32),
        "ADLER32" | "ADLER-32" => Some(Algorithm::Adler32),
        _ => None,
    }
}

fn split_tagged(body: &str) -> Option<(Algorithm, &str, &str)> {
    let open = body.find('(')?;
    let algorithm = tagged_algorithm(body[..open].trim_end())?;
    let (name, suffix) = body[open + 1..].rsplit_once(')')?;
    let expected = suffix
        .strip_prefix(" = ")
        .or_else(|| suffix.strip_prefix("= "))?;
    Some((algorithm, name, expected))
}

fn parse_untagged(body: &str, algorithm: Algorithm) -> Option<(String, String)> {
    if algorithm == Algorithm::Crc32 {
        let boundary = body.rfind([' ', '\t'])?;
        Some((body[boundary + 1..].to_owned(), body[..boundary].to_owned()))
    } else {
        let width = algorithm.hex_len();
        let marker = body.get(width..width + 2)?;
        if marker != "  " && (marker != " *" || algorithm == Algorithm::Blake3) {
            return None;
        }
        Some((
            body.get(..width)?.to_owned(),
            body.get(width + 2..)?.to_owned(),
        ))
    }
}

fn filename_algorithm(path: &Path) -> Option<Algorithm> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    match name.as_str() {
        "md5sums" => return Some(Algorithm::Md5),
        "sha1sums" => return Some(Algorithm::Sha1),
        "adler32sums" => return Some(Algorithm::Adler32),
        "sha256sums" => return Some(Algorithm::Sha256),
        "sha384sums" => return Some(Algorithm::Sha384),
        "sha512sums" => return Some(Algorithm::Sha512),
        "b3sums" | "blake3sums" => return Some(Algorithm::Blake3),
        _ => {}
    }
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "sha256" | "sha256sum" => Some(Algorithm::Sha256),
        "sha384" | "sha384sum" => Some(Algorithm::Sha384),
        "sha512" | "sha512sum" => Some(Algorithm::Sha512),
        "md5" | "md5sum" => Some(Algorithm::Md5),
        "sha1" | "sha1sum" => Some(Algorithm::Sha1),
        "adler32" | "adler32sum" => Some(Algorithm::Adler32),
        "b3" | "blake3" => Some(Algorithm::Blake3),
        "sfv" | "crc32" => Some(Algorithm::Crc32),
        _ => None,
    }
}

fn detect(
    path: &Path,
    text: &str,
    selected: Option<Algorithm>,
) -> Result<(Algorithm, Vec<Record>), String> {
    if let Some(algorithm) = selected {
        return parse(text, algorithm).map(|rows| (algorithm, rows));
    }
    if let Some(algorithm) = filename_algorithm(path) {
        return parse(text, algorithm).map(|rows| (algorithm, rows)).map_err(|error|
            format!("Filename indicates {}, but the list does not match: {error}. Choose the algorithm explicitly if the filename is misleading", algorithm.label()));
    }
    let mut detected = None;
    for algorithm in Algorithm::ALL {
        if let Ok(rows) = parse(text, algorithm) {
            if detected.is_some() {
                return Err("Ambiguous checksum algorithm or record layout: choose explicitly (64-digit hashes may be SHA-256 or BLAKE3), or use a tagged list / recognized filename".into());
            }
            detected = Some((algorithm, rows));
        }
    }
    detected.ok_or_else(|| "Could not detect a supported checksum list. Mixed algorithms, unsupported tags and malformed records are refused; choose an algorithm explicitly for format details".into())
}

fn read_manifest(path: &Path) -> Result<String, String> {
    let before = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() {
        return Err("Select a regular checksum-list file, not a symlink or directory".into());
    }
    if before.len() > MAX_BYTES as u64 {
        return Err("Checksum lists are limited to 1 MiB".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    let identity = |meta: &std::fs::Metadata| {
        crate::path_identity::PathIdentity::from_metadata(path.into(), meta)
    };
    let expected = identity(&before);
    if !file
        .metadata()
        .is_ok_and(|meta| meta.is_file() && expected.same_shallow_binding(&identity(&meta)))
    {
        return Err("Checksum list changed before opening".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("Checksum lists are limited to 1 MiB".into());
    }
    if !file
        .metadata()
        .is_ok_and(|meta| expected.same_shallow_binding(&identity(&meta)))
        || !crate::path_identity::PathIdentity::observe(path)
            .is_ok_and(|now| expected.same_shallow_binding(&now))
    {
        return Err("Checksum list changed while reading".into());
    }
    String::from_utf8(bytes).map_err(|_| "Checksum list is not UTF-8".into())
}

fn resolve(root: &Path, canonical_root: &Path, name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("Parent traversal and drive-prefixed paths are not supported".into());
    }
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .or_else(|_| path.strip_prefix(canonical_root))
            .map_err(|_| "Path is outside the checksum-list folder")?
    } else {
        path
    };
    let mut resolved = canonical_root.to_path_buf();
    let mut count = 0;
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                resolved.push(name);
                count += 1;
                let meta = std::fs::symlink_metadata(&resolved).map_err(|e| e.to_string())?;
                if meta.file_type().is_symlink() {
                    return Err("Symlinks in checksum-list paths are not followed".into());
                }
            }
            _ => return Err("Path is outside the checksum-list folder".into()),
        }
    }
    if count == 0 {
        return Err("Checksum record has no filename".into());
    }
    Ok(resolved)
}

pub fn verify(
    path: &Path,
    algorithm: Option<Algorithm>,
    cancelled: &dyn Fn() -> bool,
    completed: &AtomicUsize,
    total: &AtomicUsize,
) -> Report {
    let mut report = Report {
        rows: Vec::new(),
        total: 0,
        cancelled: false,
        error: None,
        algorithm,
    };
    if cancelled() {
        report.cancelled = true;
        return report;
    }
    let setup = || {
        if !path.is_absolute() {
            return Err("Checksum-list path must be absolute".into());
        }
        let (algorithm, records) = detect(path, &read_manifest(path)?, algorithm)?;
        let root = path
            .parent()
            .ok_or("Checksum list has no folder")?
            .to_path_buf();
        let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
        Ok::<_, String>((algorithm, records, root, canonical_root))
    };
    let (algorithm, records, root, canonical_root) = match setup() {
        Ok(value) => value,
        Err(error) => {
            report.error = Some(error);
            return report;
        }
    };
    report.algorithm = Some(algorithm);
    report.total = records.len();
    total.store(records.len(), Ordering::Release);
    for record in records {
        if cancelled() {
            report.cancelled = true;
            break;
        }
        let (actual, status) = match resolve(&root, &canonical_root, &record.name) {
            Err(error) => (None, Status::Error(error)),
            Ok(target) => {
                let row = crate::checksum::verify_one(&target, cancelled);
                let actual = algorithm.digest(&row).map(str::to_owned);
                let status = if let Some(error) = row.error.or(row.skipped) {
                    Status::Error(error)
                } else if let Some(actual) = &actual {
                    if actual.eq_ignore_ascii_case(&record.expected) {
                        Status::Match
                    } else {
                        Status::Mismatch
                    }
                } else {
                    Status::Error("Digest unavailable".into())
                };
                (actual, status)
            }
        };
        if cancelled() {
            report.cancelled = true;
            break;
        }
        report.rows.push(Row {
            name: record.name,
            expected: record.expected,
            actual,
            status,
        });
        completed.fetch_add(1, Ordering::Release);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    fn run(path: &Path, algorithm: Algorithm) -> Report {
        verify(
            path,
            Some(algorithm),
            &|| false,
            &AtomicUsize::new(0),
            &AtomicUsize::new(0),
        )
    }

    #[test]
    fn adler32_lists_support_tags_hints_and_gnu_layout_without_crc_confusion() {
        let dir = TempDir::new();
        dir.file("data", "Wikipedia");
        for (name, text) in [
            ("checksums.txt", "11e60398  data\n"),
            ("ADLER32SUMS", "11E60398 *data\n"),
            ("list.adler32", "ADLER-32 (data) = 11e60398\n"),
            ("list.adler32sum", "ADLER32 (data) = 11e60398\n"),
        ] {
            let path = dir.file(name, text);
            let report = verify(
                &path,
                None,
                &|| false,
                &AtomicUsize::new(0),
                &AtomicUsize::new(0),
            );
            assert_eq!(
                report.algorithm,
                Some(Algorithm::Adler32),
                "{:?}",
                report.error
            );
            assert_eq!(report.rows[0].status, Status::Match);
        }
        let path = dir.file("bad.adler32", "00000000  data\n");
        assert_eq!(
            run(&path, Algorithm::Adler32).rows[0].status,
            Status::Mismatch
        );
        assert!(parse("CRC32 (data) = 11e60398\n", Algorithm::Adler32).is_err());
        assert!(parse("ADLER32 (data) = 11e60398\n", Algorithm::Crc32).is_err());
        assert!(detect(Path::new("list.sfv"), "11e60398  data\n", None).is_err());
    }

    #[test]
    fn sha1_lists_detect_gnu_bsd_and_filename_hints_and_verify_mismatches() {
        let dir = TempDir::new();
        dir.file("data", "abc");
        let hash = "a9993e364706816aba3e25717850c26c9cd0d89d";
        for (name, text) in [
            ("checksums.txt", format!("{hash} *data\n")),
            ("SHA1SUMS", format!("{hash}  data\n")),
            (
                "list.sha1sum",
                format!("SHA-1 (data) = {}\n", hash.to_uppercase()),
            ),
        ] {
            let manifest = dir.file(name, &text);
            let report = verify(
                &manifest,
                None,
                &|| false,
                &AtomicUsize::new(0),
                &AtomicUsize::new(0),
            );
            assert_eq!(report.algorithm, Some(Algorithm::Sha1));
            assert_eq!(report.rows[0].status, Status::Match, "{:?}", report.error);
        }
        let manifest = dir.file("bad.sha1", &format!("{}  data\n", "0".repeat(40)));
        let report = verify(
            &manifest,
            None,
            &|| false,
            &AtomicUsize::new(0),
            &AtomicUsize::new(0),
        );
        assert_eq!(report.rows[0].status, Status::Mismatch);
        assert!(detect(Path::new("SHA1SUMS"), &format!("{ABC}  data\n"), None).is_err());
        assert!(
            detect(
                Path::new("mixed.txt"),
                &format!("SHA1 (data) = {hash}\nMD5 (data) = 900150983cd24fb0d6963f7d28e17f72\n"),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn parser_preserves_escaped_names_spaces_crlf_and_sfv_comments() {
        let text = format!(
            "\u{feff}# comment\r\n\\{ABC} *dir\\\\name\\nline\\rreturn \r\n{ABC}  ./other\n"
        );
        let rows = parse(&text, Algorithm::Sha256).unwrap();
        assert_eq!(rows[0].name, "dir\\name\nline\rreturn ");
        assert_eq!(rows[1].name, "./other");
        let rows = parse(
            "; comment\r\n./name with spaces  352441C2\r\n",
            Algorithm::Crc32,
        )
        .unwrap();
        assert_eq!(rows[0].name, "./name with spaces ");
        assert_eq!(rows[0].expected, "352441C2");
        for text in [
            format!("{ABC} ./missing-marker\n"),
            format!("\\{ABC} *bad\\q\n"),
            format!("{ABC} *bad\0name\n"),
            format!("{ABC} *\n"),
            "garbage".into(),
        ] {
            assert!(parse(&text, Algorithm::Sha256).is_err());
        }
        assert!(parse(&format!("{ABC} *file"), Algorithm::Blake3).is_err());
        assert!(parse("# only comments\n", Algorithm::Sha256).is_err());
    }

    #[test]
    fn tagged_names_preserve_delimiters_unicode_spaces_and_gnu_escapes() {
        let name = " папка/(a) = b ";
        for line in [
            format!("SHA256 ({name}) = {ABC}"),
            format!("SHA256({name})= {ABC}"),
            format!("SHA2-256 ({name}) = {ABC}"),
        ] {
            let rows = parse(&line, Algorithm::Sha256).unwrap();
            assert_eq!(rows[0].name, name);
            assert_eq!(rows[0].expected, ABC);
        }
        let rows = parse(
            &format!("\\SHA256 (a\\nline\\\\name) = {ABC}"),
            Algorithm::Sha256,
        )
        .unwrap();
        assert_eq!(rows[0].name, "a\nline\\name");
        let rows = parse("\\literal\\q 352441C2", Algorithm::Crc32).unwrap();
        assert_eq!(rows[0].name, "\\literal\\q");
        for name in ["CRC32 (name)", "SHA256 (name)", "\\CRC32 (name)"] {
            let rows = parse(&format!("{name} 352441C2"), Algorithm::Crc32).unwrap();
            assert_eq!(rows[0].name, name);
        }
        for text in [
            format!("SHA512 (a) = {ABC}"),
            format!("SHA256 () = {ABC}"),
            format!("SHA256 (a) = {ABC} trailing"),
            format!("\\SHA256 (a\\q) = {ABC}"),
        ] {
            assert!(parse(&text, Algorithm::Sha256).is_err());
        }
    }

    #[test]
    fn automatic_detection_requires_an_unambiguous_complete_list_and_respects_manual_override() {
        let raw = format!("{ABC}  a\n");
        assert!(
            detect(Path::new("list.txt"), &raw, None)
                .unwrap_err()
                .contains("Ambiguous")
        );
        assert_eq!(
            detect(Path::new("SHA256SUMS"), &raw, None).unwrap().0,
            Algorithm::Sha256
        );
        assert_eq!(
            detect(Path::new("list.BLAKE3"), &raw, None).unwrap().0,
            Algorithm::Blake3
        );
        assert_eq!(
            detect(Path::new("list"), &raw, Some(Algorithm::Sha256))
                .unwrap()
                .0,
            Algorithm::Sha256
        );
        let tagged = "MD5 (a) = 900150983cd24fb0d6963f7d28e17f72\n";
        assert_eq!(
            detect(Path::new("list"), tagged, None).unwrap().0,
            Algorithm::Md5
        );
        assert!(
            detect(Path::new("list.sha256"), tagged, None)
                .unwrap_err()
                .contains("Filename indicates")
        );
        assert_eq!(
            detect(Path::new("list.sha256"), tagged, Some(Algorithm::Md5))
                .unwrap()
                .0,
            Algorithm::Md5
        );
        assert!(
            detect(
                Path::new("list"),
                &format!("{tagged}SHA256 (a) = {ABC}\n"),
                None
            )
            .is_err()
        );
        assert!(
            detect(
                Path::new("list"),
                &format!("SHA256 (a) = {ABC}\ngarbage\n"),
                None
            )
            .is_err()
        );
        assert!(detect(Path::new("list"), "# comments only\n", None).is_err());
        assert!(
            detect(
                Path::new("list"),
                &format!("SHA3-256 (a) = {}", "0".repeat(64)),
                None
            )
            .is_err()
        );
        for (algorithm, width) in [
            (Algorithm::Md5, 32),
            (Algorithm::Sha1, 40),
            (Algorithm::Sha384, 96),
            (Algorithm::Sha512, 128),
        ] {
            assert_eq!(
                detect(
                    Path::new("list"),
                    &format!("{} *a", "0".repeat(width)),
                    None
                )
                .unwrap()
                .0,
                algorithm
            );
        }
        assert_eq!(
            detect(Path::new("list"), "; comment\na 352441C2\n", None)
                .unwrap()
                .0,
            Algorithm::Crc32
        );
    }

    #[test]
    fn auto_worker_reports_detected_algorithm_and_refuses_bad_lists_before_hashing_targets() {
        let dir = TempDir::new();
        dir.file("a (space) = ü", "abc");
        let manifest = dir.file("list.checksums", "MD5 (a (space) = ü) = 900150983cd24fb0d6963f7d28e17f72\nMD5 (missing) = 900150983cd24fb0d6963f7d28e17f72\n");
        let completed = AtomicUsize::new(0);
        let total = AtomicUsize::new(0);
        let report = verify(&manifest, None, &|| false, &completed, &total);
        assert_eq!(report.algorithm, Some(Algorithm::Md5));
        assert_eq!(report.rows[0].name, "a (space) = ü");
        assert_eq!(report.rows[0].status, Status::Match);
        assert!(matches!(report.rows[1].status, Status::Error(_)));
        assert!(report.text().starts_with("MD5 checksum list"));
        std::fs::write(&manifest, format!("{ABC}  a\n")).unwrap();
        completed.store(0, Ordering::Release);
        total.store(0, Ordering::Release);
        let report = verify(&manifest, None, &|| false, &completed, &total);
        assert!(report.error.unwrap().contains("Ambiguous"));
        assert!(report.rows.is_empty() && report.algorithm.is_none());
        assert_eq!(completed.load(Ordering::Acquire), 0);
        assert_eq!(total.load(Ordering::Acquire), 0);
        std::fs::write(&manifest, format!("SHA256 (../outside) = {ABC}\n")).unwrap();
        let report = verify(&manifest, None, &|| false, &completed, &total);
        assert!(matches!(report.rows[0].status, Status::Error(_)));
        assert!(report.rows[0].actual.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn openssl_tagged_files_are_detected_and_verified_without_extension_hints() {
        let dir = TempDir::new();
        let file = dir.file("a (space) = ü", "abc");
        for (argument, algorithm) in [
            ("-sha256", Algorithm::Sha256),
            ("-sha384", Algorithm::Sha384),
            ("-sha512", Algorithm::Sha512),
            ("-md5", Algorithm::Md5),
            ("-sha1", Algorithm::Sha1),
        ] {
            let output = std::process::Command::new("/usr/bin/openssl")
                .args(["dgst", argument])
                .arg(&file)
                .output()
                .unwrap();
            assert!(output.status.success());
            let manifest = dir.file(
                "openssl.checksums",
                &String::from_utf8(output.stdout).unwrap(),
            );
            let report = verify(
                &manifest,
                None,
                &|| false,
                &AtomicUsize::new(0),
                &AtomicUsize::new(0),
            );
            assert_eq!(report.algorithm, Some(algorithm));
            assert_eq!(report.rows[0].status, Status::Match, "{:?}", report.error);
        }
    }

    #[test]
    fn rhash_bsd_lists_are_detected_and_verified_when_external_verifier_is_available() {
        let Some(tool) = std::env::var_os("COMMANDER_RHASH_VERIFY") else {
            return;
        };
        let dir = TempDir::new();
        let file = dir.file("a (space) = ü", "abc");
        for (argument, algorithm) in [
            ("--sha256", Algorithm::Sha256),
            ("--sha384", Algorithm::Sha384),
            ("--sha512", Algorithm::Sha512),
            ("--md5", Algorithm::Md5),
            ("--blake3", Algorithm::Blake3),
            ("--crc32", Algorithm::Crc32),
        ] {
            let output = std::process::Command::new(&tool)
                .args(["--bsd", argument])
                .arg(&file)
                .output()
                .unwrap();
            assert!(output.status.success());
            let manifest = dir.file(
                "rhash.checksums",
                &String::from_utf8(output.stdout).unwrap(),
            );
            let report = verify(
                &manifest,
                None,
                &|| false,
                &AtomicUsize::new(0),
                &AtomicUsize::new(0),
            );
            assert_eq!(report.algorithm, Some(algorithm), "{:?}", report.error);
            assert_eq!(report.rows[0].status, Status::Match);
        }
    }

    #[test]
    fn exported_manifests_round_trip_all_algorithms_absolute_and_relative() {
        let dir = TempDir::new();
        let regular = [
            dir.file("abc", "abc"),
            dir.file("nested/with space ", ""),
            dir.file("-", "123456789"),
            dir.file(";comment", "abc"),
        ];
        let escaped = [
            dir.file("line\nname", "abc"),
            dir.file("back\\slash", "abc"),
            dir.file("return\rname", "abc"),
        ];
        for algorithm in [
            Algorithm::Sha256,
            Algorithm::Sha384,
            Algorithm::Sha512,
            Algorithm::Blake3,
            Algorithm::Crc32,
            Algorithm::Md5,
        ] {
            let mut paths = regular.to_vec();
            if algorithm != Algorithm::Crc32 {
                paths.extend(escaped.clone());
            }
            let hashes = crate::checksum::verify_paths(paths.clone());
            for relative in [false, true] {
                let list = hashes
                    .checksum_list_at(algorithm, relative.then_some(dir.path()))
                    .unwrap();
                let manifest = dir.file("manifest", &list);
                let report = run(&manifest, algorithm);
                assert!(report.error.is_none(), "{algorithm:?}: {:?}", report.error);
                assert_eq!(report.total, paths.len());
                assert!(
                    report.rows.iter().all(|row| row.status == Status::Match),
                    "{algorithm:?}: {:?}",
                    report.rows
                );
            }
        }
    }

    #[test]
    fn verified_rows_distinguish_match_mismatch_missing_and_rerun_changes() {
        let dir = TempDir::new();
        dir.file("good", "abc");
        let bad = dir.file("bad", "changed");
        let manifest = dir.file(
            "list",
            &format!("{} *good\n{ABC} *bad\n{ABC} *missing\n", ABC.to_uppercase()),
        );
        let report = run(&manifest, Algorithm::Sha256);
        assert_eq!(report.rows[0].status, Status::Match);
        assert_eq!(report.rows[1].status, Status::Mismatch);
        assert!(matches!(report.rows[2].status, Status::Error(_)));
        assert_eq!(report.rows[2].actual, None);
        assert_eq!(
            report.summary(),
            "Verified 3 of 3 records: 1 match, 1 mismatch, 1 failed"
        );
        assert!(report.text().contains("MATCH good expected="));
        assert!(report.text().contains("MISMATCH bad expected="));
        std::fs::write(bad, "abc").unwrap();
        assert_eq!(
            run(&manifest, Algorithm::Sha256).rows[1].status,
            Status::Match
        );
        assert!(run(&manifest, Algorithm::Md5).error.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn parent_outside_absolute_and_symbolic_paths_are_refused() {
        let dir = TempDir::new();
        let outside = TempDir::new();
        let secret = outside.file("outside", "abc");
        std::os::unix::fs::symlink(&secret, dir.path().join("file-link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("dir-link")).unwrap();
        let manifest = dir.file(
            "list",
            &format!(
                "{ABC} *../outside\n{ABC} *{}\n{ABC} *file-link\n{ABC} *dir-link/outside\n",
                secret.display()
            ),
        );
        let report = run(&manifest, Algorithm::Sha256);
        assert_eq!(report.rows.len(), 4);
        assert!(
            report
                .rows
                .iter()
                .all(|row| matches!(row.status, Status::Error(_)) && row.actual.is_none())
        );
        std::os::unix::fs::symlink(&manifest, dir.path().join("list-link")).unwrap();
        assert!(
            run(&dir.path().join("list-link"), Algorithm::Sha256)
                .error
                .is_some()
        );
        assert_eq!(std::fs::read(secret).unwrap(), b"abc");
    }

    #[test]
    fn limits_parse_failure_and_cancellation_never_claim_unprocessed_records() {
        let dir = TempDir::new();
        let payload = dir.file("a", "abc");
        let invalid = dir.file("invalid", &format!("{ABC} *a\ngarbage\n"));
        let report = run(&invalid, Algorithm::Sha256);
        assert!(report.error.unwrap().contains("line 2"));
        assert!(report.rows.is_empty());
        let large = dir.file("large", &"x".repeat(MAX_BYTES + 1));
        assert!(run(&large, Algorithm::Sha256).error.is_some());
        assert!(
            parse(
                &format!("{ABC} *a\n").repeat(MAX_RECORDS + 1),
                Algorithm::Sha256
            )
            .is_err()
        );
        let manifest = dir.file("list", &format!("{ABC} *a\n").repeat(3));
        let completed = AtomicUsize::new(0);
        let total = AtomicUsize::new(0);
        let report = verify(
            &manifest,
            Some(Algorithm::Sha256),
            &|| completed.load(Ordering::Acquire) >= 1,
            &completed,
            &total,
        );
        assert!(report.cancelled);
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.total, 3);
        assert_eq!(total.load(Ordering::Acquire), 3);
        assert_eq!(report.rows[0].status, Status::Match);
        assert!(report.summary().starts_with("Stopped: 1 of 3"));
        let report = verify(
            &dir.path().join("nonexistent"),
            Some(Algorithm::Sha256),
            &|| true,
            &AtomicUsize::new(0),
            &AtomicUsize::new(0),
        );
        assert!(report.cancelled && report.error.is_none() && report.rows.is_empty());
        assert_eq!(std::fs::read(payload).unwrap(), b"abc");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn accepts_system_shasum_generated_manifest_and_detects_corruption() {
        let dir = TempDir::new();
        let file = dir.file("external checksum", "abc");
        let output = std::process::Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(&file)
            .output()
            .unwrap();
        assert!(output.status.success());
        let manifest = dir.file(
            "external.sha256",
            &String::from_utf8(output.stdout).unwrap(),
        );
        assert_eq!(
            run(&manifest, Algorithm::Sha256).rows[0].status,
            Status::Match
        );
        std::fs::write(file, "corrupted").unwrap();
        assert_eq!(
            run(&manifest, Algorithm::Sha256).rows[0].status,
            Status::Mismatch
        );
    }
}
