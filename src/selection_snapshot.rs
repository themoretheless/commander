//! Explicit tab-owned selection bookmarks; loading never selects files.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Component, Path, PathBuf},
};
const MAX_NAMES: usize = 1024;
const MAX_NATIVE: usize = 14 * 1024;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot(Option<(PathBuf, HashSet<OsString>)>);
#[cfg(unix)]
fn encode(path: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_bytes().to_vec()
}
#[cfg(windows)]
fn encode(path: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.encode_wide().collect()
}
#[cfg(unix)]
fn decode(value: &serde_json::Value) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;
    let bytes: Vec<u8> = serde_json::from_value(value.clone()).ok()?;
    (!bytes.contains(&0)).then(|| OsString::from_vec(bytes))
}
#[cfg(windows)]
fn decode(value: &serde_json::Value) -> Option<OsString> {
    use std::os::windows::ffi::OsStringExt;
    let units: Vec<u16> = serde_json::from_value(value.clone()).ok()?;
    (!units.contains(&0)).then(|| OsString::from_wide(&units))
}
fn native_cost(name: &std::ffi::OsStr) -> usize {
    let native = encode(name);
    std::mem::size_of_val(native.as_slice())
}
fn leaf(name: &std::ffi::OsStr) -> bool {
    let mut c = Path::new(name).components();
    matches!(c.next(), Some(Component::Normal(_)))
        && c.next().is_none()
        && Path::new(name).file_name() == Some(name)
}
impl Snapshot {
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }
    pub fn belongs_to(&self, root: &Path) -> bool {
        self.0.as_ref().is_some_and(|(p, _)| p == root)
    }
    pub fn contains(&self, path: &Path) -> bool {
        self.0.as_ref().is_some_and(|(root, names)| {
            path.parent() == Some(root) && path.file_name().is_some_and(|n| names.contains(n))
        })
    }
    pub fn capture(root: &Path, paths: &HashSet<PathBuf>) -> Result<Self, String> {
        if !root.is_absolute() || paths.len() > MAX_NAMES {
            return Err(
                "Tab selection needs an absolute folder and at most 1,024 selected names".into(),
            );
        }
        let mut bytes = native_cost(root.as_os_str());
        let mut names = HashSet::new();
        for path in paths {
            let name = path
                .file_name()
                .filter(|n| leaf(n))
                .ok_or("Invalid selected filename")?;
            if path.parent() != Some(root) {
                return Err("Tab selection must contain only this folder's entries".into());
            }
            if encode(name).contains(&0) {
                return Err("Invalid selected filename".into());
            }
            bytes = bytes.saturating_add(native_cost(name));
            if bytes > MAX_NATIVE {
                return Err("Tab selection exceeds its 14 KiB native-path budget; the previous snapshot was kept".into());
            }
            names.insert(name.to_owned());
        }
        if bytes > MAX_NATIVE || encode(root.as_os_str()).contains(&0) {
            return Err("Tab selection folder exceeds its native-path budget".into());
        }
        Ok(Self(Some((root.to_owned(), names))))
    }
}
impl Serialize for Snapshot {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Some((root, names)) = &self.0 else {
            return s.serialize_none();
        };
        let mut names: Vec<_> = names.iter().map(|n| encode(n)).collect();
        names.sort();
        serde_json::json!({"version":1,"platform":std::env::consts::FAMILY,"root":encode(root.as_os_str()),"names":names}).serialize(s)
    }
}
impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let parse = || -> Option<Self> {
            if value["version"] != 1
                || value["platform"].as_str() != Some(std::env::consts::FAMILY)
                || serde_json::to_vec(&value).ok()?.len() > 65536
            {
                return None;
            }
            let root = PathBuf::from(decode(&value["root"])?);
            let rows = value["names"].as_array()?;
            if rows.len() > MAX_NAMES {
                return None;
            }
            let mut names = HashSet::new();
            for row in rows {
                let name = decode(row)?;
                if !leaf(&name) || !names.insert(root.join(name)) {
                    return None;
                }
            }
            Self::capture(&root, &names).ok()
        };
        Ok(parse().unwrap_or_default())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_round_trip_is_bounded_and_not_partial() {
        let root = PathBuf::from("/fixture");
        let paths = [root.join("a\n*.txt"), root.join("ёж")]
            .into_iter()
            .collect();
        let saved = Snapshot::capture(&root, &paths).unwrap();
        let encoded = serde_json::to_vec(&saved).unwrap();
        let back: Snapshot = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(back, saved);
        assert!(back.contains(&root.join("ёж")));
        assert!(!back.contains(Path::new("/elsewhere/ёж")));
        assert!(Snapshot::capture(&root, &[root.join("a/child")].into_iter().collect()).is_err());
        let big = (0..1025).map(|n| root.join(n.to_string())).collect();
        assert!(Snapshot::capture(&root, &big).is_err());
        let long = (0..100)
            .map(|n| root.join(format!("{n}{}", "x".repeat(200))))
            .collect();
        assert!(Snapshot::capture(&root, &long).is_err());
    }
    #[test]
    fn optional_corruption_never_restores_a_partial_set() {
        let good = serde_json::to_value(
            Snapshot::capture(
                Path::new("/fixture"),
                &[PathBuf::from("/fixture/a")].into_iter().collect(),
            )
            .unwrap(),
        )
        .unwrap();
        for field in ["version", "root", "names", "platform"] {
            let mut bad = good.clone();
            bad[field] = serde_json::Value::Null;
            assert!(serde_json::from_value::<Snapshot>(bad).unwrap().is_none());
        }
        let mut bad = good.clone();
        bad["names"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!([46, 46]));
        assert!(serde_json::from_value::<Snapshot>(bad).unwrap().is_none());
        let empty = Snapshot::capture(Path::new("/fixture"), &HashSet::new()).unwrap();
        assert!(!empty.is_none());
        assert_eq!(
            serde_json::from_value::<Snapshot>(serde_json::to_value(&empty).unwrap()).unwrap(),
            empty
        );
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_exact_native_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let root = PathBuf::from("/fixture");
        let name = OsString::from_vec(vec![b'a', 255]);
        let saved = Snapshot::capture(&root, &[root.join(&name)].into_iter().collect()).unwrap();
        let back: Snapshot = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert!(back.contains(&root.join(name)));
    }
}
