//! Pure copy/move conflict detection and relation-policy resolution. No I/O
//! and no transfer-engine types: `resolve` returns which sources to keep plus
//! a local [`Decision`], which the call site maps onto the engine's overwrite
//! policy. Reused by the confirmation dialog.

use crate::panel::FileEntry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// One name collision between a source and a same-named destination entry.
#[derive(Clone, PartialEq, Debug)]
pub struct Conflict {
    pub name: String,
    pub src_path: PathBuf,
    pub src_size: u64,
    pub dst_size: u64,
    pub src_is_dir: bool,
    pub dst_is_dir: bool,
    /// Source mtime is strictly newer than the destination's.
    pub src_newer: bool,
    /// Destination mtime is strictly newer than the source's.
    pub dst_newer: bool,
}

/// Source entries whose (case-insensitive) name already exists in `dest`.
/// The destination entry judged against is the exact-name one when present,
/// else the unique case-insensitive match. When several case variants exist
/// (`Foo` and `FOO` on a case-sensitive volume) and none is exact, the source
/// is judged against all of them, so KeepNewer/KeepLarger only replace when it
/// beats every variant rather than whichever one a map happened to keep.
pub fn detect(sources: &[FileEntry], dest: &[FileEntry]) -> Vec<Conflict> {
    let exact: HashMap<&str, &FileEntry> = dest.iter().map(|e| (e.name.as_str(), e)).collect();
    let mut folded: HashMap<&str, Vec<&FileEntry>> = HashMap::new();
    for e in dest {
        folded.entry(e.name_lower.as_str()).or_default().push(e);
    }
    sources
        .iter()
        .filter_map(|s| {
            let targets = match exact.get(s.name.as_str()) {
                Some(d) => std::slice::from_ref(d),
                None => folded.get(s.name_lower.as_str())?.as_slice(),
            };
            let newer_than =
                |d: &&FileEntry| matches!((s.modified, d.modified), (Some(a), Some(b)) if a > b);
            let older_than =
                |d: &&FileEntry| matches!((s.modified, d.modified), (Some(a), Some(b)) if b > a);
            Some(Conflict {
                name: s.name.clone(),
                src_path: s.path.clone(),
                src_size: s.size,
                dst_size: targets.iter().map(|d| d.size).max().unwrap_or(0),
                src_is_dir: s.is_dir,
                dst_is_dir: targets.iter().any(|d| d.is_dir),
                src_newer: targets.iter().all(newer_than),
                dst_newer: targets.iter().any(older_than),
            })
        })
        .collect()
}

/// How to reconcile every conflict in one batch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RelationPolicy {
    /// Overwrite every existing file.
    ReplaceAll,
    /// Transfer only the non-colliding sources.
    SkipAll,
    /// Write every source under a fresh "copy" name; nothing is lost.
    KeepBoth,
    /// Overwrite only where the source is newer; keep the destination otherwise.
    KeepNewer,
    /// Overwrite only where the source is larger; keep the destination otherwise.
    KeepLarger,
}

/// The engine-level outcome a resolution maps to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// Overwrite on collision (kept conflicts are all winners).
    Overwrite,
    /// Keep both, the engine renames the incoming file.
    KeepBoth,
    /// Skip any collision at copy time (the engine leaves the destination).
    Skip,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Resolution {
    /// Source paths to actually transfer (some conflicts dropped per policy).
    pub keep: Vec<PathBuf>,
    pub decision: Decision,
}

/// Decide which sources to transfer and the engine decision for `policy`.
/// Non-conflicting sources are always kept; only conflict losers are dropped.
pub fn resolve(
    sources: &[FileEntry],
    conflicts: &[Conflict],
    policy: RelationPolicy,
) -> Resolution {
    // SkipAll removes known collisions up front (the engine still uses SkipAll
    // for races that appear after confirmation); KeepNewer/KeepLarger pre-drop
    // conflict losers and overwrite the rest; ReplaceAll/KeepBoth keep all.
    let drop: HashSet<PathBuf> = match policy {
        RelationPolicy::ReplaceAll | RelationPolicy::KeepBoth => HashSet::new(),
        RelationPolicy::SkipAll => conflicts.iter().map(|c| c.src_path.clone()).collect(),
        RelationPolicy::KeepNewer => conflicts
            .iter()
            .filter(|c| c.src_is_dir || c.dst_is_dir || !c.src_newer)
            .map(|c| c.src_path.clone())
            .collect(),
        RelationPolicy::KeepLarger => conflicts
            .iter()
            .filter(|c| c.src_is_dir || c.dst_is_dir || c.src_size <= c.dst_size)
            .map(|c| c.src_path.clone())
            .collect(),
    };
    let keep = sources
        .iter()
        .map(|s| s.path.clone())
        .filter(|p| !drop.contains(p))
        .collect();
    let decision = match policy {
        RelationPolicy::KeepBoth => Decision::KeepBoth,
        RelationPolicy::SkipAll => Decision::Skip,
        _ => Decision::Overwrite,
    };
    Resolution { keep, decision }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, size: u64, secs: Option<u64>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from(format!("/src/{name}")),
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: false,
            size,
            extension: String::new(),
            modified: secs.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
            modified_str: "-".to_string(),
            size_str: crate::panel::format_size(size),
        }
    }

    fn dst(name: &str, size: u64, secs: Option<u64>) -> FileEntry {
        let mut e = entry(name, size, secs);
        e.path = PathBuf::from(format!("/dst/{name}"));
        e
    }

    fn dir(mut entry: FileEntry) -> FileEntry {
        entry.is_dir = true;
        entry.size = 0;
        entry
    }

    #[test]
    fn detect_matches_case_insensitively_with_flags() {
        let sources = [
            entry("Photo.JPG", 10, Some(200)),
            entry("only.txt", 1, None),
        ];
        let dest = [dst("photo.jpg", 99, Some(100))];
        let conflicts = detect(&sources, &dest);
        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(c.name, "Photo.JPG");
        assert_eq!((c.src_size, c.dst_size), (10, 99));
        assert!(c.src_newer && !c.dst_newer); // 200 > 100
    }

    #[test]
    fn detect_judges_case_variants_against_the_right_file() {
        // Case-sensitive destination holding both variants: the exact name
        // decides, whichever order the listing came in.
        let older_small = dst("Foo", 1, Some(100));
        let newer_large = dst("foo", 99, Some(900));
        for dest in [
            [older_small.clone(), newer_large.clone()],
            [newer_large.clone(), older_small.clone()],
        ] {
            let conflicts = detect(&[entry("Foo", 10, Some(500))], &dest);
            assert_eq!(conflicts.len(), 1);
            assert_eq!(conflicts[0].dst_size, 1);
            assert!(conflicts[0].src_newer);
        }
        // No exact variant: the source must beat every variant to win.
        let conflicts = detect(&[entry("FOO", 10, Some(500))], &[older_small, newer_large]);
        assert_eq!(conflicts[0].dst_size, 99);
        assert!(!conflicts[0].src_newer && conflicts[0].dst_newer);
    }

    #[test]
    fn conditional_policies_fail_closed_for_directories_and_equal_files() {
        let sources = vec![
            dir(entry("folder", 0, Some(300))),
            entry("equal.txt", 10, Some(100)),
        ];
        let destinations = vec![
            dir(dst("folder", 0, Some(100))),
            dst("equal.txt", 10, Some(100)),
        ];
        let conflicts = detect(&sources, &destinations);

        assert!(
            resolve(&sources, &conflicts, RelationPolicy::KeepNewer)
                .keep
                .is_empty()
        );
        assert!(
            resolve(&sources, &conflicts, RelationPolicy::KeepLarger)
                .keep
                .is_empty()
        );
    }

    fn sample() -> (Vec<FileEntry>, Vec<Conflict>) {
        // a.txt: dst newer + larger; b.txt: src newer + larger; c.txt: no conflict.
        let sources = vec![
            entry("a.txt", 5, Some(100)),
            entry("b.txt", 50, Some(300)),
            entry("c.txt", 1, Some(100)),
        ];
        let dest = vec![dst("a.txt", 20, Some(200)), dst("b.txt", 10, Some(100))];
        let conflicts = detect(&sources, &dest);
        (sources, conflicts)
    }

    #[test]
    fn replace_all_keeps_everything_and_overwrites() {
        let (s, c) = sample();
        let r = resolve(&s, &c, RelationPolicy::ReplaceAll);
        assert_eq!(r.keep.len(), 3);
        assert_eq!(r.decision, Decision::Overwrite);
    }

    #[test]
    fn skip_all_drops_known_conflicts_with_skip_decision() {
        let (s, c) = sample();
        let r = resolve(&s, &c, RelationPolicy::SkipAll);
        assert_eq!(r.keep, vec![PathBuf::from("/src/c.txt")]);
        assert_eq!(r.decision, Decision::Skip);
    }

    #[test]
    fn keep_both_keeps_all_with_keepboth_decision() {
        let (s, c) = sample();
        let r = resolve(&s, &c, RelationPolicy::KeepBoth);
        assert_eq!(r.keep.len(), 3);
        assert_eq!(r.decision, Decision::KeepBoth);
    }

    #[test]
    fn keep_newer_drops_only_where_dst_is_newer() {
        let (s, c) = sample();
        let r = resolve(&s, &c, RelationPolicy::KeepNewer);
        let names: Vec<String> = r
            .keep
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        // a.txt dropped (dst newer); b.txt kept (src newer); c.txt kept.
        assert!(!names.contains(&"a.txt".to_string()));
        assert!(names.contains(&"b.txt".to_string()));
        assert!(names.contains(&"c.txt".to_string()));
    }

    #[test]
    fn keep_larger_drops_only_where_dst_is_larger() {
        let (s, c) = sample();
        let r = resolve(&s, &c, RelationPolicy::KeepLarger);
        let names: Vec<String> = r
            .keep
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        // a.txt dst larger (20 > 5) -> dropped; b.txt src larger -> kept.
        assert!(!names.contains(&"a.txt".to_string()));
        assert!(names.contains(&"b.txt".to_string()));
        assert!(names.contains(&"c.txt".to_string()));
    }
}
