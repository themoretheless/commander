//! Bounded, read-only JSON of the retained listing and its current projection.
use crate::panel::{DirStatus, FileEntry, ListingIdentity, PanelState};
use serde::{
    Serialize, Serializer,
    ser::{Error, SerializeSeq},
};
use std::{
    io::{self, Write},
    path::{Component, Path},
};
const LIMIT: usize = 1024 * 1024;
struct Output(Vec<u8>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > LIMIT.saturating_sub(self.0.len()) {
            return Err(io::Error::other(
                "Listing JSON reports are limited to 1 MiB",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn path(path: &Path) -> Result<&str, String> {
    let text = path
        .to_str()
        .ok_or("Listing JSON needs lossless UTF-8 paths")?;
    if !path.is_absolute()
        || text.contains('\0')
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Listing JSON needs absolute paths without NUL or parent traversal".into());
    }
    Ok(text)
}
#[derive(Serialize)]
struct Document<'a> {
    format: &'static str,
    version: u32,
    snapshot: &'static str,
    folder: &'a str,
    scope: &'static str,
    view: crate::panel::ViewConfig,
    query: &'a str,
    facets: crate::panel::FacetSet,
    listing_rows: usize,
    visible_rows: usize,
    selected_rows: usize,
    hidden_selected_rows: usize,
    exported_rows: usize,
    rows: Rows<'a>,
}
struct Rows<'a>(&'a [&'a FileEntry]);
#[derive(Serialize)]
struct Row<'a> {
    row: usize,
    name: &'a str,
    path: &'a str,
    kind: Option<crate::path_identity::PathKind>,
    file_size_bytes: Option<u64>,
    device_id: Option<u64>,
    file_id: Option<u64>,
    hard_links: Option<u64>,
    owner_uid: Option<u64>,
    owner_gid: Option<u64>,
    allocated_bytes: Option<u64>,
    displayed_size: &'a str,
    modified_utc: Option<String>,
    accessed_utc: Option<String>,
    created_utc: Option<String>,
    changed_utc: Option<String>,
}
impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for (index, entry) in self.0.iter().enumerate() {
            let path = path(&entry.path).map_err(S::Error::custom)?;
            if entry.name.contains('\0') {
                return Err(S::Error::custom("Listing JSON names must not contain NUL"));
            }
            let kind = match &entry.identity {
                ListingIdentity::Captured(i) => i.kind,
                _ => None,
            };
            seq.serialize_element(&Row {
                row: index + 1,
                name: &entry.name,
                path,
                kind,
                device_id: entry.filesystem_id().map(|id| id.0),
                owner_uid: entry.owner_uid,
                owner_gid: entry.owner_gid,
                file_id: entry.filesystem_id().map(|id| id.1),
                hard_links: if kind == Some(crate::path_identity::PathKind::File) {
                    entry.hard_link_count()
                } else {
                    None
                },
                file_size_bytes: (kind == Some(crate::path_identity::PathKind::File))
                    .then_some(entry.size),
                allocated_bytes: if kind == Some(crate::path_identity::PathKind::File) {
                    entry.allocated_bytes()
                } else {
                    None
                },
                displayed_size: entry.size_display(),
                accessed_utc: entry.accessed.map(|time| {
                    crate::listing_export::iso_8601_utc_precision(
                        time,
                        chrono::SecondsFormat::AutoSi,
                    )
                }),
                created_utc: entry.created.map(|time| {
                    crate::listing_export::iso_8601_utc_precision(
                        time,
                        chrono::SecondsFormat::AutoSi,
                    )
                }),
                changed_utc: entry.changed.map(|time| {
                    crate::listing_export::iso_8601_utc_precision(
                        time,
                        chrono::SecondsFormat::AutoSi,
                    )
                }),
                modified_utc: entry.modified.map(|time| {
                    crate::listing_export::iso_8601_utc_precision(
                        time,
                        chrono::SecondsFormat::AutoSi,
                    )
                }),
            })?;
        }
        seq.end()
    }
}
pub fn json(panel: &PanelState) -> Result<String, String> {
    if !matches!(
        panel.dir_status(),
        DirStatus::Listed | DirStatus::Empty | DirStatus::HiddenOnly(_)
    ) {
        return Err("Wait for a complete directory listing before copying JSON".into());
    }
    let selected = panel.selected_count();
    let entries = panel.listing_entries();
    let document = Document {
        format: "commander.listing-report",
        version: 1,
        snapshot: "retained_listing_metadata",
        folder: path(&panel.current_path)?,
        scope: if selected == 0 {
            "visible_rows"
        } else {
            "visible_selected_rows"
        },
        view: panel.view_config(),
        query: panel.search_query(),
        facets: panel.facets(),
        listing_rows: panel.entries().len(),
        visible_rows: panel.filtered_entries().len(),
        selected_rows: selected,
        hidden_selected_rows: if selected == 0 {
            0
        } else {
            selected.saturating_sub(entries.len())
        },
        exported_rows: entries.len(),
        rows: Rows(&entries),
    };
    let mut output = Output(Vec::new());
    serde_json::to_writer_pretty(&mut output, &document).map_err(|e| e.to_string())?;
    String::from_utf8(output.0).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn listed(path: &Path) -> PanelState {
        let mut panel = PanelState::new(path.to_path_buf());
        panel.refresh();
        panel
    }
    #[test]
    fn alternate_dates_json_keeps_subseconds_pre_epoch_and_unknowns_without_restat() {
        let dir = crate::testutil::TempDir::new();
        let file = dir.file("time", "x");
        let mut entry = FileEntry::observe_lexical(file.clone()).unwrap();
        entry.accessed = Some(std::time::UNIX_EPOCH - std::time::Duration::from_nanos(1));
        entry.created = Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(123));
        entry.changed = Some(std::time::UNIX_EPOCH - std::time::Duration::from_nanos(123));
        std::fs::remove_file(file).unwrap();
        let rows: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&Rows(&[&entry])).unwrap()).unwrap();
        assert_eq!(rows[0]["accessed_utc"], "1969-12-31T23:59:59.999999999Z");
        assert_eq!(rows[0]["created_utc"], "1970-01-01T00:00:00.000000123Z");
        assert_eq!(rows[0]["changed_utc"], "1969-12-31T23:59:59.999999877Z");
        entry.changed = None;
        entry.accessed = None;
        entry.created = None;
        let rows: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&Rows(&[&entry])).unwrap()).unwrap();
        assert!(
            rows[0]["accessed_utc"].is_null()
                && rows[0]["created_utc"].is_null()
                && rows[0]["changed_utc"].is_null()
        );
    }

    #[test]
    fn owner_metadata_json_keeps_exact_cached_values_and_null_unknowns() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("file", "x");
        let mut entry =
            FileEntry::from_meta(path.clone(), &std::fs::metadata(&path).unwrap()).unwrap();
        std::fs::remove_file(path).unwrap();
        entry.owner_uid = Some(u64::MAX);
        entry.owner_gid = Some(0);
        let rows: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&Rows(&[&entry])).unwrap()).unwrap();
        assert_eq!(rows[0]["owner_uid"].as_u64(), Some(u64::MAX));
        assert_eq!(rows[0]["owner_gid"], 0);
        entry.owner_uid = None;
        entry.owner_gid = None;
        let rows: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&Rows(&[&entry])).unwrap()).unwrap();
        assert!(rows[0]["owner_uid"].is_null() && rows[0]["owner_gid"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_identity_report_keeps_shared_inode_and_lexical_link_facts() {
        let dir = crate::testutil::TempDir::new();
        let file = dir.file("file", "abc");
        let alias = dir.path().join("alias");
        std::fs::hard_link(&file, &alias).unwrap();
        std::os::unix::fs::symlink(&file, dir.path().join("link")).unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&json(&listed(dir.path())).unwrap()).unwrap();
        let rows = report["rows"].as_array().unwrap();
        let get = |name| rows.iter().find(|r| r["name"] == name).unwrap();
        let a = get("file");
        let b = get("alias");
        let link = get("link");
        assert_eq!(a["device_id"], b["device_id"]);
        assert_eq!(a["file_id"], b["file_id"]);
        assert_eq!(a["hard_links"], 2);
        assert_eq!(b["hard_links"], 2);
        assert_ne!(link["file_id"], a["file_id"]);
        assert_eq!(link["device_id"], a["device_id"]);
        assert!(link["hard_links"].is_null());
        use std::os::unix::fs::MetadataExt;
        for name in ["file", "alias", "link"] {
            let raw = std::fs::symlink_metadata(dir.path().join(name)).unwrap();
            assert_eq!(get(name)["owner_uid"], raw.uid());
            assert_eq!(get(name)["owner_gid"], raw.gid());
        }
    }

    #[test]
    fn json_listing_preserves_projection_sizes_selection_and_snapshot_after_removal() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.file("a.bin", "abc");
        let b = dir.file("b.bin", "def");
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        let mut panel = listed(dir.path());
        panel.set_sort(crate::panel::SortColumn::Allocated);
        let full: serde_json::Value = serde_json::from_str(&json(&panel).unwrap()).unwrap();
        assert_eq!(full["scope"], "visible_rows");
        assert_eq!(full["exported_rows"], 3);
        let folder = full["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "folder")
            .unwrap();
        assert!(folder["file_size_bytes"].is_null() && folder["allocated_bytes"].is_null());
        panel.replace_selection([a.clone(), b.clone()]);
        panel.set_search_query("a.bin");
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
        let selected: serde_json::Value = serde_json::from_str(&json(&panel).unwrap()).unwrap();
        assert_eq!(selected["scope"], "visible_selected_rows");
        assert_eq!(selected["visible_rows"], 1);
        assert_eq!(selected["selected_rows"], 2);
        assert_eq!(selected["hidden_selected_rows"], 1);
        assert_eq!(selected["rows"][0]["file_size_bytes"], 3);
        assert_eq!(selected["rows"][0]["name"], "a.bin");
        assert_eq!(selected["view"]["sort_col"], "Allocated");
        assert!(json(&listed(dir.path().join("missing").as_path())).is_err());
    }
    #[test]
    fn json_listing_escapes_controls_and_rejects_oversize_or_invalid_paths() {
        let dir = crate::testutil::TempDir::new();
        let source = dir.file("a", "x");
        let mut entry =
            FileEntry::from_meta(source.clone(), &std::fs::metadata(&source).unwrap()).unwrap();
        entry.name = "=formula,\"\n\t".into();
        entry.size = u64::MAX;
        entry.allocated_size = Some(0);
        entry.modified = Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(123));
        let mut output = Output(Vec::new());
        serde_json::to_writer(&mut output, &Rows(&[&entry])).unwrap();
        let rows: serde_json::Value = serde_json::from_slice(&output.0).unwrap();
        assert_eq!(rows[0]["name"], entry.name);
        assert_eq!(rows[0]["file_size_bytes"].as_u64(), Some(u64::MAX));
        assert_eq!(rows[0]["allocated_bytes"], 0);
        assert_eq!(rows[0]["modified_utc"], "1970-01-01T00:00:00.000000123Z");
        assert!(path(Path::new("/bad\0path")).is_err());
        entry.name = "x".repeat(LIMIT + 1);
        let mut output = Output(Vec::new());
        assert!(serde_json::to_writer(&mut output, &Rows(&[&entry])).is_err());
        assert!(output.0.len() <= LIMIT);
        entry.name = "a".into();
        entry.path = Path::new("relative/a").to_path_buf();
        assert!(serde_json::to_string(&Rows(&[&entry])).is_err());
        entry.path = dir.path().join("../a");
        assert!(serde_json::to_string(&Rows(&[&entry])).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn json_listing_rejects_lossy_paths_and_does_not_report_link_target_allocation() {
        use std::os::unix::ffi::OsStringExt;
        let dir = crate::testutil::TempDir::new();
        let file = dir.file("file", "abc");
        std::os::unix::fs::symlink(&file, dir.path().join("link")).unwrap();
        let p = listed(dir.path());
        let report: serde_json::Value = serde_json::from_str(&json(&p).unwrap()).unwrap();
        let link = report["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "link")
            .unwrap();
        assert_eq!(link["kind"], "Symlink");
        assert!(link["file_size_bytes"].is_null() && link["allocated_bytes"].is_null());
        let mut entry = FileEntry::observe_lexical(file).unwrap();
        entry.path = dir
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'x', 255]));
        assert!(
            serde_json::to_string(&Rows(&[&entry]))
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
        let mut p = p;
        p.current_path = entry.path;
        assert!(json(&p).unwrap_err().contains("UTF-8"));
    }
}
