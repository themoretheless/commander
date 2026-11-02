//! Bounded, cancellable file import for the rename name-list editor.
use std::{io::Read, path::Path};

pub fn read(path: &Path, cancelled: &impl Fn() -> bool) -> Result<String, String> {
    let check = || {
        if cancelled() {
            Err("Name-list loading cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    check()?;
    let before = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() {
        return Err("Choose a regular name-list file, not a symlink or directory".into());
    }
    let limit = crate::rename::MAX_NAME_LIST_BYTES;
    if before.len() > limit as u64 {
        return Err("Name lists are limited to 256 KiB".into());
    }
    let identity = |meta: &std::fs::Metadata| {
        crate::path_identity::PathIdentity::from_metadata(path.into(), meta)
    };
    let expected = identity(&before);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    check()?;
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    if !file
        .metadata()
        .is_ok_and(|m| m.is_file() && expected.same_shallow_binding(&identity(&m)))
    {
        return Err("Name-list file changed before opening".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        check()?;
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > limit {
            return Err("Name lists are limited to 256 KiB".into());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    check()?;
    if !file
        .metadata()
        .is_ok_and(|m| expected.same_shallow_binding(&identity(&m)))
        || !crate::path_identity::PathIdentity::observe(path)
            .is_ok_and(|now| expected.same_shallow_binding(&now))
    {
        return Err("Name-list file changed while reading".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "Name-list file must be UTF-8".to_owned())?;
    crate::rename::parse_name_list(&text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_preserves_bom_crlf_unicode_and_source_bytes() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.path().join("names.txt");
        let text = "\u{feff}Первый.txt\r\n second.txt \r\n";
        std::fs::write(&path, text).unwrap();
        assert_eq!(read(&path, &|| false).unwrap(), text);
        assert_eq!(
            crate::rename::parse_name_list(text).unwrap(),
            ["Первый.txt", " second.txt "]
        );
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
    }
    #[test]
    fn accepts_exact_byte_limit_and_refuses_replaced_source() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("names.txt", &"x".repeat(crate::rename::MAX_NAME_LIST_BYTES));
        assert_eq!(
            read(&path, &|| false).unwrap().len(),
            crate::rename::MAX_NAME_LIST_BYTES
        );
        std::fs::write(&path, "original.txt").unwrap();
        let replacement = dir.file("replacement.txt", "replaced.txt");
        let calls = std::cell::Cell::new(0);
        let error = read(&path, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                std::fs::remove_file(&path).unwrap();
                std::fs::rename(&replacement, &path).unwrap();
            }
            false
        })
        .unwrap_err();
        assert!(error.contains("changed while reading"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replaced.txt");
    }

    #[test]
    fn refuses_invalid_oversized_nonregular_missing_and_cancelled_sources() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.path().join("names.txt");
        assert!(read(&path, &|| false).is_err());
        assert!(read(dir.path(), &|| false).is_err());
        for bytes in [
            vec![255],
            b"a\n\nb".to_vec(),
            vec![b'x'; crate::rename::MAX_NAME_LIST_BYTES + 1],
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(read(&path, &|| false).is_err());
        }
        std::fs::write(&path, b"valid.txt").unwrap();
        assert!(read(&path, &|| true).unwrap_err().contains("cancelled"));
        let calls = std::cell::Cell::new(0);
        assert!(
            read(&path, &|| {
                calls.set(calls.get() + 1);
                calls.get() > 2
            })
            .unwrap_err()
            .contains("cancelled")
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read(&link, &|| false).is_err());
        }
    }
}
