use super::*;
use crate::testutil::TempDir;
use std::io::Write;

#[test]
fn aes_empty_password_is_preserved_by_worker() {
    let temp = TempDir::new();
    let path = temp.path().join("empty-password.zipx");
    fs::write(
        &path,
        include_bytes!(
            "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-empty-password.zipx"
        ),
    )
    .unwrap();
    let dest = temp.path().join("output");
    let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
    assert!(!report.errors.is_empty());
    assert!(!dest.join("payload.txt").exists());
    let run = start_extract(
        path,
        dest.clone(),
        vec![0],
        Some(zeroize::Zeroizing::new(String::new())),
        Arc::new(|| {}),
    );
    let report = run
        .receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.extracted, 1);
    assert_eq!(
        fs::read(dest.join("payload.txt")).unwrap(),
        include_bytes!(
            "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-empty-password.txt"
        )
    );
}

#[test]
fn aes_standard_codecs_reject_bad_tags_and_underfilled_declared_sizes() {
    for (name, bytes) in [
        (
            "aes-stored-bad-tag",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-bad-tag.zip"
            )
            .as_slice(),
        ),
        (
            "aes-stored-bad-size",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-bad-size.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-bad-tag",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-bad-tag.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-bad-size",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-bad-size.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-bad-tag",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-bad-tag.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-bad-size",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-bad-size.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-bad-tag",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-bad-tag.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-bad-size",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-bad-size.zip"
            )
            .as_slice(),
        ),
    ] {
        let temp = TempDir::new();
        let path = temp.path().join("input.zip");
        fs::write(&path, bytes).unwrap();
        let dest = temp.path().join("output");
        let report = extract_members_with_password(
            &path,
            &dest,
            &[0],
            &AtomicBool::new(false),
            Some(b"fixture-password"),
        )
        .unwrap();
        assert!(!report.errors.is_empty(), "{name}: unexpectedly accepted");
        assert!(
            !dest.join("payload.txt").exists(),
            "{name}: partial file retained"
        );
    }
}

#[test]
fn aes_directories_are_authenticated_before_creation() {
    for (bytes, valid) in [
        (
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-directory.zipx"
            )
            .as_slice(),
            true,
        ),
        (
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-directory-bad-tag.zipx"
            )
            .as_slice(),
            false,
        ),
    ] {
        let temp = TempDir::new();
        let path = temp.path().join("directory.zipx");
        fs::write(&path, bytes).unwrap();
        let dest = temp.path().join("output");
        let report = extract_members_with_password(
            &path,
            &dest,
            &[0],
            &AtomicBool::new(false),
            Some(b"fixture-password"),
        )
        .unwrap();
        assert_eq!(
            dest.join("empty-dir1").is_dir(),
            valid,
            "{:?}",
            report.errors
        );
        assert_eq!(report.errors.is_empty(), valid);
        assert_eq!(report.skipped_dirs, usize::from(valid));
    }
}
#[cfg(unix)]
#[test]
fn aes_symlinks_authenticate_and_preserve_target_confinement() {
    for (name, bytes, valid) in [
        (
            "safe",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-symlink-safe.zipx"
            )
            .as_slice(),
            true,
        ),
        (
            "escape",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-symlink-escape.zipx"
            )
            .as_slice(),
            false,
        ),
        (
            "tag",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-symlink-bad-tag.zipx"
            )
            .as_slice(),
            false,
        ),
    ] {
        let temp = TempDir::new();
        let path = temp.path().join("symlink.zipx");
        fs::write(&path, bytes).unwrap();
        let dest = temp.path().join("output");
        let report = extract_members_with_password(
            &path,
            &dest,
            &[0],
            &AtomicBool::new(false),
            Some(b"fixture-password"),
        )
        .unwrap();
        assert_eq!(
            report.errors.is_empty(),
            valid,
            "{name}: {:?}",
            report.errors
        );
        if valid {
            assert_eq!(
                fs::read_link(dest.join("payload.txt")).unwrap(),
                Path::new("target.txt")
            );
        } else {
            assert!(
                fs::symlink_metadata(dest.join("payload.txt")).is_err(),
                "{name}"
            );
        }
    }
}

#[test]
fn aes_standard_codecs_extract_all_strengths_and_custom_errors_cleanup() {
    let expected =
        include_bytes!("../../docs/qualification/zip-password-2026-11-03/fixtures/expected.txt");
    for (name, bytes) in [
        (
            "aes-stored-128",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-128.zip"
            )
            .as_slice(),
        ),
        (
            "aes-stored-192",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-192.zip"
            )
            .as_slice(),
        ),
        (
            "aes-stored-256",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-256.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-128",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-128.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-192",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-192.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-256",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-256.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-128",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-128.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-192",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-192.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-256",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-256.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-128",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-128.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-192",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-192.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-256",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-256.zip"
            )
            .as_slice(),
        ),
        (
            "aes-stored-128-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-128-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-stored-192-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-192-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-stored-256-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-256-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-128-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-128-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-192-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-192-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate-256-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate-256-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-128-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-128-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-192-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-192-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-bzip2-256-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bzip2-256-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-128-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-128-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-192-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-192-v1.zip"
            )
            .as_slice(),
        ),
        (
            "aes-deflate64-256-v1",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-deflate64-256-v1.zip"
            )
            .as_slice(),
        ),
    ] {
        let temp = TempDir::new();
        let path = temp.path().join("input.zip");
        fs::write(&path, bytes).unwrap();
        let dest = temp.path().join("correct");
        let cancel = AtomicBool::new(false);
        let report =
            extract_members_with_password(&path, &dest, &[0], &cancel, Some(b"fixture-password"))
                .unwrap();
        assert!(report.errors.is_empty(), "{name}: {:?}", report.errors);
        assert_eq!(report.extracted, 1);
        assert_eq!(fs::read(dest.join("payload.txt")).unwrap(), expected);
        let wrong_dest = temp.path().join("wrong");
        let report = extract_members_with_password(
            &path,
            &wrong_dest,
            &[0],
            &cancel,
            Some(b"wrong-password"),
        )
        .unwrap();
        assert!(!report.errors.is_empty(), "{name}");
        assert!(!wrong_dest.join("payload.txt").exists());
    }
    for bytes in [
        include_bytes!(
            "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-ppmd-bad-tag.zipx"
        )
        .as_slice(),
        include_bytes!(
            "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-ppmd-tail-bad-tag.zipx"
        )
        .as_slice(),
        include_bytes!(
            "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-bad-size.zipx"
        )
        .as_slice(),
    ] {
        let temp = TempDir::new();
        let path = temp.path().join("input.zipx");
        fs::write(&path, bytes).unwrap();
        let dest = temp.path().join("output");
        let report = extract_members_with_password(
            &path,
            &dest,
            &[0],
            &AtomicBool::new(false),
            Some(b"fixture-password"),
        )
        .unwrap();
        assert!(!report.errors.is_empty());
        assert!(!dest.join("payload.txt").exists());
    }
}
#[test]
fn aes_authentication_tail_observes_extraction_cancellation() {
    let cancelled = AtomicBool::new(false);
    let input = ZipExtractionInput {
        reader: std::io::Cursor::new(
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-ppmd-tail.zipx"
            )
            .as_slice(),
        ),
        cancelled: &cancelled,
    };
    let mut archive = zip::ZipArchive::new(input).unwrap();
    let mut member =
        crate::zip_reader::open_with_password(&mut archive, 0, Some(b"fixture-password")).unwrap();
    let expected = include_bytes!(
        "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-ppmd-tail.txt"
    );
    let mut decoded = vec![0; expected.len()];
    member.read_exact(&mut decoded).unwrap();
    assert_eq!(decoded, expected);
    cancelled.store(true, Ordering::Release);
    assert!(
        member
            .read(&mut [0; 1])
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
}

#[test]
fn password_worker_extracts_custom_codec_and_cleans_failed_payloads() {
    let temp = TempDir::new();
    let path = temp.path().join("ppmd.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-password-2026-11-03/fixtures/ppmd.zipx"),
    )
    .unwrap();
    let dest = temp.path().join("worker");
    let run = start_extract(
        path.clone(),
        dest.clone(),
        vec![0],
        Some(zeroize::Zeroizing::new("fixture-password".into())),
        Arc::new(|| {}),
    );
    let report = run
        .receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.extracted, 1);
    assert_eq!(
        fs::read(dest.join("payload.txt")).unwrap(),
        include_bytes!("../../docs/qualification/zip-password-2026-11-03/fixtures/ppmd.txt")
    );
    for (name, bytes) in [
        (
            "crc",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/ppmd-bad-crc.zipx"
            )
            .as_slice(),
        ),
        (
            "size",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/ppmd-bad-size.zipx"
            )
            .as_slice(),
        ),
        (
            "eos",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/ppmd-no-eos.zipx"
            )
            .as_slice(),
        ),
        (
            "truncated",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/xz-truncated.zipx"
            )
            .as_slice(),
        ),
    ] {
        fs::write(&path, bytes).unwrap();
        let dest = temp.path().join(name);
        let report = extract_members_with_password(
            &path,
            &dest,
            &[0],
            &AtomicBool::new(false),
            Some(b"fixture-password"),
        )
        .unwrap();
        assert!(!report.errors.is_empty(), "{name}");
        assert!(!dest.join("payload.txt").exists(), "{name}");
    }
}

#[test]
fn password_extraction_standard_zipcrypto_codecs() {
    let expected =
        include_bytes!("../../docs/qualification/zip-password-2026-11-03/fixtures/expected.txt");
    for (name, bytes) in [
        (
            "stored",
            include_bytes!("../../docs/qualification/zip-password-2026-11-03/fixtures/stored.zip")
                .as_slice(),
        ),
        (
            "deflate",
            include_bytes!("../../docs/qualification/zip-password-2026-11-03/fixtures/deflate.zip")
                .as_slice(),
        ),
        (
            "bzip2",
            include_bytes!("../../docs/qualification/zip-password-2026-11-03/fixtures/bzip2.zip")
                .as_slice(),
        ),
        (
            "deflate64",
            include_bytes!(
                "../../docs/qualification/zip-password-2026-11-03/fixtures/deflate64.zip"
            )
            .as_slice(),
        ),
    ] {
        let temp = TempDir::new();
        let path = temp.path().join(format!("{name}.zip"));
        fs::write(&path, bytes).unwrap();
        let cancel = AtomicBool::new(false);
        for (attempt, password) in [
            ("missing", None),
            ("wrong", Some(b"wrong-fixture-secret".as_slice())),
        ] {
            let dest = temp.path().join(attempt);
            let report =
                extract_members_with_password(&path, &dest, &[0], &cancel, password).unwrap();
            assert!(!report.errors.is_empty(), "{name}/{attempt}");
            assert!(!dest.join("payload.txt").exists(), "{name}/{attempt}");
            assert!(!format!("{report:?}").contains("wrong-fixture-secret"));
        }
        let dest = temp.path().join("correct");
        let report =
            extract_members_with_password(&path, &dest, &[0], &cancel, Some(b"fixture-password"))
                .unwrap();
        assert!(report.errors.is_empty(), "{name}: {:?}", report.errors);
        assert_eq!(fs::read(dest.join("payload.txt")).unwrap(), expected);
        let report =
            extract_members_with_password(&path, &dest, &[0], &cancel, Some(b"fixture-password"))
                .unwrap();
        assert!(report.errors.is_empty());
        assert_eq!(report.skipped_existing, 1);
        assert_eq!(report.extracted, 0);
        assert_eq!(fs::read(dest.join("payload.txt")).unwrap(), expected);
    }
}

fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn ppmd_zipx_reads_restart_cutoff_and_arena_boundary() {
    let temp = TempDir::new();
    let cancel = AtomicBool::new(false);
    let payload =
        include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/expected.txt");
    for (name, bytes, count) in [
        (
            "normal",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/normal.zipx")
                .as_slice(),
            3,
        ),
        (
            "limit-arena",
            include_bytes!(
                "../../docs/qualification/zip-ppmd-2026-11-03/fixtures/limit-arena.zipx"
            )
            .as_slice(),
            1,
        ),
        (
            "cutoff",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/cutoff.zipx")
                .as_slice(),
            1,
        ),
    ] {
        let path = temp.path().join(format!("{name}.ZIPX"));
        fs::write(&path, bytes).unwrap();
        let listing = list_archive(path.clone(), &cancel).unwrap();
        assert_eq!(listing.members.len(), count);
        assert!(
            listing
                .members
                .iter()
                .all(|m| m.zip_info.as_ref().unwrap().compression.eq("Ppmd"))
        );
        let mut contents = std::collections::BTreeMap::new();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                contents.insert(m.member.path, m.content.unwrap().as_bytes().to_vec());
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 0);
        assert_eq!(summary.emitted, count);
        assert_eq!(contents[Path::new("payload.txt")], payload.to_vec());
        if name == "normal" {
            assert_eq!(contents[Path::new("nested/ёж.txt")], payload.repeat(3));
            assert!(contents[Path::new("empty.txt")].is_empty());
        }
        let check =
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .unwrap();
        assert_eq!(check.members, count);
        assert_eq!(
            check.decoded,
            contents.values().map(|x| x.len() as u64).sum::<u64>()
        );
        let dest = temp.path().join(name);
        let indexes: Vec<_> = listing.members.iter().map(|m| m.index).collect();
        let report = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(report.extracted, count);
        assert!(report.errors.is_empty());
        for (file, data) in contents {
            assert_eq!(fs::read(dest.join(file)).unwrap(), data);
        }
        fs::write(dest.join("payload.txt"), b"keep").unwrap();
        let repeat = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(repeat.skipped_existing, count);
        assert_eq!(fs::read(dest.join("payload.txt")).unwrap(), b"keep");
    }
}

#[test]
fn ppmd_zipx_refuses_bad_parameters_and_missing_end_marker() {
    let temp = TempDir::new();
    for (name, bytes) in [
        (
            "encrypted",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/encrypted.zipx")
                .as_slice(),
        ),
        (
            "crc",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/bad-crc.zipx")
                .as_slice(),
        ),
        (
            "size",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/bad-size.zipx")
                .as_slice(),
        ),
        (
            "truncated",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/truncated.zipx")
                .as_slice(),
        ),
        (
            "short-header",
            include_bytes!(
                "../../docs/qualification/zip-ppmd-2026-11-03/fixtures/short-header.zipx"
            )
            .as_slice(),
        ),
        (
            "memory",
            include_bytes!(
                "../../docs/qualification/zip-ppmd-2026-11-03/fixtures/large-arena.zipx"
            )
            .as_slice(),
        ),
        (
            "order",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/bad-order.zipx")
                .as_slice(),
        ),
        (
            "restore",
            include_bytes!(
                "../../docs/qualification/zip-ppmd-2026-11-03/fixtures/bad-restore.zipx"
            )
            .as_slice(),
        ),
        (
            "freeze",
            include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/freeze.zipx")
                .as_slice(),
        ),
        (
            "eos",
            include_bytes!(
                "../../docs/qualification/zip-ppmd-2026-11-03/fixtures/missing-eos.zipx"
            )
            .as_slice(),
        ),
    ] {
        let path = temp.path().join(format!("{name}.zipx"));
        fs::write(&path, bytes).unwrap();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                assert!(m.content.is_none(), "{name}");
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 1, "{name}");
        assert!(
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .is_err(),
            "{name}"
        );
        let dest = temp.path().join(name);
        let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
        assert_eq!(report.extracted, 0, "{name}");
        assert_eq!(report.errors.len(), 1, "{name}");
        assert!(!dest.join("payload.txt").exists(), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn ppmd_zipx_extracts_only_symlinks_that_stay_inside_destination() {
    let temp = TempDir::new();
    let path = temp.path().join("symlinks.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/symlinks.zipx"),
    )
    .unwrap();
    let dest = temp.path().join("out");
    let report = extract_members(&path, &dest, &[0, 1, 2], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.extracted, 2);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(
        fs::read_link(dest.join("link-safe")).unwrap(),
        Path::new("target.txt")
    );
    assert_eq!(
        fs::read(dest.join("link-safe")).unwrap(),
        include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/expected.txt")
    );
    assert!(fs::symlink_metadata(dest.join("link-escape")).is_err());
}

#[test]
fn ppmd_zipx_keeps_path_ratio_and_cancellation_guards() {
    let temp = TempDir::new();
    let path = temp.path().join("unsafe.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/unsafe.zipx"),
    )
    .unwrap();
    let listing = list_archive(path.clone(), &AtomicBool::new(false)).unwrap();
    assert_eq!(listing.unsafe_members, 1);
    assert_eq!(listing.members.len(), 1);
    let report = extract_members(
        &path,
        &temp.path().join("out"),
        &[0, 1],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.extracted, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(!temp.path().join("escape.txt").exists());
    let ratio = temp.path().join("ratio.zipx");
    fs::write(
        &ratio,
        include_bytes!("../../docs/qualification/zip-ppmd-2026-11-03/fixtures/ratio.zipx"),
    )
    .unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &ratio,
        true,
        &mut budget,
        || false,
        |m| {
            assert!(m.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 0);
    assert!(
        visit_members(
            &ratio,
            true,
            &mut budget,
            || true,
            |_| panic!("cancelled visit")
        )
        .unwrap()
        .cancelled
    );
}

#[test]
fn zstd_zipx_reads_checks_and_concatenated_streams() {
    let temp = TempDir::new();
    let cancel = AtomicBool::new(false);
    let payload =
        include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/expected.txt");
    for (name, bytes, count) in [
        (
            "legacy",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/legacy.zipx")
                .as_slice(),
            1,
        ),
        (
            "limit-window",
            include_bytes!(
                "../../docs/qualification/zip-zstd-2026-11-03/fixtures/limit-window.zipx"
            )
            .as_slice(),
            1,
        ),
        (
            "normal",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/normal.zipx")
                .as_slice(),
            4,
        ),
        (
            "concatenated",
            include_bytes!(
                "../../docs/qualification/zip-zstd-2026-11-03/fixtures/concatenated.zipx"
            )
            .as_slice(),
            1,
        ),
    ] {
        let path = temp.path().join(format!("{name}.ZIPX"));
        fs::write(&path, bytes).unwrap();
        let listing = list_archive(path.clone(), &cancel).unwrap();
        assert_eq!(listing.members.len(), count);
        assert!(
            listing.members.iter().all(|m| m
                .zip_info
                .as_ref()
                .unwrap()
                .compression
                .starts_with("Zstd"))
        );
        let mut contents = std::collections::BTreeMap::new();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                contents.insert(m.member.path, m.content.unwrap().as_bytes().to_vec());
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 0);
        assert_eq!(summary.emitted, count);
        assert_eq!(
            contents[Path::new("payload.txt")],
            if name == "concatenated" {
                payload.repeat(3)
            } else {
                payload.to_vec()
            }
        );
        if name == "normal" {
            assert_eq!(contents[Path::new("nested/ёж.txt")], payload.repeat(3));
            assert!(contents[Path::new("empty.txt")].is_empty());
        }
        let check =
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .unwrap();
        assert_eq!(check.members, count);
        assert_eq!(
            check.decoded,
            contents.values().map(|x| x.len() as u64).sum::<u64>()
        );
        let dest = temp.path().join(name);
        let indexes: Vec<_> = listing.members.iter().map(|m| m.index).collect();
        let report = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(report.extracted, count);
        assert!(report.errors.is_empty());
        for (file, data) in contents {
            assert_eq!(fs::read(dest.join(file)).unwrap(), data);
        }
        fs::write(dest.join("payload.txt"), b"keep").unwrap();
        let repeat = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(repeat.skipped_existing, count);
        assert_eq!(fs::read(dest.join("payload.txt")).unwrap(), b"keep");
    }
}

#[test]
fn zstd_zipx_refuses_bad_streams_and_later_dictionary_requests() {
    let temp = TempDir::new();
    for (name, bytes) in [
        (
            "legacy-window",
            include_bytes!(
                "../../docs/qualification/zip-zstd-2026-11-03/fixtures/legacy-window.zipx"
            )
            .as_slice(),
        ),
        (
            "encrypted",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/encrypted.zipx")
                .as_slice(),
        ),
        (
            "crc",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/bad-crc.zipx")
                .as_slice(),
        ),
        (
            "size",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/bad-size.zipx")
                .as_slice(),
        ),
        (
            "truncated",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/truncated.zipx")
                .as_slice(),
        ),
        (
            "window",
            include_bytes!(
                "../../docs/qualification/zip-zstd-2026-11-03/fixtures/huge-window.zipx"
            )
            .as_slice(),
        ),
        (
            "later-window",
            include_bytes!(
                "../../docs/qualification/zip-zstd-2026-11-03/fixtures/later-window.zipx"
            )
            .as_slice(),
        ),
        (
            "trailing",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/trailing.zipx")
                .as_slice(),
        ),
        (
            "check",
            include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/bad-check.zipx")
                .as_slice(),
        ),
    ] {
        let path = temp.path().join(format!("{name}.zipx"));
        fs::write(&path, bytes).unwrap();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                assert!(m.content.is_none(), "{name}");
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 1, "{name}");
        assert!(
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .is_err(),
            "{name}"
        );
        let dest = temp.path().join(name);
        let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
        assert_eq!(report.extracted, 0, "{name}");
        assert_eq!(report.errors.len(), 1, "{name}");
        assert!(!dest.join("payload.txt").exists(), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn zstd_zipx_extracts_only_symlinks_that_stay_inside_destination() {
    let temp = TempDir::new();
    let path = temp.path().join("symlinks.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/symlinks.zipx"),
    )
    .unwrap();
    let dest = temp.path().join("out");
    let report = extract_members(&path, &dest, &[0, 1, 2], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.extracted, 2);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(
        fs::read_link(dest.join("link-safe")).unwrap(),
        Path::new("target.txt")
    );
    assert_eq!(
        fs::read(dest.join("link-safe")).unwrap(),
        include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/expected.txt")
    );
    assert!(fs::symlink_metadata(dest.join("link-escape")).is_err());
}

#[test]
fn zstd_zipx_keeps_path_ratio_and_cancellation_guards() {
    let temp = TempDir::new();
    let path = temp.path().join("unsafe.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/unsafe.zipx"),
    )
    .unwrap();
    let listing = list_archive(path.clone(), &AtomicBool::new(false)).unwrap();
    assert_eq!(listing.unsafe_members, 1);
    assert_eq!(listing.members.len(), 1);
    let report = extract_members(
        &path,
        &temp.path().join("out"),
        &[0, 1],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.extracted, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(!temp.path().join("escape.txt").exists());
    let skip = temp.path().join("skip.zipx");
    fs::write(
        &skip,
        include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/skippable.zipx"),
    )
    .unwrap();
    let mut empty_budget = SearchBudget::default();
    let empty = visit_members(
        &skip,
        true,
        &mut empty_budget,
        || false,
        |m| {
            assert_eq!(m.content.as_deref(), Some(""));
            true
        },
    )
    .unwrap();
    assert_eq!(empty.content_skipped, 0);
    assert_eq!(empty.emitted, 1);
    assert_eq!(
        crate::archive_check::check(&skip, &|| false, &std::sync::atomic::AtomicU64::new(0))
            .unwrap()
            .decoded,
        0
    );
    let ratio = temp.path().join("ratio.zipx");
    fs::write(
        &ratio,
        include_bytes!("../../docs/qualification/zip-zstd-2026-11-03/fixtures/ratio.zipx"),
    )
    .unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &ratio,
        true,
        &mut budget,
        || false,
        |m| {
            assert!(m.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 0);
    assert!(
        visit_members(
            &ratio,
            true,
            &mut budget,
            || true,
            |_| panic!("cancelled visit")
        )
        .unwrap()
        .cancelled
    );
}

#[test]
fn xz_zipx_reads_checks_and_concatenated_streams() {
    let temp = TempDir::new();
    let cancel = AtomicBool::new(false);
    let payload =
        include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/expected.txt");
    for (name, bytes, count) in [
        (
            "normal",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/normal.zipx")
                .as_slice(),
            4,
        ),
        (
            "concatenated",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/concatenated.zipx")
                .as_slice(),
            1,
        ),
    ] {
        let path = temp.path().join(format!("{name}.ZIPX"));
        fs::write(&path, bytes).unwrap();
        let listing = list_archive(path.clone(), &cancel).unwrap();
        assert_eq!(listing.members.len(), count);
        assert!(
            listing
                .members
                .iter()
                .all(|m| m.zip_info.as_ref().unwrap().compression == "Xz")
        );
        let mut contents = std::collections::BTreeMap::new();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                contents.insert(m.member.path, m.content.unwrap().as_bytes().to_vec());
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 0);
        assert_eq!(summary.emitted, count);
        assert_eq!(
            contents[Path::new("payload.txt")],
            if name == "normal" {
                payload.to_vec()
            } else {
                payload.repeat(3)
            }
        );
        if name == "normal" {
            assert_eq!(contents[Path::new("nested/ёж.txt")], payload.repeat(3));
            assert!(contents[Path::new("empty.txt")].is_empty());
        }
        let check =
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .unwrap();
        assert_eq!(check.members, count);
        assert_eq!(
            check.decoded,
            contents.values().map(|x| x.len() as u64).sum::<u64>()
        );
        let dest = temp.path().join(name);
        let indexes: Vec<_> = listing.members.iter().map(|m| m.index).collect();
        let report = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(report.extracted, count);
        assert!(report.errors.is_empty());
        for (file, data) in contents {
            assert_eq!(fs::read(dest.join(file)).unwrap(), data);
        }
        fs::write(dest.join("payload.txt"), b"keep").unwrap();
        let repeat = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(repeat.skipped_existing, count);
        assert_eq!(fs::read(dest.join("payload.txt")).unwrap(), b"keep");
    }
}

#[test]
fn xz_zipx_refuses_bad_streams_and_later_dictionary_requests() {
    let temp = TempDir::new();
    for (name, bytes) in [
        (
            "encrypted",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/encrypted.zipx")
                .as_slice(),
        ),
        (
            "crc",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/bad-crc.zipx")
                .as_slice(),
        ),
        (
            "size",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/bad-size.zipx")
                .as_slice(),
        ),
        (
            "truncated",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/truncated.zipx")
                .as_slice(),
        ),
        (
            "memory",
            include_bytes!(
                "../../docs/qualification/zip-xz-2026-11-03/fixtures/huge-dictionary.zipx"
            )
            .as_slice(),
        ),
        (
            "later-block",
            include_bytes!(
                "../../docs/qualification/zip-xz-2026-11-03/fixtures/later-block-memory.zipx"
            )
            .as_slice(),
        ),
        (
            "later-stream",
            include_bytes!(
                "../../docs/qualification/zip-xz-2026-11-03/fixtures/later-stream-memory.zipx"
            )
            .as_slice(),
        ),
        (
            "trailing",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/trailing.zipx")
                .as_slice(),
        ),
        (
            "check",
            include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/bad-check.zipx")
                .as_slice(),
        ),
    ] {
        let path = temp.path().join(format!("{name}.zipx"));
        fs::write(&path, bytes).unwrap();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                assert!(m.content.is_none(), "{name}");
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 1, "{name}");
        assert!(
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .is_err(),
            "{name}"
        );
        let dest = temp.path().join(name);
        let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
        assert_eq!(report.extracted, 0, "{name}");
        assert_eq!(report.errors.len(), 1, "{name}");
        assert!(!dest.join("payload.txt").exists(), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn xz_zipx_extracts_only_symlinks_that_stay_inside_destination() {
    let temp = TempDir::new();
    let path = temp.path().join("symlinks.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/symlinks.zipx"),
    )
    .unwrap();
    let dest = temp.path().join("out");
    let report = extract_members(&path, &dest, &[0, 1, 2], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.extracted, 2);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(
        fs::read_link(dest.join("link-safe")).unwrap(),
        Path::new("target.txt")
    );
    assert_eq!(
        fs::read(dest.join("link-safe")).unwrap(),
        include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/expected.txt")
    );
    assert!(fs::symlink_metadata(dest.join("link-escape")).is_err());
}

#[test]
fn xz_zipx_keeps_path_ratio_and_cancellation_guards() {
    let temp = TempDir::new();
    let path = temp.path().join("unsafe.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/unsafe.zipx"),
    )
    .unwrap();
    let listing = list_archive(path.clone(), &AtomicBool::new(false)).unwrap();
    assert_eq!(listing.unsafe_members, 1);
    assert_eq!(listing.members.len(), 1);
    let report = extract_members(
        &path,
        &temp.path().join("out"),
        &[0, 1],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.extracted, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(!temp.path().join("escape.txt").exists());
    let ratio = temp.path().join("ratio.zipx");
    fs::write(
        &ratio,
        include_bytes!("../../docs/qualification/zip-xz-2026-11-03/fixtures/ratio.zipx"),
    )
    .unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &ratio,
        true,
        &mut budget,
        || false,
        |m| {
            assert!(m.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 0);
    assert!(
        visit_members(
            &ratio,
            true,
            &mut budget,
            || true,
            |_| panic!("cancelled visit")
        )
        .unwrap()
        .cancelled
    );
}

#[test]
fn lzma_zipx_reads_eos_and_known_size_without_eos() {
    let temp = TempDir::new();
    let cancel = AtomicBool::new(false);
    let payload =
        include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/expected.txt");
    for (name, bytes, count) in [
        (
            "normal",
            include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/normal.zipx")
                .as_slice(),
            3,
        ),
        (
            "no-eos",
            include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/no-eos.zipx")
                .as_slice(),
            1,
        ),
    ] {
        let path = temp.path().join(format!("{name}.ZIPX"));
        fs::write(&path, bytes).unwrap();
        let listing = list_archive(path.clone(), &cancel).unwrap();
        assert_eq!(listing.members.len(), count);
        assert!(
            listing
                .members
                .iter()
                .all(|m| m.zip_info.as_ref().unwrap().compression == "Lzma")
        );
        let mut contents = std::collections::BTreeMap::new();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                contents.insert(m.member.path, m.content.unwrap().as_bytes().to_vec());
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 0);
        assert_eq!(summary.emitted, count);
        assert_eq!(contents[Path::new("payload.txt")], payload.as_slice());
        if name == "normal" {
            assert_eq!(contents[Path::new("nested/ёж.txt")], payload.repeat(3));
            assert!(contents[Path::new("empty.txt")].is_empty());
        }
        let check =
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .unwrap();
        assert_eq!(check.members, count);
        assert_eq!(
            check.decoded,
            contents.values().map(|x| x.len() as u64).sum::<u64>()
        );
        let dest = temp.path().join(name);
        let indexes: Vec<_> = listing.members.iter().map(|m| m.index).collect();
        let report = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(report.extracted, count);
        assert!(report.errors.is_empty());
        for (file, data) in contents {
            assert_eq!(fs::read(dest.join(file)).unwrap(), data);
        }
        fs::write(dest.join("payload.txt"), b"keep").unwrap();
        let repeat = extract_members(&path, &dest, &indexes, &cancel).unwrap();
        assert_eq!(repeat.skipped_existing, count);
        assert_eq!(fs::read(dest.join("payload.txt")).unwrap(), b"keep");
    }
}

#[test]
fn lzma_zipx_refuses_bad_streams_and_memory_requests_without_partial_output() {
    let temp = TempDir::new();
    for (name, bytes) in [
        (
            "encrypted",
            include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/encrypted.zipx")
                .as_slice(),
        ),
        (
            "crc",
            include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/bad-crc.zipx")
                .as_slice(),
        ),
        (
            "size",
            include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/bad-size.zipx")
                .as_slice(),
        ),
        (
            "truncated",
            include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/truncated.zipx")
                .as_slice(),
        ),
        (
            "dictionary",
            include_bytes!(
                "../../docs/qualification/zip-lzma-2026-11-03/fixtures/huge-dictionary.zipx"
            )
            .as_slice(),
        ),
        (
            "properties",
            include_bytes!(
                "../../docs/qualification/zip-lzma-2026-11-03/fixtures/bad-properties.zipx"
            )
            .as_slice(),
        ),
        (
            "memory",
            include_bytes!(
                "../../docs/qualification/zip-lzma-2026-11-03/fixtures/memory-limit.zipx"
            )
            .as_slice(),
        ),
        (
            "eos",
            include_bytes!(
                "../../docs/qualification/zip-lzma-2026-11-03/fixtures/missing-required-eos.zipx"
            )
            .as_slice(),
        ),
    ] {
        let path = temp.path().join(format!("{name}.zipx"));
        fs::write(&path, bytes).unwrap();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                assert!(m.content.is_none(), "{name}");
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 1, "{name}");
        assert!(
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .is_err(),
            "{name}"
        );
        let dest = temp.path().join(name);
        let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
        assert_eq!(report.extracted, 0, "{name}");
        assert_eq!(report.errors.len(), 1, "{name}");
        assert!(!dest.join("payload.txt").exists(), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn lzma_zipx_extracts_only_symlinks_that_stay_inside_destination() {
    let temp = TempDir::new();
    let path = temp.path().join("symlinks.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/symlinks.zipx"),
    )
    .unwrap();
    let dest = temp.path().join("out");
    let report = extract_members(&path, &dest, &[0, 1, 2], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.extracted, 2);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(
        fs::read_link(dest.join("link-safe")).unwrap(),
        Path::new("target.txt")
    );
    assert_eq!(
        fs::read(dest.join("link-safe")).unwrap(),
        include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/expected.txt")
    );
    assert!(fs::symlink_metadata(dest.join("link-escape")).is_err());
}

#[test]
fn lzma_zipx_keeps_path_ratio_and_cancellation_guards() {
    let temp = TempDir::new();
    let path = temp.path().join("unsafe.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/unsafe.zipx"),
    )
    .unwrap();
    let listing = list_archive(path.clone(), &AtomicBool::new(false)).unwrap();
    assert_eq!(listing.unsafe_members, 1);
    assert_eq!(listing.members.len(), 1);
    let report = extract_members(
        &path,
        &temp.path().join("out"),
        &[0, 1],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.extracted, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(!temp.path().join("escape.txt").exists());
    let ratio = temp.path().join("ratio.zipx");
    fs::write(
        &ratio,
        include_bytes!("../../docs/qualification/zip-lzma-2026-11-03/fixtures/ratio.zipx"),
    )
    .unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &ratio,
        true,
        &mut budget,
        || false,
        |m| {
            assert!(m.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 0);
    assert!(
        visit_members(
            &ratio,
            true,
            &mut budget,
            || true,
            |_| panic!("cancelled visit")
        )
        .unwrap()
        .cancelled
    );
}

#[test]
fn deflate64_reads_extended_lengths_and_full_window_fixture() {
    let temp = TempDir::new();
    let path = temp.path().join("extended.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-deflate64-2026-11-03/fixtures/extended.zipx"),
    )
    .unwrap();
    let expected = [
        (
            "extended.txt",
            include_bytes!(
                "../../docs/qualification/zip-deflate64-2026-11-03/fixtures/expected-extended.txt"
            )
            .as_slice(),
        ),
        (
            "nested/ёж.txt",
            include_bytes!(
                "../../docs/qualification/zip-deflate64-2026-11-03/fixtures/expected-distance.txt"
            )
            .as_slice(),
        ),
        ("empty.txt", b"".as_slice()),
    ];
    let cancel = AtomicBool::new(false);
    let listing = list_archive(path.clone(), &cancel).unwrap();
    assert_eq!(listing.members.len(), 3);
    assert!(
        listing
            .members
            .iter()
            .all(|m| m.zip_info.as_ref().unwrap().compression == "Deflate64")
    );
    let mut budget = SearchBudget::default();
    let mut contents = std::collections::BTreeMap::new();
    let summary = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |m| {
            contents.insert(m.member.path, m.content.unwrap());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 0);
    assert_eq!(summary.emitted, 3);
    let decoded = expected
        .iter()
        .map(|(_, bytes)| bytes.len() as u64)
        .sum::<u64>();
    assert_eq!(budget.content_bytes, decoded);
    let check =
        crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
            .unwrap();
    assert_eq!(check.members, 3);
    assert_eq!(check.decoded, decoded);
    let dest = temp.path().join("out");
    let indexes: Vec<_> = listing.members.iter().map(|m| m.index).collect();
    let report = extract_members(&path, &dest, &indexes, &cancel).unwrap();
    assert_eq!(report.extracted, 3);
    assert!(report.errors.is_empty());
    for (name, bytes) in expected {
        assert_eq!(contents[Path::new(name)].as_bytes(), bytes);
        assert_eq!(fs::read(dest.join(name)).unwrap(), bytes);
    }
    fs::write(dest.join("extended.txt"), b"keep").unwrap();
    let repeat = extract_members(&path, &dest, &indexes, &cancel).unwrap();
    assert_eq!(repeat.skipped_existing, 3);
    assert_eq!(fs::read(dest.join("extended.txt")).unwrap(), b"keep");
}

#[test]
fn deflate64_corrupt_and_truncated_members_leave_no_partial_file() {
    let temp = TempDir::new();
    for (name, bytes) in [
        (
            "bad-crc",
            include_bytes!(
                "../../docs/qualification/zip-deflate64-2026-11-03/fixtures/bad-crc.zipx"
            )
            .as_slice(),
        ),
        (
            "truncated",
            include_bytes!(
                "../../docs/qualification/zip-deflate64-2026-11-03/fixtures/truncated.zipx"
            )
            .as_slice(),
        ),
    ] {
        let path = temp.path().join(format!("{name}.zipx"));
        fs::write(&path, bytes).unwrap();
        let mut budget = SearchBudget::default();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |m| {
                assert!(m.content.is_none());
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 1, "{name}");
        assert!(
            crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
                .is_err()
        );
        let dest = temp.path().join(name);
        let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
        assert_eq!(report.extracted, 0, "{name}");
        assert_eq!(report.errors.len(), 1, "{name}");
        assert!(!dest.join("bad.txt").exists(), "{name}");
    }
}

#[test]
fn deflate64_maximum_length_preserves_ratio_and_cancel_guards() {
    let temp = TempDir::new();
    let path = temp.path().join("ratio.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-deflate64-2026-11-03/fixtures/ratio.zipx"),
    )
    .unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |m| {
            assert!(m.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 0);
    let check =
        crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
            .unwrap();
    assert_eq!(check.decoded, 65539);
    let dest = temp.path().join("out");
    let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.extracted, 1);
    assert!(report.errors.is_empty());
    assert_eq!(fs::read(dest.join("ratio.txt")).unwrap(), vec![b'A'; 65539]);
    let cancelled = visit_members(
        &path,
        true,
        &mut budget,
        || true,
        |_| panic!("cancelled visit"),
    )
    .unwrap();
    assert!(cancelled.cancelled);
}

#[test]
fn bzip2_zipx_lists_searches_checks_and_extracts_python_fixture() {
    let temp = TempDir::new();
    let path = temp.path().join("mixed.ZIPX");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-bzip2-2026-11-03/fixtures/mixed.zipx"),
    )
    .unwrap();
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(path.clone(), &cancelled).unwrap();
    assert_eq!(listing.members.len(), 4);
    assert!(!listing.truncated);
    let unicode = listing
        .members
        .iter()
        .find(|m| m.path == Path::new("nested/ёж.txt"))
        .unwrap();
    assert_eq!(unicode.zip_info.as_ref().unwrap().compression, "Bzip2");
    let mut found = std::collections::BTreeMap::new();
    let mut budget = SearchBudget::default();
    let visit = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |m| {
            found.insert(m.member.path, m.content.unwrap().to_string());
            true
        },
    )
    .unwrap();
    assert_eq!(visit.emitted, 4);
    assert_eq!(visit.content_skipped, 0);
    assert_eq!(
        found[Path::new("nested/ёж.txt")],
        "BZip2 archive needle: ёж / café\n"
    );
    assert_eq!(found[Path::new("empty.txt")], "");
    let progress = std::sync::atomic::AtomicU64::new(0);
    let check = crate::archive_check::check(&path, &|| false, &progress).unwrap();
    assert_eq!(check.members, 4);
    assert_eq!(
        check.decoded,
        found.values().map(|s| s.len() as u64).sum::<u64>()
    );
    let destination = temp.path().join("out");
    let indexes: Vec<_> = listing.members.iter().map(|m| m.index).collect();
    let report = extract_members(&path, &destination, &indexes, &cancelled).unwrap();
    assert_eq!(report.extracted, 4);
    assert!(report.errors.is_empty());
    for (name, content) in &found {
        assert_eq!(
            fs::read(destination.join(name)).unwrap(),
            content.as_bytes()
        );
    }
    fs::write(destination.join("nested/ёж.txt"), b"keep").unwrap();
    let repeat = extract_members(&path, &destination, &indexes, &cancelled).unwrap();
    assert_eq!(repeat.extracted, 0);
    assert_eq!(repeat.skipped_existing, 4);
    assert_eq!(
        fs::read(destination.join("nested/ёж.txt")).unwrap(),
        b"keep"
    );
}

#[test]
fn bzip2_zipx_rejects_bad_crc_without_retaining_partial_output() {
    let temp = TempDir::new();
    let path = temp.path().join("bad.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-bzip2-2026-11-03/fixtures/bad-crc.zipx"),
    )
    .unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |m| {
            assert!(m.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 1);
    assert!(
        crate::archive_check::check(&path, &|| false, &std::sync::atomic::AtomicU64::new(0))
            .is_err()
    );
    let dest = temp.path().join("out");
    let report = extract_members(&path, &dest, &[0], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.extracted, 0);
    assert_eq!(report.errors.len(), 1);
    assert!(!dest.join("payload.txt").exists());
}

#[test]
fn bzip2_zipx_keeps_path_and_ratio_guards() {
    let temp = TempDir::new();
    let path = temp.path().join("unsafe.zipx");
    fs::write(
        &path,
        include_bytes!("../../docs/qualification/zip-bzip2-2026-11-03/fixtures/unsafe.zipx"),
    )
    .unwrap();
    let listing = list_archive(path.clone(), &AtomicBool::new(false)).unwrap();
    assert_eq!(listing.unsafe_members, 1);
    assert_eq!(listing.members.len(), 1);
    let dest = temp.path().join("out");
    let report = extract_members(&path, &dest, &[0, 1], &AtomicBool::new(false)).unwrap();
    assert_eq!(report.extracted, 1);
    assert_eq!(report.errors.len(), 1);
    assert!(!temp.path().join("escape.txt").exists());
    let ratio = temp.path().join("ratio.zipx");
    fs::write(
        &ratio,
        include_bytes!("../../docs/qualification/zip-bzip2-2026-11-03/fixtures/ratio.zipx"),
    )
    .unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &ratio,
        true,
        &mut budget,
        || false,
        |m| {
            assert!(m.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 0);
    let summary = visit_members(
        &ratio,
        true,
        &mut budget,
        || true,
        |_| panic!("cancelled visit"),
    )
    .unwrap();
    assert!(summary.cancelled);
}

#[test]
fn zip_listing_reports_methods_and_encryption_without_decoding_payloads() {
    let temp = TempDir::new();
    let path = temp.path().join("metadata.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for method in [
        zip::CompressionMethod::Stored,
        zip::CompressionMethod::Deflated,
    ] {
        writer
            .start_file(
                method.to_string(),
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(b"payload").unwrap();
    }
    writer.finish().unwrap();
    let cancel = AtomicBool::new(false);
    let listing = list_archive(path.clone(), &cancel).unwrap();
    for member in &listing.members {
        let info = member.zip_info.as_ref().unwrap();
        assert_eq!(info.compression, member.path.to_str().unwrap());
        assert!(!info.encrypted);
        assert_eq!(info.encryption, "None");
    }
    // Change both headers to an unsupported method and set the encryption
    // bit. The payload deliberately remains undecodable: listing must only
    // inspect metadata, not try to decompress or require a password.
    let mut bytes = fs::read(&path).unwrap();
    for offset in 0..bytes.len().saturating_sub(12) {
        let (flags, method) = if bytes[offset..].starts_with(b"PK\x03\x04") {
            (offset + 6, offset + 8)
        } else if bytes[offset..].starts_with(b"PK\x01\x02") {
            (offset + 8, offset + 10)
        } else {
            continue;
        };
        bytes[flags] |= 1;
        bytes[method..method + 2].copy_from_slice(&222u16.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    let listing = list_archive(path, &cancel).unwrap();
    assert_eq!(listing.members.len(), 2);
    assert_eq!(listing.unreadable_members, 0);
    for member in listing.members {
        let info = member.zip_info.unwrap();
        assert!(info.encrypted);
        assert_eq!(info.encryption, "ZipCrypto");
        assert!(info.compression.contains("222"), "{}", info.compression);
    }
}

#[test]
fn zip_encryption_metadata_reports_all_aes_strengths_and_versions_without_passwords() {
    let temp = TempDir::new();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/qualification/zip-password-2026-11-03/fixtures");
    for bits in [128, 192, 256] {
        for version in [1, 2] {
            let name = format!(
                "aes-stored-{bits}{}.zip",
                if version == 1 { "-v1" } else { "" }
            );
            let path = temp.path().join(&name);
            fs::copy(fixtures.join(name), &path).unwrap();
            let listing = list_archive(path, &AtomicBool::new(false)).unwrap();
            assert_eq!(listing.unreadable_members, 0);
            assert_eq!(listing.members.len(), 1);
            let info = listing.members[0].zip_info.as_ref().unwrap();
            assert!(info.encrypted);
            assert_eq!(info.encryption, format!("WinZip AES-{bits} (AE-{version})"));
            assert_eq!(info.crc32.is_some(), version == 1);
        }
    }
}

#[test]
fn zip_encryption_metadata_never_calls_strong_flags_zipcrypto() {
    let temp = TempDir::new();
    let path = temp.path().join("strong.zipx");
    let mut bytes = include_bytes!(
        "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-stored-256.zip"
    )
    .to_vec();
    for offset in 0..bytes.len().saturating_sub(10) {
        let flag = if bytes[offset..].starts_with(b"PK\x03\x04") {
            Some(offset + 6)
        } else if bytes[offset..].starts_with(b"PK\x01\x02") {
            Some(offset + 8)
        } else {
            None
        };
        if let Some(flag) = flag {
            bytes[flag] |= 0x40;
        }
    }
    fs::write(&path, bytes).unwrap();
    let listing = list_archive(path, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        listing.members[0].zip_info.as_ref().unwrap().encryption,
        "Unknown (strong-encryption flag)"
    );
}

#[test]
fn password_content_search_preserves_authentication_limits_and_cancellation() {
    let temp = TempDir::new();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/qualification/zip-password-2026-11-03/fixtures");
    for codec in ["lzma", "xz", "zstd", "legacy", "ppmd"] {
        for strength in 1..=3 {
            for version in 1..=2 {
                let stem = format!("aes-{codec}-{strength}-v{version}");
                let path = fixtures.join(format!("{stem}.zipx"));
                let expected = fs::read_to_string(fixtures.join(format!("{stem}.txt"))).unwrap();
                let mut texts = Vec::new();
                let summary = visit_members_with_password(
                    &path,
                    true,
                    &mut SearchBudget::default(),
                    Some(b"fixture-password"),
                    || false,
                    |m| {
                        if let Some(text) = m.content {
                            texts.push(text);
                        }
                        true
                    },
                )
                .unwrap();
                assert_eq!(summary.content_skipped, 0, "{stem}");
                assert_eq!(texts.len(), 1, "{stem}");
                assert_eq!(&*texts[0], expected, "{stem}");
            }
        }
    }
    for (stem, password) in [
        ("aes-empty-password", ""),
        ("lzma", "fixture-password"),
        ("ppmd", "fixture-password"),
    ] {
        let path = fixtures.join(format!("{stem}.zipx"));
        let mut texts = Vec::new();
        let summary = visit_members_with_password(
            &path,
            true,
            &mut SearchBudget::default(),
            Some(password.as_bytes()),
            || false,
            |m| {
                if let Some(text) = m.content {
                    texts.push(text);
                }
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 0, "{stem}");
        assert_eq!(texts.len(), 1, "{stem}");
        assert_eq!(
            &*texts[0],
            fs::read_to_string(fixtures.join(format!("{stem}.txt"))).unwrap()
        );
    }
    for (name, password, expected) in [
        ("aes-utf8-password.zipx", "fixture-пароль-ёж", true),
        ("aes-stored-bad-tag.zip", "fixture-password", false),
        ("aes-ae1-bad-crc.zipx", "fixture-password", false),
    ] {
        let path = temp.path().join(name);
        fs::copy(fixtures.join(name), &path).unwrap();
        let mut budget = SearchBudget::default();
        let mut contents = Vec::new();
        let summary = visit_members_with_password(
            &path,
            true,
            &mut budget,
            Some(password.as_bytes()),
            || false,
            |m| {
                contents.push(m.content);
                true
            },
        )
        .unwrap();
        assert_eq!(contents.iter().any(Option::is_some), expected, "{name}");
        assert_eq!(summary.content_skipped, usize::from(!expected));
        let summary = visit_members_with_password(
            &path,
            true,
            &mut SearchBudget::default(),
            Some(password.as_bytes()),
            || true,
            |_| panic!("cancelled search emitted"),
        )
        .unwrap();
        assert!(summary.cancelled);
        let mut capped = SearchBudget {
            content_bytes: MAX_TOTAL_TEXT_BYTES,
            ..Default::default()
        };
        let summary = visit_members_with_password(
            &path,
            true,
            &mut capped,
            Some(password.as_bytes()),
            || false,
            |m| {
                assert!(m.content.is_none());
                true
            },
        )
        .unwrap();
        assert_eq!(summary.content_skipped, 1);
    }
}

#[test]
fn zip_declared_crc32_keeps_zero_and_header_values_without_payload_verification() {
    let temp = TempDir::new();
    let path = temp.path().join("crc.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for (name, bytes) in [
        ("empty", b"".as_slice()),
        ("ёж.txt", b"123456789".as_slice()),
    ] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    let listing = list_archive(path.clone(), &AtomicBool::new(false)).unwrap();
    assert_eq!(listing.members[0].zip_info.as_ref().unwrap().crc32, Some(0));
    assert_eq!(
        listing.members[1].zip_info.as_ref().unwrap().crc32,
        Some(0xcbf43926)
    );
    let mut bytes = fs::read(&path).unwrap();
    let central = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    bytes[central + 16..central + 20].copy_from_slice(&0xdeadbeefu32.to_le_bytes());
    fs::write(&path, bytes).unwrap();
    let listing = list_archive(path, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        listing.members[0].zip_info.as_ref().unwrap().crc32,
        Some(0xdeadbeef)
    );
}

#[test]
fn zip_declared_crc32_suppresses_nonzero_ae2_placeholder() {
    let temp = TempDir::new();
    let path = temp.path().join("ae2.zipx");
    let bytes = include_bytes!(
        "../../docs/qualification/zip-password-2026-11-03/fixtures/aes-ae2-nonzero-crc.zipx"
    );
    let mut raw = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_ne!(raw.by_index_raw(0).unwrap().crc32(), 0);
    fs::write(&path, bytes).unwrap();
    let listing = list_archive(path, &AtomicBool::new(false)).unwrap();
    assert!(
        listing.members[0]
            .zip_info
            .as_ref()
            .unwrap()
            .crc32
            .is_none()
    );
}

#[test]
fn zip_members_carry_their_own_modified_time() {
    let temp = TempDir::new();
    let path = temp.path().join("dated.zip");
    let stamp = zip::DateTime::from_date_and_time(2001, 2, 3, 4, 5, 6).unwrap();
    let file = fs::File::create(&path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let dated = zip::write::SimpleFileOptions::default().last_modified_time(stamp);
    writer.start_file("dated.txt", dated).unwrap();
    writer.write_all(b"d").unwrap();
    // No time recorded: the DOS default, which must not read as 1980.
    let undated =
        zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
    writer.start_file("undated.txt", undated).unwrap();
    writer.write_all(b"u").unwrap();
    writer.finish().unwrap();

    let cancelled = AtomicBool::new(false);
    let listing = list_archive(path, &cancelled).unwrap();
    let modified = |name: &str| {
        listing
            .members
            .iter()
            .find(|member| member.path == Path::new(name))
            .unwrap()
            .modified
    };
    let expected: std::time::SystemTime = {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(2001, 2, 3, 4, 5, 6)
            .earliest()
            .unwrap()
            .into()
    };
    assert_eq!(modified("dated.txt"), Some(expected));
    assert_eq!(modified("undated.txt"), None);
}

fn write_test_tar(path: &Path, files: &[(&str, &[u8])]) {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(37);
        header.set_cksum();
        builder.append_data(&mut header, name, *bytes).unwrap();
    }
    let bytes = builder.into_inner().unwrap();
    fs::write(
        path,
        encode_tar_fixture(kind_of(path).unwrap(), &bytes).unwrap(),
    )
    .unwrap();
}

#[test]
fn tar_content_search_handles_plain_and_compressed_formats_and_preserves_metadata() {
    let temp = TempDir::new();
    let large = vec![b'a'; MAX_MEMBER_TEXT_BYTES as usize + 1];
    for name in ["pack.tar", "pack.tgz", "pack.tbz2", "pack.txz", "pack.tzst"] {
        let path = temp.path().join(name);
        write_test_tar(
            &path,
            &[
                ("docs/ёж.txt", "Unicode needle".as_bytes()),
                ("binary.dat", b"zero\0bytes"),
                ("large.txt", &large),
            ],
        );
        let mut budget = SearchBudget::default();
        let mut members = Vec::new();
        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |member| {
                members.push(member);
                true
            },
        )
        .unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].content.as_deref(), Some("Unicode needle"));
        assert_eq!(members[0].member.path, Path::new("docs/ёж.txt"));
        assert_eq!(
            members[0].member.modified,
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(37))
        );
        assert!(members[1].content.is_none());
        assert!(members[2].content.is_none());
        assert_eq!(summary.content_skipped, 2);
        assert_eq!(
            summary.declared_uncompressed_bytes,
            u128::from(14_u64 + 10 + large.len() as u64)
        );
        assert!(!summary.members_not_searched);
        assert!(!budget.truncated);
    }
}

#[test]
fn compressed_tar_limits_and_cancellation_keep_partial_inspection_explicit() {
    let temp = TempDir::new();
    for name in ["limited.tbz2", "limited.txz", "limited.tzst"] {
        let path = temp.path().join(name);
        write_test_tar(
            &path,
            &[("large.txt", &vec![b'x'; 9000]), ("later.txt", b"needle")],
        );
        for needs_content in [false, true] {
            let mut budget = SearchBudget::default();
            visit_tar(
                &path,
                kind_of(&path).unwrap(),
                needs_content,
                1024,
                &mut budget,
                || false,
                |_| true,
            )
            .unwrap();
            assert!(budget.truncated, "{name}");
        }
        let cancelled = AtomicBool::new(false);
        let summary = visit_members(
            &path,
            false,
            &mut SearchBudget::default(),
            || cancelled.load(Ordering::Acquire),
            |_| {
                cancelled.store(true, Ordering::Release);
                true
            },
        )
        .unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.emitted, 1);
    }
}

#[test]
fn compressed_tar_concatenated_frames_and_broken_streams_are_handled() {
    let temp = TempDir::new();
    let raw = temp.path().join("raw.tar");
    write_test_tar(
        &raw,
        &[("docs/ёж.txt", b"needle"), ("second.txt", b"second")],
    );
    let bytes = fs::read(raw).unwrap();
    for name in ["frames.tbz2", "frames.txz", "frames.tzst"] {
        let path = temp.path().join(name);
        let kind = kind_of(&path).unwrap();
        let mut encoded = encode_tar_fixture(kind, &bytes[..511]).unwrap();
        encoded.extend(encode_tar_fixture(kind, &bytes[511..]).unwrap());
        fs::write(&path, &encoded).unwrap();
        let listing = list_archive(path.clone(), &AtomicBool::new(false)).unwrap();
        assert_eq!(listing.members.len(), 2);
        assert!(!listing.truncated);
        fs::write(&path, &encoded[..8]).unwrap();
        assert!(list_archive(path.clone(), &AtomicBool::new(false)).is_err());
        fs::write(&path, b"not compressed TAR").unwrap();
        assert!(list_archive(path, &AtomicBool::new(false)).is_err());
    }
}

#[test]
fn decoded_tar_staging_has_private_permissions_cleanup_budget_and_cancel() {
    let temp = TempDir::new();
    for name in ["stage.tbz2", "stage.txz", "stage.tzst"] {
        let path = temp.path().join(name);
        write_test_tar(&path, &[("large.txt", &vec![b'x'; 9000])]);
        let kind = kind_of(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let prepared =
            prepare_tar(&path, kind, &AtomicBool::new(false), MAX_TAR_STREAM_BYTES).unwrap();
        let staged = prepared.path.clone();
        assert!(staged.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&staged).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            tar::Archive::new(fs::File::open(&staged).unwrap())
                .entries()
                .unwrap()
                .count(),
            1
        );
        drop(prepared);
        assert!(!staged.exists());
        assert!(
            prepare_tar(&path, kind, &AtomicBool::new(false), 1024)
                .err()
                .unwrap()
                .contains("limit")
        );
        assert!(
            prepare_tar(&path, kind, &AtomicBool::new(true), MAX_TAR_STREAM_BYTES)
                .err()
                .unwrap()
                .contains("cancelled")
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[cfg(unix)]
#[test]
fn new_tar_formats_extract_selected_literal_names_metadata_and_skip_existing() {
    let temp = TempDir::new();
    for name in ["pack.tar.bz2", "pack.tar.xz", "pack.tar.zst"] {
        let path = temp.path().join(name);
        write_test_tar(
            &path,
            &[
                ("keep.txt", b"archive"),
                ("nested/ёж*.txt", b"selected"),
                ("-literal.txt", b"dash"),
                ("unselected.txt", b"leave"),
            ],
        );
        let before = fs::read(&path).unwrap();
        let dest = temp.dir(&format!("out-{name}"));
        fs::write(dest.join("keep.txt"), b"original").unwrap();
        let report = extract_members(&path, &dest, &[0, 1, 2], &AtomicBool::new(false)).unwrap();
        assert!(report.errors.is_empty(), "{name}: {:?}", report.errors);
        assert_eq!(report.extracted, 2);
        assert_eq!(report.skipped_existing, 1);
        assert_eq!(fs::read(dest.join("keep.txt")).unwrap(), b"original");
        assert_eq!(fs::read(dest.join("nested/ёж*.txt")).unwrap(), b"selected");
        assert_eq!(fs::read(dest.join("-literal.txt")).unwrap(), b"dash");
        assert!(!dest.join("unselected.txt").exists());
        assert_eq!(
            fs::metadata(dest.join("-literal.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(37)
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn tar_stream_budget_includes_skipped_payloads_and_cancels_between_members() {
    let temp = TempDir::new();
    let path = temp.path().join("limited.tgz");
    write_test_tar(
        &path,
        &[("large.txt", &vec![b'x'; 9000]), ("later.txt", b"needle")],
    );
    for needs_content in [false, true] {
        let mut budget = SearchBudget::default();
        visit_tar(
            &path,
            ArchiveKind::TarGz,
            needs_content,
            1024,
            &mut budget,
            || false,
            |_| true,
        )
        .unwrap();
        assert!(budget.truncated);
    }
    let cancelled = AtomicBool::new(false);
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &path,
        false,
        &mut budget,
        || cancelled.load(Ordering::Acquire),
        |_| {
            cancelled.store(true, Ordering::Release);
            true
        },
    )
    .unwrap();
    assert!(summary.cancelled);
    assert_eq!(summary.emitted, 1);
    assert!(!budget.truncated);
}

#[test]
fn tar_visits_share_member_and_text_budgets() {
    let temp = TempDir::new();
    let path = temp.path().join("budget.tar");
    write_test_tar(&path, &[("first.txt", b"needle"), ("second.txt", b"later")]);
    let mut budget = SearchBudget {
        members: MAX_MEMBERS_PER_RUN - 1,
        content_bytes: MAX_TOTAL_TEXT_BYTES - 3,
        ..Default::default()
    };
    let summary = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |member| {
            assert!(member.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.emitted, 1);
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.members, MAX_MEMBERS_PER_RUN);
    assert_eq!(budget.content_bytes, MAX_TOTAL_TEXT_BYTES - 3);
    assert!(budget.truncated);
}

#[test]
fn tar_stream_checks_cancellation_while_skipping_a_payload() {
    let remaining = std::cell::Cell::new(1000);
    let limited = std::cell::Cell::new(false);
    let cancelled = std::cell::Cell::new(false);
    let cancel = || cancelled.get();
    let mut reader = TarStream {
        inner: std::io::repeat(0),
        remaining: &remaining,
        limited: &limited,
        cancelled: &cancel,
    };
    assert_eq!(reader.read(&mut [0; 128]).unwrap(), 128);
    cancelled.set(true);
    assert!(reader.read(&mut [0; 128]).is_err());
    assert_eq!(remaining.get(), 872);
    assert!(!limited.get());
}

#[test]
fn tar_content_search_does_not_follow_link_targets() {
    let temp = TempDir::new();
    let outside = temp.file("secret.txt", "must not be read");
    let path = temp.path().join("links.tar");
    let mut builder = tar::Builder::new(fs::File::create(&path).unwrap());
    for entry_type in [tar::EntryType::Symlink, tar::EntryType::Link] {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(entry_type);
        builder
            .append_link(
                &mut header,
                if entry_type.is_symlink() {
                    "symbolic"
                } else {
                    "hard"
                },
                &outside,
            )
            .unwrap();
    }
    builder.finish().unwrap();
    let mut budget = SearchBudget::default();
    let summary = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |member| {
            assert!(member.content.is_none());
            true
        },
    )
    .unwrap();
    assert_eq!(summary.emitted, 2);
    assert_eq!(summary.content_skipped, 2);
    assert_eq!(budget.content_bytes, 0);
}

#[cfg(unix)]
#[test]
fn plain_tar_extraction_keeps_existing_files_and_extracts_literal_names() {
    let temp = TempDir::new();
    let path = temp.path().join("plain.tar");
    write_test_tar(
        &path,
        &[("keep.txt", b"archive"), ("nested/a*b.txt", b"new")],
    );
    let dest = temp.dir("out");
    fs::write(dest.join("keep.txt"), b"original").unwrap();
    let report = extract_members(&path, &dest, &[0, 1], &AtomicBool::new(false)).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.extracted, 1);
    assert_eq!(report.skipped_existing, 1);
    assert_eq!(fs::read(dest.join("keep.txt")).unwrap(), b"original");
    assert_eq!(fs::read(dest.join("nested/a*b.txt")).unwrap(), b"new");
}

#[test]
fn supported_archive_detection_is_case_insensitive() {
    assert!(is_supported(Path::new("bundle.ZIP")));
    for name in ["bundle.zipx", "bundle.ZIPX"] {
        assert_eq!(kind_of(Path::new(name)), Some(ArchiveKind::Zip));
    }
    assert_eq!(kind_of(Path::new("bundle.tgz")), Some(ArchiveKind::TarGz));
    assert_eq!(
        kind_of(Path::new("bundle.tar.gz")),
        Some(ArchiveKind::TarGz)
    );
    assert_eq!(kind_of(Path::new("bundle.TAR")), Some(ArchiveKind::Tar));
    assert_eq!(
        kind_of(Path::new("bundle.TAR.XZ")),
        Some(ArchiveKind::TarXz)
    );
    for name in ["bundle.tar.bz2", "bundle.tbz2", "bundle.TBZ"] {
        assert_eq!(kind_of(Path::new(name)), Some(ArchiveKind::TarBz2));
    }
    for name in ["bundle.tar.zst", "bundle.tar.zstd", "bundle.TZST"] {
        assert_eq!(kind_of(Path::new(name)), Some(ArchiveKind::TarZstd));
    }
    assert!(!is_supported(Path::new("standalone.xz")));
}

#[test]
fn extract_writes_selected_zip_members_without_clobber() {
    let temp = TempDir::new();
    let archive = temp.path().join("pack.zip");
    write_zip(
        &archive,
        &[("readme.txt", b"hello"), ("nested/a.txt", b"nested")],
    );
    let dest = temp.path().join("out");
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(archive.clone(), &cancelled).unwrap();
    let tree = listing.tree.as_ref().unwrap();
    assert_eq!(tree.directories.len(), 1);
    assert_eq!(tree.directories[0].path, Path::new("nested"));
    assert_eq!(
        listing.members.len(),
        2,
        "synthetic folders never enter extraction members"
    );
    let indexes: Vec<usize> = listing
        .members
        .iter()
        .filter(|member| !member.is_dir)
        .map(|member| member.index)
        .collect();
    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();
    assert_eq!(report.extracted, 2);
    assert_eq!(
        fs::read_to_string(dest.join("readme.txt")).unwrap(),
        "hello"
    );
    let again = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();
    assert_eq!(again.extracted, 0);
    assert_eq!(again.skipped_existing, 2);
}

#[test]
fn listing_rejects_parent_traversal_members() {
    let temp = TempDir::new();
    let path = temp.path().join("unsafe.zip");
    write_zip(
        &path,
        &[("../escape.txt", b"bad"), ("safe/readme.txt", b"ok")],
    );
    let cancelled = AtomicBool::new(false);

    let listing = list_archive(path, &cancelled).unwrap();

    assert_eq!(listing.unsafe_members, 1);
    assert_eq!(listing.members.len(), 1);
    assert_eq!(listing.members[0].path, PathBuf::from("safe/readme.txt"));
}

#[test]
fn member_visit_reads_text_and_skips_binary_content() {
    let temp = TempDir::new();
    let path = temp.path().join("content.zip");
    write_zip(
        &path,
        &[("notes.txt", b"searchable"), ("image.bin", b"a\0b")],
    );
    let mut budget = SearchBudget::default();
    let mut visited = Vec::new();

    let summary = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |member| {
            visited.push(member);
            true
        },
    )
    .unwrap();

    assert_eq!(visited.len(), 2);
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 13);
    assert!(visited.iter().any(|member| {
        member.member.path == Path::new("notes.txt")
            && member.content.as_deref() == Some("searchable")
    }));
}

#[test]
fn suspicious_compression_ratio_is_not_decompressed_for_search() {
    let temp = TempDir::new();
    let path = temp.path().join("ratio.zip");
    let repeated = vec![b'a'; 512 * 1024];
    write_zip(&path, &[("large.txt", &repeated)]);
    let mut budget = SearchBudget::default();
    let mut content = None;

    let summary = visit_members(
        &path,
        true,
        &mut budget,
        || false,
        |member| {
            content = member.content;
            true
        },
    )
    .unwrap();

    assert!(content.is_none());
    assert_eq!(summary.content_skipped, 1);
    assert_eq!(budget.content_bytes, 0);
}

#[test]
fn cancellation_stops_before_visiting_members() {
    let temp = TempDir::new();
    let path = temp.path().join("cancel.zip");
    write_zip(&path, &[("one.txt", b"one")]);
    let mut budget = SearchBudget::default();

    let summary = visit_members(
        &path,
        false,
        &mut budget,
        || true,
        |_| panic!("cancelled visit emitted a member"),
    )
    .unwrap();

    assert!(summary.cancelled);
    assert_eq!(summary.emitted, 0);
}

#[test]
fn member_budget_cap_truncates_mid_archive() {
    let temp = TempDir::new();
    let path = temp.path().join("cap.zip");
    write_zip(&path, &[("one.txt", b"1"), ("two.txt", b"2")]);
    // One member left in the run's budget: the second is never visited.
    let mut budget = SearchBudget {
        members: MAX_MEMBERS_PER_RUN - 1,
        ..Default::default()
    };
    let mut visited = Vec::new();

    let summary = visit_members(
        &path,
        false,
        &mut budget,
        || false,
        |member| {
            visited.push(member.member.path);
            true
        },
    )
    .unwrap();

    assert_eq!(visited, [PathBuf::from("one.txt")]);
    assert_eq!(summary.emitted, 1);
    assert!(budget.truncated);
    assert_eq!(budget.members, MAX_MEMBERS_PER_RUN);
}

#[test]
fn exhausted_budget_skips_the_archive_without_opening_it() {
    // Not even a zip: a spent budget must return before reading it.
    let temp = TempDir::new();
    let path = temp.path().join("garbage.zip");
    fs::write(&path, b"not a zip").unwrap();
    for mut budget in [
        SearchBudget {
            archives: MAX_ARCHIVES_PER_SEARCH,
            ..Default::default()
        },
        SearchBudget {
            members: MAX_MEMBERS_PER_RUN,
            ..Default::default()
        },
    ] {
        let before = budget.archives;

        let summary = visit_members(
            &path,
            true,
            &mut budget,
            || false,
            |_| panic!("budget-capped visit emitted a member"),
        )
        .unwrap();

        assert_eq!(summary, VisitSummary::default());
        assert!(budget.truncated);
        assert_eq!(budget.archives, before, "a skipped archive was counted");
    }
}

#[test]
fn zip_extract_skips_unsafe_members_and_keeps_going() {
    let temp = TempDir::new();
    let archive = temp.path().join("mixed.zip");
    write_zip(&archive, &[("../escape.txt", b"bad"), ("ok.txt", b"ok")]);
    let dest = temp.path().join("out");
    let cancelled = AtomicBool::new(false);

    let report = extract_members(&archive, &dest, &[0, 1], &cancelled).unwrap();

    assert_eq!(report.extracted, 1);
    assert_eq!(report.errors, ["Skipped unsafe member ../escape.txt"]);
    assert_eq!(fs::read(dest.join("ok.txt")).unwrap(), b"ok");
    assert!(!temp.path().join("escape.txt").exists());
}

#[cfg(unix)]
#[test]
fn zip_extract_does_not_write_through_dangling_symlink() {
    let temp = TempDir::new();
    let archive = temp.path().join("pack.zip");
    write_zip(&archive, &[("readme.txt", b"hello")]);
    let dest = temp.path().join("out");
    fs::create_dir(&dest).unwrap();
    let target = temp.path().join("elsewhere.txt");
    std::os::unix::fs::symlink(&target, dest.join("readme.txt")).unwrap();
    let cancelled = AtomicBool::new(false);

    let report = extract_members(&archive, &dest, &[0], &cancelled).unwrap();

    assert_eq!(report.extracted, 0);
    assert_eq!(report.skipped_existing, 1);
    assert!(!target.exists(), "extraction followed the dangling link");
}

#[cfg(unix)]
#[test]
fn zip_extract_creates_only_symlinks_that_stay_inside_the_destination() {
    let temp = TempDir::new();
    let archive = temp.path().join("links.zip");
    let file = fs::File::create(&archive).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    writer
        .add_symlink("dir/link", "../target.txt", options)
        .unwrap();
    writer
        .add_symlink("escape", "../../outside", options)
        .unwrap();
    writer.finish().unwrap();
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(archive.clone(), &cancelled).unwrap();
    let indexes: Vec<_> = listing.members.iter().map(|member| member.index).collect();
    let dest = temp.path().join("out");

    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

    assert_eq!(report.extracted, 1);
    assert_eq!(
        fs::read_link(dest.join("dir/link")).unwrap(),
        PathBuf::from("../target.txt")
    );
    assert!(!dest.join("escape").exists());
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("unsafe symlink"))
    );
}

#[test]
fn listing_lines_stop_at_limit_and_on_cancel() {
    // An endless listing: without the incremental reader this never ends.
    let endless = std::io::BufReader::new(std::io::repeat(b'\n'));
    let limited = collect_listing_lines(endless, 3, || false).unwrap();
    assert_eq!(limited.lines.len(), 3);
    assert!(limited.limited && !limited.cancelled);

    let calls = std::cell::Cell::new(0);
    let endless = std::io::BufReader::new(std::io::repeat(b'\n'));
    let cancelled = collect_listing_lines(endless, usize::MAX, || {
        calls.set(calls.get() + 1);
        calls.get() > 5
    })
    .unwrap();
    assert!(cancelled.cancelled);
    assert_eq!(cancelled.lines.len(), 5);

    let finite = collect_listing_lines(&b"a\nb c\r\n"[..], 10, || false).unwrap();
    assert_eq!(finite.lines, ["a", "b c\r"]);
    assert!(!finite.limited && !finite.cancelled);
}

#[test]
fn tar_member_paths_must_be_plain_relative_names() {
    assert!(is_enclosed_tar_path(Path::new("dir/file.txt")));
    assert!(is_enclosed_tar_path(Path::new("dir/")));
    assert!(!is_enclosed_tar_path(Path::new("/etc/passwd")));
    assert!(!is_enclosed_tar_path(Path::new("a/../../b")));
}

#[cfg(unix)]
#[test]
fn tar_listing_rejects_absolute_members() {
    let temp = TempDir::new();
    let source = temp.path().join("abs.txt");
    fs::write(&source, b"abs").unwrap();
    fs::write(temp.path().join("rel.txt"), b"rel").unwrap();
    let archive = temp.path().join("abs.tgz");
    let status = std::process::Command::new("tar")
        .arg("-czPf")
        .arg(&archive)
        .arg(&source)
        .arg("-C")
        .arg(temp.path())
        .arg("rel.txt")
        .status()
        .unwrap();
    assert!(status.success());
    let cancelled = AtomicBool::new(false);

    let listing = list_archive(archive, &cancelled).unwrap();

    assert_eq!(listing.unsafe_members, 1);
    assert_eq!(listing.members.len(), 1);
    assert_eq!(listing.members[0].path, Path::new("rel.txt"));
}

#[cfg(unix)]
#[test]
fn tar_extract_reports_unknown_member_indexes() {
    let temp = TempDir::new();
    let source = temp.path().join("src");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"a").unwrap();
    let archive = temp.path().join("one.tgz");
    write_tar_gz(&source, &archive, &["a.txt"]);
    let dest = temp.path().join("out");
    let cancelled = AtomicBool::new(false);

    let report = extract_members(&archive, &dest, &[0, 7], &cancelled).unwrap();

    assert_eq!(report.extracted, 1);
    assert_eq!(report.errors, ["Unknown archive member #7"]);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"a");
}

#[test]
fn chunked_copy_stops_when_cancelled_mid_member() {
    /// Sets the cancel flag after the first chunk, like a dialog closed
    /// while a large member is being written.
    struct CancelAfterFirst<'a>(&'a AtomicBool);
    impl Read for CancelAfterFirst<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.store(true, Ordering::Release);
            buf.fill(b'x');
            Ok(buf.len())
        }
    }
    let cancelled = AtomicBool::new(false);
    let mut out = Vec::new();

    let finished =
        copy_until_cancelled(&mut CancelAfterFirst(&cancelled), &mut out, &cancelled).unwrap();

    assert!(!finished);
    assert_eq!(out.len(), 256 * 1024, "copy kept reading after cancel");

    let idle = AtomicBool::new(false);
    let mut out = Vec::new();
    assert!(copy_until_cancelled(&mut &b"done"[..], &mut out, &idle).unwrap());
    assert_eq!(out, b"done");
}

#[test]
fn zip_extract_names_unsafe_members() {
    let temp = TempDir::new();
    let archive = temp.path().join("unsafe.zip");
    write_zip(&archive, &[("../escape.txt", b"bad")]);
    let dest = temp.path().join("out");
    let cancelled = AtomicBool::new(false);

    let report = extract_members(&archive, &dest, &[0], &cancelled).unwrap();

    assert_eq!(report.errors, ["Skipped unsafe member ../escape.txt"]);
}

#[cfg(unix)]
#[test]
fn zip_extract_keeps_unix_mode_and_mtime() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new();
    let archive = temp.path().join("script.zip");
    let file = fs::File::create(&archive).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let stamp = zip::DateTime::from_date_and_time(2020, 3, 4, 5, 6, 8).unwrap();
    let options = zip::write::SimpleFileOptions::default()
        .unix_permissions(0o755)
        .last_modified_time(stamp);
    writer.start_file("run.sh", options).unwrap();
    writer.write_all(b"#!/bin/sh\n").unwrap();
    writer.finish().unwrap();
    let dest = temp.path().join("out");
    let cancelled = AtomicBool::new(false);

    let report = extract_members(&archive, &dest, &[0], &cancelled).unwrap();

    assert_eq!(report.extracted, 1);
    let meta = fs::metadata(dest.join("run.sh")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o755);
    assert_eq!(meta.modified().unwrap(), dos_time_to_system(stamp).unwrap());
}

#[cfg(unix)]
#[test]
fn child_wait_kills_process_on_cancel() {
    let child = std::process::Command::new("sleep")
        .arg("30")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let cancelled = AtomicBool::new(true);
    let started = std::time::Instant::now();

    let output = wait_child_or_cancel(child, &cancelled).unwrap();

    assert!(output.is_none());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));

    let child = std::process::Command::new("sh")
        .args(["-c", "echo oops >&2; exit 3"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let output = wait_child_or_cancel(child, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stderr, b"oops\n");
}

/// Build `<dir>/<name>.tgz` from `names` under `source` with system tar,
/// without a `./` prefix so members extract under their listed names.
#[cfg(unix)]
fn write_tar_gz(source: &Path, archive: &Path, names: &[&str]) {
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(archive)
        .arg("-C")
        .arg(source)
        .args(names)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn verbose_tar_lines_parse_gnu_and_bsd_layouts() {
    assert_eq!(
        parse_tar_verbose_line("-rw-r--r-- root/root         5 2026-10-14 18:17 ./hl"),
        Some(("./hl", 5))
    );
    assert_eq!(
        parse_tar_verbose_line("-rw-r--r-- u/g 1 2026-10-14 18:16  sp "),
        Some((" sp ", 1))
    );
    assert_eq!(
        parse_tar_verbose_line("lrwxrwxrwx root/root 0 2026-10-14 18:17 sl -> f"),
        Some(("sl", 0))
    );
    assert_eq!(
        parse_tar_verbose_line("hrw-r--r-- root/root 0 2026-10-14 18:17 f link to hl"),
        Some(("f", 0))
    );
    assert_eq!(
        parse_tar_verbose_line("-rw-r--r--  0 user   staff    1234 Oct  4  2024 dir/a b.txt"),
        Some(("dir/a b.txt", 1234))
    );
    assert_eq!(parse_tar_verbose_line("garbage"), None);
}

#[cfg(unix)]
#[test]
fn tar_listing_keeps_member_sizes_and_edge_spaces() {
    let temp = TempDir::new();
    let source = temp.path().join("src");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("hello.txt"), b"hello").unwrap();
    fs::write(source.join(" padded "), b"pad").unwrap();
    let archive = temp.path().join("pack.tgz");
    write_tar_gz(&source, &archive, &["hello.txt", " padded "]);
    let cancelled = AtomicBool::new(false);

    let listing = list_archive(archive.clone(), &cancelled).unwrap();

    let hello = listing
        .members
        .iter()
        .find(|member| member.path == Path::new("hello.txt"))
        .unwrap();
    assert_eq!(hello.size, 5);
    let padded = listing
        .members
        .iter()
        .find(|member| member.path == Path::new(" padded "))
        .expect("edge spaces are part of the member name");
    let indexes = [padded.index];
    let dest = temp.path().join("out");
    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(fs::read(dest.join(" padded ")).unwrap(), b"pad");
}

#[cfg(unix)]
#[test]
fn tar_listing_preserves_non_utf8_member_names() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let temp = TempDir::new();
    let archive_path = temp.path().join("raw-name.tgz");
    let encoder = flate2::write::GzEncoder::new(
        fs::File::create(&archive_path).unwrap(),
        flate2::Compression::default(),
    );
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(1);
    header.set_mode(0o644);
    header.set_cksum();
    let raw = std::ffi::OsString::from_vec(vec![b'f', 0xff]);
    builder
        .append_data(&mut header, Path::new(&raw), &b"x"[..])
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();

    let listing = list_archive(archive_path, &AtomicBool::new(false)).unwrap();

    assert_eq!(listing.members.len(), 1);
    assert_eq!(
        listing.members[0].path.as_os_str().as_bytes(),
        &[b'f', 0xff]
    );
    assert_eq!(
        listing.members[0].tar_name.as_deref().unwrap().as_bytes(),
        &[b'f', 0xff]
    );
}

#[cfg(unix)]
#[test]
fn tar_lists_and_extracts_option_like_member_names() {
    // Names that look like tar flags must stay member names, both in the
    // listing and in the extract name list.
    let temp = TempDir::new();
    let source = temp.path().join("src");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("-v"), b"dash").unwrap();
    fs::write(source.join("--null"), b"double").unwrap();
    let archive = temp.path().join("flags.tgz");
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .args(["--", "-v", "--null"])
        .status()
        .unwrap();
    assert!(status.success());
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(archive.clone(), &cancelled).unwrap();
    let mut names: Vec<_> = listing
        .members
        .iter()
        .map(|member| member.path.clone())
        .collect();
    names.sort();
    assert_eq!(names, [PathBuf::from("--null"), PathBuf::from("-v")]);
    let indexes: Vec<usize> = listing.members.iter().map(|member| member.index).collect();
    let dest = temp.path().join("out");

    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.extracted, 2);
    assert_eq!(fs::read(dest.join("-v")).unwrap(), b"dash");
    assert_eq!(fs::read(dest.join("--null")).unwrap(), b"double");
}

#[cfg(unix)]
#[test]
fn tar_extracts_glob_metacharacters_as_literal_member_names() {
    let temp = TempDir::new();
    let source = temp.path().join("src");
    fs::create_dir(&source).unwrap();
    for (name, content) in [
        ("a*b", b"star".as_slice()),
        ("axb", b"decoy".as_slice()),
        ("q?.txt", b"question".as_slice()),
        ("[x].txt", b"bracket".as_slice()),
    ] {
        fs::write(source.join(name), content).unwrap();
    }
    let archive = temp.path().join("patterns.tgz");
    write_tar_gz(&source, &archive, &["a*b", "axb", "q?.txt", "[x].txt"]);
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(archive.clone(), &cancelled).unwrap();
    let indexes: Vec<_> = listing
        .members
        .iter()
        .filter(|member| matches!(member.path.to_str(), Some("a*b" | "q?.txt" | "[x].txt")))
        .map(|member| member.index)
        .collect();
    let dest = temp.path().join("out");

    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.extracted, 3);
    assert_eq!(fs::read(dest.join("a*b")).unwrap(), b"star");
    assert_eq!(fs::read(dest.join("q?.txt")).unwrap(), b"question");
    assert_eq!(fs::read(dest.join("[x].txt")).unwrap(), b"bracket");
    assert!(
        !dest.join("axb").exists(),
        "glob matched an unrequested member"
    );
}

#[cfg(unix)]
#[test]
fn failed_tar_extract_still_counts_the_members_it_wrote() {
    let temp = TempDir::new();
    let source = temp.path().join("src");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::write(source.join("a.txt"), b"a").unwrap();
    fs::write(source.join("sub/b.txt"), b"b").unwrap();
    let archive = temp.path().join("pack.tgz");
    write_tar_gz(&source, &archive, &["a.txt", "sub/b.txt"]);
    let dest = temp.path().join("out");
    fs::create_dir(&dest).unwrap();
    // A file where `sub/` must go makes tar fail for b.txt only.
    fs::write(dest.join("sub"), b"in the way").unwrap();
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(archive.clone(), &cancelled).unwrap();
    let indexes = listing
        .members
        .iter()
        .filter(|member| !member.is_dir)
        .map(|member| member.index)
        .collect::<Vec<_>>();

    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(report.extracted, 1);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"a");
}

#[cfg(unix)]
#[test]
fn tar_extract_handles_dot_slash_prefixed_members() {
    // `tar -czf x.tgz -C src .` stores every member as `./name`.
    let temp = TempDir::new();
    let source = temp.path().join("src");
    fs::create_dir_all(source.join("sub")).unwrap();
    fs::write(source.join("top.txt"), b"top").unwrap();
    fs::write(source.join("sub/inner.txt"), b"inner").unwrap();
    let archive = temp.path().join("dot.tgz");
    write_tar_gz(&source, &archive, &["."]);
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(archive.clone(), &cancelled).unwrap();
    assert!(
        listing
            .members
            .iter()
            .any(|member| member.path == Path::new("sub/inner.txt"))
    );
    let indexes: Vec<usize> = listing.members.iter().map(|member| member.index).collect();
    let dest = temp.path().join("out");

    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.extracted, 2);
    assert_eq!(fs::read(dest.join("top.txt")).unwrap(), b"top");
    assert_eq!(fs::read(dest.join("sub/inner.txt")).unwrap(), b"inner");
}

#[cfg(unix)]
#[test]
fn tar_extract_all_does_not_overflow_argument_list() {
    // ~2.2 MB of member paths: more than Linux's 2 MB ARG_MAX when the
    // names were passed as tar arguments. Each path stays well under
    // macOS's 1024-byte PATH_MAX.
    let temp = TempDir::new();
    let source = temp.path().join("src");
    let mut deep = PathBuf::new();
    for level in 0..3 {
        deep.push(format!("{level:02}{}", "d".repeat(200)));
    }
    fs::create_dir_all(source.join(&deep)).unwrap();
    for index in 0..3_600 {
        fs::write(source.join(&deep).join(format!("f{index}")), b"x").unwrap();
    }
    let archive = temp.path().join("many.tgz");
    let top = deep.iter().next().unwrap().to_str().unwrap().to_string();
    write_tar_gz(&source, &archive, &[&top]);
    let cancelled = AtomicBool::new(false);
    let listing = list_archive(archive.clone(), &cancelled).unwrap();
    let indexes: Vec<usize> = listing.members.iter().map(|member| member.index).collect();
    let dest = temp.path().join("out");

    let report = extract_members(&archive, &dest, &indexes, &cancelled).unwrap();

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.extracted, 3_600);
    assert!(dest.join(&deep).join("f3599").is_file());
}
