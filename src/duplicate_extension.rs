//! Optional extension partitioning of already content-verified duplicate groups.
use crate::dedup::DupGroup;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Mode {
    #[default]
    Any,
    CaseSensitive,
    IgnoreAsciiCase,
}

/// Retain scan/member order. Cancellation drops the entire unpublished result.
/// Existing hard-link representative selection has already happened upstream.
pub fn partition(
    groups: Vec<DupGroup>,
    mode: Mode,
    min_copies: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Vec<DupGroup> {
    if mode == Mode::Any {
        return retain_groups(groups, min_copies, cancelled);
    }
    partition_by(groups, min_copies, &mut cancelled, |path| {
        let mut extension = path
            .extension()
            .unwrap_or_default()
            .as_encoded_bytes()
            .to_vec();
        if mode == Mode::IgnoreAsciiCase {
            extension.make_ascii_lowercase();
        }
        Some(extension)
    })
}

pub fn retain_groups(
    groups: Vec<DupGroup>,
    minimum: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Vec<DupGroup> {
    let mut result = Vec::new();
    for group in groups {
        if cancelled() {
            return Vec::new();
        }
        if group.files.len() >= minimum.max(2) {
            result.push(group);
        }
    }
    result
}

/// Shared stable partitioner for content-verified groups; absent components never match.
pub fn partition_by(
    groups: Vec<DupGroup>,
    min_copies: usize,
    mut cancelled: impl FnMut() -> bool,
    mut key: impl FnMut(&std::path::Path) -> Option<Vec<u8>>,
) -> Vec<DupGroup> {
    let minimum = min_copies.max(2);
    let mut output = Vec::new();
    for group in groups {
        if cancelled() {
            return Vec::new();
        }
        let mut indices: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut partitions: Vec<DupGroup> = Vec::new();
        for file in group.files {
            if cancelled() {
                return Vec::new();
            }
            let Some(component) = key(&file.path) else {
                continue;
            };
            let next = partitions.len();
            let index = *indices.entry(component).or_insert(next);
            if index == next {
                partitions.push(DupGroup {
                    size: group.size,
                    files: Vec::new(),
                });
            }
            partitions[index].files.push(file);
        }
        output.extend(partitions.into_iter().filter(|g| g.files.len() >= minimum));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    fn group(names: &[&str]) -> DupGroup {
        DupGroup {
            size: 10,
            files: names
                .iter()
                .map(|name| crate::dedup::FileKey {
                    path: (*name).into(),
                    identity: crate::panel::ListingIdentity::Unavailable,
                    size: 10,
                    hash: 1,
                    modified: None,
                })
                .collect(),
        }
    }
    fn names(groups: Vec<DupGroup>) -> Vec<Vec<String>> {
        groups
            .into_iter()
            .map(|g| {
                g.files
                    .into_iter()
                    .map(|f| f.path.to_string_lossy().into_owned())
                    .collect()
            })
            .collect()
    }
    #[test]
    fn extension_partition_preserves_order_case_threshold_and_empty_extensions() {
        let input = group(&[
            "b.txt", "a.bin", "c.TXT", "d.txt", "e.bin", "plain", "hidden.",
        ]);
        assert_eq!(
            names(partition(
                vec![input.clone()],
                Mode::CaseSensitive,
                2,
                || false
            )),
            vec![
                vec!["b.txt", "d.txt"],
                vec!["a.bin", "e.bin"],
                vec!["plain", "hidden."]
            ]
        );
        assert_eq!(
            names(partition(
                vec![input.clone()],
                Mode::IgnoreAsciiCase,
                3,
                || false
            )),
            vec![vec!["b.txt", "c.TXT", "d.txt"]]
        );
        assert_eq!(
            partition(vec![input.clone()], Mode::Any, 2, || false),
            vec![input]
        );
        assert!(
            partition(
                vec![group(&["a.É", "b.é"])],
                Mode::IgnoreAsciiCase,
                2,
                || false
            )
            .is_empty()
        );
        let mut ticks = 0;
        assert!(
            partition(
                vec![group(&["a.txt", "b.txt"]), group(&["c.txt", "d.txt"])],
                Mode::CaseSensitive,
                2,
                || {
                    ticks += 1;
                    ticks == 5
                }
            )
            .is_empty()
        );
    }
    #[cfg(unix)]
    #[test]
    fn extension_partition_never_aliases_non_utf8_with_lossy_labels() {
        use std::os::unix::ffi::OsStringExt;
        let mut input = group(&["a.�", "b.�", "c.txt", "d.txt"]);
        input.files[0].path = std::ffi::OsString::from_vec(b"a.\xff".to_vec()).into();
        input.files[1].path = std::ffi::OsString::from_vec(b"b.\xff".to_vec()).into();
        let mut unicode = group(&["e.�", "f.�"]);
        input.files.append(&mut unicode.files);
        let result = partition(vec![input], Mode::IgnoreAsciiCase, 2, || false);
        assert_eq!(result.len(), 3);
        assert_eq!(
            result[0].files[0]
                .path
                .extension()
                .unwrap()
                .as_encoded_bytes(),
            b"\xff"
        );
        assert_eq!(result[2].files[0].path.extension().unwrap(), "�");
    }
}
