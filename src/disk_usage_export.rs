//! Lossless, bounded reports of the existing retained disk-usage snapshot.
use crate::disk_usage_metric::{Measurement, Metric};
use crate::{tree_filter::View, tree_overview::OverviewSnapshot};
use serde::Serialize;
use std::{
    io::{self, Write},
    path::{Component, Path},
};
const LIMIT: usize = 1024 * 1024;
#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > LIMIT.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("Disk-usage reports are limited to 1 MiB"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn text(path: &Path) -> Result<&str, String> {
    let text = path.to_str().ok_or("Report paths must be lossless UTF-8")?;
    if !path.is_absolute()
        || text.contains('\0')
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("Report paths must be absolute and contain no NUL or parent traversal".into());
    }
    Ok(text)
}
fn relative<'a>(root: &Path, path: &'a Path) -> Result<&'a str, String> {
    text(path)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "Report row is outside the scan root")?;
    if relative.as_os_str().is_empty() {
        return Err("Report row has no relative path".into());
    }
    relative
        .to_str()
        .ok_or_else(|| "Report paths must be lossless UTF-8".into())
}
fn validate(snapshot: &OverviewSnapshot) -> Result<&str, String> {
    let root = text(&snapshot.root)?;
    if snapshot.nodes.len() > crate::tree_overview::NODE_CAP
        || snapshot.top_level.len() > crate::tree_overview::TOP_LEVEL_CAP
        || snapshot.inaccessible_paths.len() > snapshot.inaccessible_count
    {
        return Err("Scan snapshot exceeds its declared bounds".into());
    }
    Ok(root)
}
#[derive(Serialize)]
struct Directory<'a> {
    path: &'a str,
    relative_path: &'a str,
    label: &'a str,
    depth: usize,
    logical_bytes: Option<u64>,
    file_allocation_bytes: Option<u64>,
    known_bytes: Option<u64>,
    known_value: u64,
    file_count: Option<u64>,
    measurement: Measurement,
    measurement_complete: bool,
    files: usize,
    unreadable: bool,
}
#[derive(Serialize)]
struct MapItem<'a> {
    path: &'a str,
    relative_path: &'a str,
    is_directory: bool,
    logical_bytes: Option<u64>,
    file_allocation_bytes: Option<u64>,
    known_bytes: Option<u64>,
    known_value: u64,
    file_count: Option<u64>,
    measurement: Measurement,
    measurement_complete: bool,
}
#[derive(Serialize)]
struct Summary {
    logical_bytes: Option<u64>,
    file_allocation_bytes: Option<u64>,
    known_bytes: Option<u64>,
    known_value: u64,
    file_count: Option<u64>,
    measurement: Measurement,
    measurement_complete: bool,
    entries: usize,
    files: usize,
    directories: usize,
}
#[derive(Serialize)]
struct ScanStatus<'a> {
    cancelled: bool,
    tree_truncated: bool,
    map_truncated: bool,
    inaccessible_count: usize,
    inaccessible_paths_truncated: bool,
    inaccessible_paths: Vec<&'a str>,
}
#[derive(Serialize)]
struct JsonReport<'a> {
    metric: Metric,
    measurement_unit: &'static str,
    measurement_scope: &'static str,
    schema: u8,
    scope: &'static str,
    root: &'a str,
    directory_rows_are_nested_totals: bool,
    summary: Summary,
    status: ScanStatus<'a>,
    map_items: Vec<MapItem<'a>>,
    directories: Vec<Directory<'a>>,
}

pub fn snapshot_json(snapshot: &OverviewSnapshot) -> Result<String, String> {
    let root = validate(snapshot)?;
    let directories = snapshot
        .nodes
        .iter()
        .map(|node| {
            Ok(Directory {
                path: text(&node.path)?,
                relative_path: relative(&snapshot.root, &node.path)?,
                label: &node.label,
                depth: node.depth,
                logical_bytes: (snapshot.metric == Metric::Logical).then_some(node.bytes),
                file_allocation_bytes: (snapshot.metric == Metric::FileAllocation
                    && !snapshot.cancelled
                    && !node.unreadable)
                    .then(|| node.measurement.value(node.bytes))
                    .flatten(),
                known_bytes: (snapshot.metric != Metric::FileCount).then_some(node.bytes),
                known_value: node.bytes,
                file_count: (snapshot.metric == Metric::FileCount
                    && node.measurement.complete()
                    && !snapshot.cancelled
                    && !node.unreadable)
                    .then(|| node.measurement.value(node.bytes))
                    .flatten(),
                measurement: node.measurement,
                measurement_complete: node.measurement.complete()
                    && !snapshot.cancelled
                    && !node.unreadable,
                files: node.files,
                unreadable: node.unreadable,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let map_items = snapshot
        .top_level
        .iter()
        .map(|(entry, bytes)| {
            Ok(MapItem {
                path: text(&entry.path)?,
                relative_path: relative(&snapshot.root, &entry.path)?,
                is_directory: entry.is_dir,
                logical_bytes: (snapshot.metric == Metric::Logical).then_some(*bytes),
                file_allocation_bytes: (snapshot.metric == Metric::FileAllocation
                    && !snapshot.cancelled)
                    .then(|| snapshot.measurement_for(&entry.path).value(*bytes))
                    .flatten(),
                known_bytes: (snapshot.metric != Metric::FileCount).then_some(*bytes),
                known_value: *bytes,
                file_count: (snapshot.metric == Metric::FileCount
                    && snapshot.measurement_for(&entry.path).complete()
                    && !snapshot.cancelled)
                    .then(|| snapshot.measurement_for(&entry.path).value(*bytes))
                    .flatten(),
                measurement: snapshot.measurement_for(&entry.path),
                measurement_complete: snapshot.measurement_for(&entry.path).complete()
                    && !snapshot.cancelled,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let inaccessible_paths = snapshot
        .inaccessible_paths
        .iter()
        .map(|path| {
            let text = text(path)?;
            if !path.starts_with(&snapshot.root) {
                return Err("Unreadable path is outside the scan root".into());
            }
            Ok(text)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let report = JsonReport {
        metric: snapshot.metric,
        measurement_unit: snapshot.metric.unit(),
        measurement_scope: match snapshot.metric {
            Metric::FileAllocation => "regular_file_data_blocks_once_per_identity",
            Metric::FileCount => "non_directory_entries_including_hard_link_aliases",
            Metric::Logical => "non_directory_entry_lengths_once_per_identity",
        },
        schema: 1,
        scope: "bounded_disk_usage_snapshot",
        root,
        directory_rows_are_nested_totals: true,
        summary: Summary {
            logical_bytes: (snapshot.metric == Metric::Logical).then_some(snapshot.progress.bytes),
            file_allocation_bytes: (snapshot.metric == Metric::FileAllocation
                && snapshot.measurement_complete())
            .then(|| snapshot.progress.measurement.value(snapshot.progress.bytes))
            .flatten(),
            known_bytes: (snapshot.metric != Metric::FileCount).then_some(snapshot.progress.bytes),
            known_value: snapshot.progress.bytes,
            file_count: (snapshot.metric == Metric::FileCount && snapshot.measurement_complete())
                .then(|| snapshot.progress.measurement.value(snapshot.progress.bytes))
                .flatten(),
            measurement: snapshot.progress.measurement,
            measurement_complete: snapshot.measurement_complete(),
            entries: snapshot.progress.entries,
            files: snapshot.progress.files,
            directories: snapshot.progress.directories,
        },
        status: ScanStatus {
            cancelled: snapshot.cancelled,
            tree_truncated: snapshot.truncated,
            map_truncated: snapshot.top_level_truncated,
            inaccessible_count: snapshot.inaccessible_count,
            inaccessible_paths_truncated: snapshot.inaccessible_count > inaccessible_paths.len(),
            inaccessible_paths,
        },
        map_items,
        directories,
    };
    let mut output = Output::default();
    serde_json::to_writer_pretty(&mut output, &report).map_err(|error| error.to_string())?;
    String::from_utf8(output.bytes).map_err(|error| error.to_string())
}

pub fn visible_csv(snapshot: &OverviewSnapshot, view: &View) -> Result<String, String> {
    let root = validate(snapshot)?;
    if !view.is_current() {
        return Err("Wait for the displayed tree filter to update".into());
    }
    // Refuse stale/corrupt projection indices rather than exporting different rows.
    let mut projected = View::new(view.options.clone());
    projected.refresh(&snapshot.root, &snapshot.nodes);
    if projected.indices != view.indices
        || projected.direct != view.direct
        || projected.matches != view.matches
        || projected.has_children != view.has_children
    {
        return Err("Displayed tree projection is stale; update the view before copying".into());
    }
    if view.indices.is_empty() {
        return Err("No visible directories to copy".into());
    }
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Output::default());
    writer
        .write_record([
            "root",
            "path",
            "relative_path",
            "depth",
            "logical_bytes",
            "files",
            "unreadable",
            "context",
            "query",
            "fuzzy",
            "match_case",
            "scan_cancelled",
            "tree_truncated",
            "inaccessible_count",
            "collapsed",
            "metric",
            "known_bytes",
            "unknown_measurements",
            "measurement_overflowed",
            "measurement_complete",
            "file_allocation_bytes",
            "measurement_unit",
            "known_value",
            "file_count",
        ])
        .map_err(|error| error.to_string())?;
    for &index in &view.indices {
        let node = &snapshot.nodes[index];
        writer
            .write_record([
                root.to_owned(),
                text(&node.path)?.to_owned(),
                relative(&snapshot.root, &node.path)?.to_owned(),
                node.depth.to_string(),
                if snapshot.metric == Metric::Logical {
                    node.bytes.to_string()
                } else {
                    String::new()
                },
                node.files.to_string(),
                node.unreadable.to_string(),
                (!view.direct[index]).to_string(),
                view.options.query.clone(),
                view.options.fuzzy.to_string(),
                view.options.match_case.to_string(),
                snapshot.cancelled.to_string(),
                snapshot.truncated.to_string(),
                snapshot.inaccessible_count.to_string(),
                (view.options.query.trim().is_empty()
                    && view.options.collapsed.contains(&node.path)
                    && view.has_children[index])
                    .to_string(),
                snapshot.metric.key().to_string(),
                if snapshot.metric == Metric::FileCount {
                    String::new()
                } else {
                    node.bytes.to_string()
                },
                node.measurement.unknown.to_string(),
                node.measurement.overflowed.to_string(),
                (node.measurement.complete() && !snapshot.cancelled && !node.unreadable)
                    .to_string(),
                if snapshot.metric == Metric::FileAllocation
                    && !snapshot.cancelled
                    && !node.unreadable
                {
                    node.measurement
                        .value(node.bytes)
                        .map_or_else(String::new, |n| n.to_string())
                } else {
                    String::new()
                },
                snapshot.metric.unit().to_string(),
                node.bytes.to_string(),
                if snapshot.metric == Metric::FileCount
                    && node.measurement.complete()
                    && !snapshot.cancelled
                    && !node.unreadable
                {
                    node.bytes.to_string()
                } else {
                    String::new()
                },
            ])
            .map_err(|error| error.to_string())?;
    }
    let output = writer.into_inner().map_err(|error| error.to_string())?;
    String::from_utf8(output.bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree_overview::{OverviewNode, ScanProgress};
    fn snapshot() -> OverviewSnapshot {
        OverviewSnapshot {
            metric: Default::default(),
            map_measurements: Default::default(),
            root: "/fixture".into(),
            top_level: Vec::new(),
            top_level_truncated: false,
            nodes: vec![
                OverviewNode {
                    measurement: Default::default(),
                    path: "/fixture/branch".into(),
                    label: "branch".into(),
                    depth: 0,
                    bytes: 30,
                    files: 2,
                    unreadable: false,
                },
                OverviewNode {
                    measurement: Default::default(),
                    path: "/fixture/branch/target,\"line\nname".into(),
                    label: "target,\"line\nname".into(),
                    depth: 1,
                    bytes: 10,
                    files: 1,
                    unreadable: false,
                },
                OverviewNode {
                    measurement: Default::default(),
                    path: "/fixture/elsewhere".into(),
                    label: "elsewhere".into(),
                    depth: 0,
                    bytes: 50,
                    files: 1,
                    unreadable: false,
                },
            ],
            progress: ScanProgress {
                measurement: Default::default(),
                entries: 10,
                files: 4,
                directories: 3,
                bytes: 80,
                current: "/fixture".into(),
            },
            inaccessible_paths: Vec::new(),
            inaccessible_count: 0,
            truncated: false,
            cancelled: false,
            elapsed: Default::default(),
        }
    }
    #[test]
    fn file_counts_never_serialize_or_format_as_bytes() {
        let mut snapshot = snapshot();
        snapshot.metric = Metric::FileCount;
        snapshot.progress.bytes = snapshot.progress.files as u64;
        for node in &mut snapshot.nodes {
            node.bytes = node.files as u64;
        }
        let mut view = View::default();
        view.refresh(&snapshot.root, &snapshot.nodes);
        for unknown in [false, true] {
            snapshot.progress.measurement = if unknown {
                Measurement::unknown()
            } else {
                Measurement::default()
            };
            let report: serde_json::Value =
                serde_json::from_str(&snapshot_json(&snapshot).unwrap()).unwrap();
            assert_eq!(report["measurement_unit"], "entries");
            assert_eq!(
                report["measurement_scope"],
                "non_directory_entries_including_hard_link_aliases"
            );
            let summary = &report["summary"];
            assert!(
                summary["logical_bytes"].is_null()
                    && summary["file_allocation_bytes"].is_null()
                    && summary["known_bytes"].is_null()
            );
            assert_eq!(summary["known_value"], snapshot.progress.bytes);
            assert_eq!(
                summary["file_count"].as_u64(),
                (!unknown).then_some(snapshot.progress.bytes)
            );
        }
        let csv = visible_csv(&snapshot, &view).unwrap();
        let mut reader = csv::Reader::from_reader(csv.as_bytes());
        let header = reader.headers().unwrap().clone();
        for row in reader.records() {
            let row = row.unwrap();
            let get = |key: &str| {
                row.get(header.iter().position(|h| h == key).unwrap())
                    .unwrap()
            };
            assert_eq!(get("measurement_unit"), "entries");
            assert!(
                get("logical_bytes").is_empty()
                    && get("known_bytes").is_empty()
                    && get("file_allocation_bytes").is_empty()
            );
            assert_eq!(get("known_value"), get("file_count"));
        }
        assert_eq!(
            Metric::FileCount.text(u64::MAX, Measurement::default()),
            format!("{} entries", u64::MAX)
        );
        assert_eq!(
            Metric::FileCount.text(0, Measurement::unknown()),
            "≥0 entries"
        );
    }

    #[test]
    fn allocation_reports_name_the_metric_and_preserve_unknown_zero_and_overflow() {
        let mut snapshot = snapshot();
        snapshot.metric = Metric::FileAllocation;
        for (measurement, value) in [
            (Measurement::default(), Some(80)),
            (Measurement::unknown(), None),
            (
                Measurement {
                    unknown: 0,
                    overflowed: true,
                },
                None,
            ),
        ] {
            snapshot.progress.measurement = measurement;
            snapshot.nodes[0].measurement = measurement;
            let report: serde_json::Value =
                serde_json::from_str(&snapshot_json(&snapshot).unwrap()).unwrap();
            assert_eq!(report["metric"], "file_allocation");
            assert!(report["summary"]["logical_bytes"].is_null());
            assert_eq!(report["summary"]["known_bytes"], 80);
            assert_eq!(report["summary"]["file_allocation_bytes"].as_u64(), value);
            assert_eq!(
                report["summary"]["measurement_complete"],
                measurement.complete()
            );
            let mut view = View::default();
            view.refresh(&snapshot.root, &snapshot.nodes);
            let text = visible_csv(&snapshot, &view).unwrap();
            let mut reader = csv::Reader::from_reader(text.as_bytes());
            let headers = reader.headers().unwrap().clone();
            let row = reader.records().next().unwrap().unwrap();
            let get = |name: &str| &row[headers.iter().position(|h| h == name).unwrap()];
            assert_eq!(get("metric"), "file_allocation");
            assert_eq!(get("logical_bytes"), "");
            assert_eq!(get("known_bytes"), "30");
            assert_eq!(get("unknown_measurements"), measurement.unknown.to_string());
            assert_eq!(
                get("measurement_overflowed"),
                measurement.overflowed.to_string()
            );
            assert_eq!(
                get("file_allocation_bytes"),
                if measurement.complete() { "30" } else { "" }
            );
        }
        snapshot.progress.bytes = 0;
        snapshot.progress.measurement = Measurement::default();
        let r: serde_json::Value =
            serde_json::from_str(&snapshot_json(&snapshot).unwrap()).unwrap();
        assert_eq!(r["summary"]["file_allocation_bytes"], 0);
        snapshot.cancelled = true;
        let r: serde_json::Value =
            serde_json::from_str(&snapshot_json(&snapshot).unwrap()).unwrap();
        assert!(r["summary"]["file_allocation_bytes"].is_null());
        assert_eq!(r["summary"]["measurement_complete"], false);
    }

    #[test]
    fn json_preserves_exact_bytes_full_model_and_partial_scan_flags() {
        let mut snapshot = snapshot();
        snapshot.progress.bytes = u64::MAX;
        snapshot.nodes[0].bytes = u64::MAX;
        snapshot.cancelled = true;
        snapshot.truncated = true;
        snapshot.top_level_truncated = true;
        snapshot.inaccessible_count = 3;
        snapshot.inaccessible_paths.push("/fixture/denied".into());
        snapshot.nodes[1].unreadable = true;
        let text = snapshot_json(&snapshot).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["root"], "/fixture");
        assert_eq!(json["summary"]["logical_bytes"].as_u64(), Some(u64::MAX));
        assert_eq!(
            json["directories"][0]["logical_bytes"].as_u64(),
            Some(u64::MAX)
        );
        assert_eq!(
            json["directories"][1]["path"],
            "/fixture/branch/target,\"line\nname"
        );
        assert_eq!(json["directories"][1]["unreadable"], true);
        assert_eq!(json["directories"].as_array().unwrap().len(), 3);
        for key in [
            "cancelled",
            "tree_truncated",
            "map_truncated",
            "inaccessible_paths_truncated",
        ] {
            assert_eq!(json["status"][key], true);
        }
        assert_eq!(json["status"]["inaccessible_count"], 3);
        assert_eq!(json["status"]["inaccessible_paths"][0], "/fixture/denied");
        assert_eq!(json["directory_rows_are_nested_totals"], true);
    }
    #[test]
    fn csv_exports_collapsed_projection_and_search_reveal_without_touching_json() {
        let snapshot = snapshot();
        let mut view = View::default();
        view.options.collapsed.insert(snapshot.root.join("branch"));
        view.refresh(&snapshot.root, &snapshot.nodes);
        let text = visible_csv(&snapshot, &view).unwrap();
        let rows: Vec<_> = csv::Reader::from_reader(text.as_bytes())
            .records()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(&rows[0][2], "branch");
        assert_eq!(&rows[0][14], "true");
        assert_eq!(&rows[0][4], "30");
        view.options.query = "target".into();
        view.refresh(&snapshot.root, &snapshot.nodes);
        let text = visible_csv(&snapshot, &view).unwrap();
        let rows: Vec<_> = csv::Reader::from_reader(text.as_bytes())
            .records()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| &row[14] == "false"));
        let json: serde_json::Value =
            serde_json::from_str(&snapshot_json(&snapshot).unwrap()).unwrap();
        assert_eq!(json["directories"].as_array().unwrap().len(), 3);
        view.has_children.clear();
        assert!(visible_csv(&snapshot, &view).is_err());
        view.invalidate();
        view.refresh(&snapshot.root, &snapshot.nodes);
        view.options.query.clear();
        assert!(visible_csv(&snapshot, &view).is_err());
    }
    #[test]
    fn csv_follows_visible_order_context_and_lossless_cell_quoting_without_changing_totals() {
        let snapshot = snapshot();
        let mut view = View::default();
        view.options.query = "target".into();
        view.refresh(&snapshot.root, &snapshot.nodes);
        let text = visible_csv(&snapshot, &view).unwrap();
        let mut reader = csv::Reader::from_reader(text.as_bytes());
        assert_eq!(reader.headers().unwrap().len(), 24);
        let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(&rows[0][2], "branch");
        assert_eq!(&rows[0][4], "30");
        assert_eq!(&rows[0][7], "true");
        assert_eq!(&rows[1][1], "/fixture/branch/target,\"line\nname");
        assert_eq!(&rows[1][4], "10");
        assert_eq!(&rows[1][7], "false");
        assert_eq!(&rows[1][8], "target");
        assert_eq!(&rows[0][11], "false");
        assert_eq!(&rows[0][12], "false");
        assert_eq!(&rows[0][13], "0");
        assert!(!text.contains("elsewhere"));
        assert_eq!(snapshot.progress.bytes, 80);
        assert_eq!(snapshot.nodes[0].bytes, 30);
        let mut partial = snapshot.clone();
        partial.cancelled = true;
        partial.truncated = true;
        partial.inaccessible_count = 2;
        let partial_text = visible_csv(&partial, &view).unwrap();
        let partial_row = csv::Reader::from_reader(partial_text.as_bytes())
            .records()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(&partial_row[11], "true");
        assert_eq!(&partial_row[12], "true");
        assert_eq!(&partial_row[13], "2");
        let json: serde_json::Value =
            serde_json::from_str(&snapshot_json(&snapshot).unwrap()).unwrap();
        assert_eq!(json["directories"].as_array().unwrap().len(), 3);
    }
    #[test]
    fn csv_refuses_dirty_stale_and_corrupt_projection_and_invalid_paths() {
        let mut snapshot = snapshot();
        let mut view = View::default();
        view.options.query = "target".into();
        view.refresh(&snapshot.root, &snapshot.nodes);
        view.options.query = "other".into();
        assert!(visible_csv(&snapshot, &view).is_err());
        view.options.query = "target".into();
        view.indices.push(1);
        assert!(visible_csv(&snapshot, &view).is_err());
        view.invalidate();
        view.refresh(&snapshot.root, &snapshot.nodes);
        snapshot.nodes[1].path = "/fixture/branch/renamed".into();
        assert!(visible_csv(&snapshot, &view).is_err());
        for path in [
            "/outside/node",
            "/fixture/../outside",
            "/fixture/invalid\0name",
        ] {
            snapshot.nodes[0].path = path.into();
            assert!(snapshot_json(&snapshot).is_err());
        }
        snapshot.root = "relative".into();
        assert!(snapshot_json(&snapshot).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn reports_refuse_non_utf8_paths_instead_of_lossy_names() {
        use std::os::unix::ffi::OsStringExt;
        let mut snapshot = snapshot();
        snapshot.nodes[0].path = snapshot.root.join(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(snapshot_json(&snapshot).is_err());
        let mut view = View::default();
        view.refresh(&snapshot.root, &snapshot.nodes);
        assert!(visible_csv(&snapshot, &view).is_err());
    }
    #[test]
    fn both_serializers_refuse_oversized_reports_instead_of_partial_output() {
        let mut snapshot = snapshot();
        let template = snapshot.nodes[0].clone();
        snapshot.nodes = (0..6000)
            .map(|index| {
                let mut node = template.clone();
                node.path = snapshot.root.join(format!("{index}-{}", "x".repeat(200)));
                node.label = "x".repeat(200);
                node
            })
            .collect();
        let mut view = View::default();
        view.refresh(&snapshot.root, &snapshot.nodes);
        assert!(snapshot_json(&snapshot).is_err());
        assert!(visible_csv(&snapshot, &view).is_err());
    }
}
