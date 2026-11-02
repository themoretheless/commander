//! Fresh bounded byte comparison for sync previews, without the verified-hash cache.
use crate::{
    panel::{FileEntry, ListingIdentity},
    path_identity::{PathIdentity, PathKind},
};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};
const CHUNK: usize = 128 * 1024;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Comparison {
    #[default]
    Unchecked,
    Equal,
    Different,
    Unavailable(String),
}
impl Comparison {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Equal => "equal",
            Self::Different => "different",
            Self::Unavailable(_) => "unavailable",
        }
    }
    pub fn error(&self) -> Option<&str> {
        if let Self::Unavailable(error) = self {
            Some(error)
        } else {
            None
        }
    }
}
fn open(entry: &FileEntry) -> Result<(File, PathIdentity), String> {
    let ListingIdentity::Captured(expected) = &entry.identity else {
        return Err("No captured file identity".into());
    };
    if expected.kind != Some(PathKind::File) {
        return Err(
            "Only regular files are compared; symbolic links and special files are not followed"
                .into(),
        );
    }
    let lexical = std::fs::symlink_metadata(&entry.path).map_err(|e| e.to_string())?;
    if !lexical.is_file() {
        return Err("The path no longer names a regular file".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY);
    }
    let file = options.open(&entry.path).map_err(|e| e.to_string())?;
    let actual = PathIdentity::from_metadata(
        entry.path.clone(),
        &file.metadata().map_err(|e| e.to_string())?,
    );
    if !expected.same_shallow_binding(&actual) {
        return Err("File changed since the directory scan".into());
    }
    Ok((file, actual))
}
fn current(file: &File, path: &Path, expected: &PathIdentity) -> Result<(), String> {
    let held = PathIdentity::from_metadata(
        path.to_owned(),
        &file.metadata().map_err(|e| e.to_string())?,
    );
    let bound = PathIdentity::observe(path).map_err(|e| e.to_string())?;
    if !expected.same_shallow_binding(&held) || !expected.same_shallow_binding(&bound) {
        return Err("File or pathname changed during content comparison".into());
    }
    Ok(())
}
fn fill(file: &mut impl Read, buffer: &mut [u8], cancel: &dyn Fn() -> bool) -> Result<(), String> {
    let mut at = 0;
    while at < buffer.len() {
        if cancel() {
            return Err("Content comparison cancelled".into());
        }
        match file.read(&mut buffer[at..]) {
            Ok(0) => return Err("File became shorter during content comparison".into()),
            Ok(n) => at += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
fn compare(left: &FileEntry, right: &FileEntry, cancel: &dyn Fn() -> bool) -> Result<bool, String> {
    if cancel() {
        return Err("Content comparison cancelled".into());
    }
    let (mut a, ia) = open(left)?;
    let (mut b, ib) = open(right)?;
    let mut equal = left.size == right.size;
    if equal {
        let mut ba = vec![0; CHUNK];
        let mut bb = vec![0; CHUNK];
        let mut remaining = left.size;
        while remaining > 0 {
            let n = usize::try_from(remaining.min(CHUNK as u64)).expect("bounded chunk");
            fill(&mut a, &mut ba[..n], cancel)?;
            fill(&mut b, &mut bb[..n], cancel)?;
            if ba[..n] != bb[..n] {
                equal = false;
                break;
            }
            remaining -= n as u64;
        }
        if equal {
            let mut byte = [0];
            for file in [&mut a, &mut b] {
                loop {
                    if cancel() {
                        return Err("Content comparison cancelled".into());
                    }
                    match file.read(&mut byte) {
                        Ok(0) => break,
                        Ok(_) => return Err("File grew during content comparison".into()),
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            }
        }
    }
    current(&a, &left.path, &ia)?;
    current(&b, &right.path, &ib)?;
    if cancel() {
        return Err("Content comparison cancelled".into());
    }
    Ok(equal)
}
pub fn refine(
    actions: &mut [crate::sync::SyncAction],
    left: &[FileEntry],
    right: &[FileEntry],
    policy: crate::sync::SyncPolicy,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    use std::collections::HashMap;
    let left: HashMap<_, _> = left.iter().map(|e| (e.path.as_path(), e)).collect();
    let right: HashMap<_, _> = right.iter().map(|e| (e.path.as_path(), e)).collect();
    for action in actions {
        if cancel() {
            return Err("Content comparison cancelled".into());
        }
        if matches!(
            action.status,
            crate::sync::SyncStatus::DirectoryPair
                | crate::sync::SyncStatus::TypeConflict
                | crate::sync::SyncStatus::CaseConflict
        ) {
            continue;
        }
        let [Some(a), Some(b)] = action.side_paths() else {
            continue;
        };
        let result = match (left.get(a), right.get(b)) {
            (Some(a), Some(b)) => match compare(a, b, cancel) {
                Ok(true) => Comparison::Equal,
                Ok(false) => Comparison::Different,
                Err(error) => Comparison::Unavailable(error),
            },
            _ => Comparison::Unavailable("Paired entry is absent from the captured scan".into()),
        };
        if cancel() {
            return Err("Content comparison cancelled".into());
        }
        action.set_content(result, policy);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sync::{SyncDirection as D, SyncPolicy as P, SyncStatus as S},
        testutil::TempDir,
    };
    fn observe(path: &Path) -> FileEntry {
        FileEntry::observe_lexical(path.to_owned()).unwrap()
    }
    fn date(path: &Path, text: &str) {
        File::open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(crate::query::parse_absolute_time(text).unwrap()),
            )
            .unwrap();
    }
    #[test]
    fn content_plan_detects_equal_metadata_changes_and_equal_bytes_with_different_dates() {
        let left = TempDir::new();
        let right = TempDir::new();
        for (name, a, b) in [
            ("hidden-change", "aaaa", "bbbb"),
            ("same-bytes", "same", "same"),
            ("older-diff", "old", "new"),
            ("sizes", "x", "long"),
            ("empty", "", ""),
        ] {
            let a = left.file(name, a);
            let b = right.file(name, b);
            date(&a, "2020-01-01");
            date(&b, "2020-01-01");
        }
        date(&left.path().join("same-bytes"), "2022-01-01");
        date(&right.path().join("older-diff"), "2022-01-01");
        left.file("only", "x");
        left.file("folder/a", "a");
        right.file("folder/b", "b");
        let (plain, stamp) =
            crate::sync_guard::build_plan(left.path(), right.path(), false, false, P::TwoWay)
                .unwrap();
        assert!(!stamp.compare_contents);
        assert!(plain.iter().all(|a| a.content == Comparison::Unchecked));
        let hidden = plain.iter().find(|a| a.name == "hidden-change").unwrap();
        assert_eq!((hidden.status, hidden.direction), (S::Identical, D::Skip));
        let (actions, checked) = crate::sync_guard::build_plan_with_contents(
            left.path(),
            right.path(),
            false,
            false,
            P::TwoWay,
            true,
            &|| false,
        )
        .unwrap();
        assert!(checked.compare_contents);
        assert_ne!(checked.filter_key(), stamp.filter_key());
        let row = |name: &str| actions.iter().find(|a| a.name == name).unwrap();
        assert_eq!(
            (
                row("hidden-change").status,
                row("hidden-change").direction,
                row("hidden-change").content.clone()
            ),
            (S::Differing, D::Skip, Comparison::Different)
        );
        assert_eq!(
            (
                row("same-bytes").status,
                row("same-bytes").direction,
                row("same-bytes").content.clone()
            ),
            (S::LeftNewer, D::Skip, Comparison::Equal)
        );
        assert_eq!(
            (row("older-diff").status, row("older-diff").direction),
            (S::RightNewer, D::ToLeft)
        );
        assert_eq!(row("empty").content, Comparison::Equal);
        assert_eq!(row("sizes").content, Comparison::Different);
        assert_eq!(
            (row("folder").status, row("folder").content.clone()),
            (S::DirectoryPair, Comparison::Unchecked)
        );
        assert_eq!(row("only").content, Comparison::Unchecked);
        for policy in [P::MirrorLeftToRight, P::MirrorRightToLeft] {
            let (a, _) = crate::sync_guard::build_plan_with_contents(
                left.path(),
                right.path(),
                false,
                false,
                policy,
                true,
                &|| false,
            )
            .unwrap();
            assert_eq!(
                a.iter().find(|a| a.name == "same-bytes").unwrap().direction,
                D::Skip
            );
            assert_eq!(
                a.iter()
                    .find(|a| a.name == "hidden-change")
                    .unwrap()
                    .direction,
                if policy == P::MirrorLeftToRight {
                    D::ToRight
                } else {
                    D::ToLeft
                }
            );
        }
    }
    #[test]
    fn content_reads_are_fresh_after_same_length_rewrites_with_restored_mtime() {
        let temp = TempDir::new();
        let a = temp.file("a", "same");
        let b = temp.file("b", "same");
        let old = std::fs::metadata(&b).unwrap().modified().unwrap();
        let _ = crate::verified_hash::file(&b).unwrap();
        assert!(compare(&observe(&a), &observe(&b), &|| false).unwrap());
        std::fs::write(&b, b"DIFF").unwrap();
        File::open(&b)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        assert!(!compare(&observe(&a), &observe(&b), &|| false).unwrap());
        let empty = temp.file("empty", "");
        assert!(compare(&observe(&empty), &observe(&empty), &|| false).unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn content_comparison_rejects_links_special_files_and_changed_path_bindings() {
        let temp = TempDir::new();
        let a = temp.file("a", "same");
        let b = temp.file("b", "same");
        let original = observe(&a);
        let (held, identity) = open(&original).unwrap();
        std::fs::rename(&a, temp.path().join("held")).unwrap();
        std::os::unix::fs::symlink(&b, &a).unwrap();
        assert!(current(&held, &a, &identity).is_err());
        assert!(compare(&original, &observe(&b), &|| false).is_err());
        assert!(
            compare(&observe(&a), &observe(&b), &|| false)
                .unwrap_err()
                .contains("regular")
        );
        let socket = std::os::unix::net::UnixListener::bind(temp.path().join("socket")).unwrap();
        assert!(
            compare(&observe(&temp.path().join("socket")), &observe(&b), &|| {
                false
            })
            .is_err()
        );
        drop(socket);
        std::fs::remove_file(&a).unwrap();
        std::fs::write(&a, b"same").unwrap();
        assert!(current(&held, &a, &identity).is_err());
    }
    #[test]
    fn chunk_reader_handles_short_reads_interruption_truncation_and_cancellation() {
        struct Short {
            data: std::io::Cursor<Vec<u8>>,
            interrupt: bool,
        }
        impl Read for Short {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                if self.interrupt {
                    self.interrupt = false;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.data.read(&mut b[..1])
            }
        }
        let mut reader = Short {
            data: std::io::Cursor::new(b"four".to_vec()),
            interrupt: true,
        };
        let mut bytes = [0; 4];
        fill(&mut reader, &mut bytes, &|| false).unwrap();
        assert_eq!(&bytes, b"four");
        assert!(fill(&mut reader, &mut [0], &|| false).is_err());
        assert!(
            fill(&mut std::io::empty(), &mut [0], &|| true)
                .unwrap_err()
                .contains("cancelled")
        );
        let temp = TempDir::new();
        let a = temp.file("a", &"x".repeat(CHUNK * 3));
        let b = temp.file("b", &"x".repeat(CHUNK * 3));
        let calls = std::cell::Cell::new(0);
        let result = compare(&observe(&a), &observe(&b), &|| {
            let n = calls.get() + 1;
            calls.set(n);
            n > 4
        });
        assert!(result.unwrap_err().contains("cancelled"));
    }
    #[test]
    fn unavailable_pairs_never_allow_copy_and_identity_replacements_invalidate_the_stamp() {
        let left = TempDir::new();
        let right = TempDir::new();
        let held = TempDir::new();
        let a = left.file("pair", "old");
        right.file("pair", "new");
        let (mut actions, stamp) =
            crate::sync_guard::build_plan(left.path(), right.path(), false, false, P::TwoWay)
                .unwrap();
        actions[0].set_content(
            Comparison::Unavailable("cannot read".into()),
            P::MirrorLeftToRight,
        );
        assert_eq!(actions[0].direction, D::Skip);
        assert!(!actions[0].allows(D::ToRight));
        assert!(!actions[0].allows(D::ToLeft));
        let before = std::fs::metadata(&a).unwrap().modified().unwrap();
        std::fs::rename(&a, held.path().join("original")).unwrap();
        std::fs::write(&a, b"old").unwrap();
        File::open(&a)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(before))
            .unwrap();
        let (_, new) =
            crate::sync_guard::build_plan(left.path(), right.path(), false, false, P::TwoWay)
                .unwrap();
        assert_ne!(stamp.source_fingerprint, new.source_fingerprint);
        let guard = crate::sync_guard::GuardPolicy::default();
        let expected =
            crate::sync_guard::settings_fingerprint(P::TwoWay, &guard, stamp.filter_key());
        actions[0].direction = D::ToRight;
        let error = crate::sync_guard::validate(&crate::sync_guard::GuardedPlan {
            actions: &actions,
            stamp: &stamp,
            policy: P::TwoWay,
            guard: &guard,
            expected_settings: expected,
            allow_large_plan: false,
        })
        .unwrap_err();
        assert!(error.contains("Unverified"));
    }
}
