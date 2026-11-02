//! Bounded read-only reports of a retained sync plan. Reports are not replay plans.
use crate::{
    app::SyncState,
    sync::{SyncAction, SyncDirection, SyncPolicy, SyncStatus},
};
use serde::{
    Serialize,
    ser::{SerializeSeq, Serializer},
};
use std::{
    io::{self, Write},
    path::Path,
};
const LIMIT: usize = 1024 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Full,
    Shown,
}
impl Scope {
    pub const ALL: [Self; 2] = [Self::Full, Self::Shown];
    pub const fn key(self) -> &'static str {
        match self {
            Self::Full => "full_plan",
            Self::Shown => "shown_rows",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "Full plan",
            Self::Shown => "Shown rows",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
}
impl Format {
    pub const ALL: [Self; 2] = [Self::Csv, Self::Json];
    #[cfg(any(test, feature = "visual-qa"))]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Csv => "Copy plan CSV",
            Self::Json => "Copy plan JSON",
        }
    }
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > LIMIT.saturating_sub(self.0.len()) {
            return Err(io::Error::other("Sync plan reports are limited to 1 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn ready(state: &SyncState) -> bool {
    state.scan.is_none()
        && state.error.is_none()
        && state
            .stamp
            .as_ref()
            .is_some_and(|s| s.compare_contents == state.compare_contents)
}
fn path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "Report paths must be lossless UTF-8".into())
}
fn direction(value: SyncDirection) -> &'static str {
    match value {
        SyncDirection::ToLeft => "to_left",
        SyncDirection::ToRight => "to_right",
        SyncDirection::Skip => "skip",
    }
}
fn status(value: SyncStatus) -> &'static str {
    match value {
        SyncStatus::LeftOnly => "left_only",
        SyncStatus::RightOnly => "right_only",
        SyncStatus::LeftNewer => "left_newer",
        SyncStatus::RightNewer => "right_newer",
        SyncStatus::Differing => "differing",
        SyncStatus::Identical => "matching_metadata",
        SyncStatus::DirectoryPair => "uncompared_folder_pair",
        SyncStatus::TypeConflict => "type_conflict",
        SyncStatus::CaseConflict => "case_conflict",
    }
}
fn policy(value: SyncPolicy) -> &'static str {
    match value {
        SyncPolicy::TwoWay => "two_way",
        SyncPolicy::MirrorLeftToRight => "copy_left_to_right",
        SyncPolicy::MirrorRightToLeft => "copy_right_to_left",
    }
}
fn indices(state: &SyncState, scope: Scope) -> impl Iterator<Item = usize> + '_ {
    (0..if scope == Scope::Full {
        state.actions.len()
    } else {
        state.review.indices.len()
    })
        .map(move |i| {
            if scope == Scope::Full {
                i
            } else {
                state.review.indices[i]
            }
        })
}
#[derive(Serialize)]
struct Row<'a> {
    original_row: usize,
    name: &'a str,
    status: &'static str,
    direction: &'static str,
    left_path: Option<&'a str>,
    right_path: Option<&'a str>,
    source_path: Option<&'a str>,
    target_folder: Option<&'a str>,
    comparison_display: Option<String>,
    content_comparison: &'static str,
    content_error: Option<&'a str>,
}
fn row(state: &SyncState, index: usize) -> Result<Row<'_>, String> {
    let action: &SyncAction = &state.actions[index];
    let [left, right] = action.side_paths();
    Ok(Row {
        original_row: index + 1,
        name: &action.name,
        status: status(action.status),
        direction: direction(action.direction),
        left_path: left.map(path).transpose()?,
        right_path: right.map(path).transpose()?,
        source_path: action.source_path().map(path).transpose()?,
        target_folder: match action.direction {
            SyncDirection::ToLeft => Some(path(&state.left_dir)?),
            SyncDirection::ToRight => Some(path(&state.right_dir)?),
            SyncDirection::Skip => None,
        },
        comparison_display: action.comparison_text(),
        content_comparison: action.content.key(),
        content_error: action.content.error(),
    })
}
struct Rows<'a> {
    state: &'a SyncState,
    scope: Scope,
}
impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        for index in indices(self.state, self.scope) {
            seq.serialize_element(&row(self.state, index).map_err(serde::ser::Error::custom)?)?;
        }
        seq.end()
    }
}
#[derive(Serialize)]
struct Report<'a> {
    format: &'static str,
    version: u8,
    comparison: &'static str,
    scope: &'static str,
    left_folder: &'a str,
    right_folder: &'a str,
    policy: &'static str,
    view_query: &'a str,
    view_match_mode: &'static str,
    view_category: &'static str,
    view_sort: &'static str,
    view_descending: bool,
    full_rows: usize,
    exported_rows: usize,
    full_planned: usize,
    exported_planned: usize,
    rows: Rows<'a>,
}
pub fn export(state: &SyncState, scope: Scope, format: Format) -> Result<String, String> {
    if !ready(state) {
        return Err("Wait for a completed sync plan before copying".into());
    }
    let stamp = state.stamp.as_ref().expect("ready checked");
    if stamp.left_root != state.left_dir || stamp.right_root != state.right_dir {
        return Err("Refresh the synchronization plan before copying".into());
    }
    if scope == Scope::Shown {
        let mut seen = std::collections::HashSet::new();
        if state
            .review
            .indices
            .iter()
            .any(|&index| index >= state.actions.len() || !seen.insert(index))
        {
            return Err("Refresh the plan view before copying".into());
        }
    }
    let left = path(&state.left_dir)?;
    let right = path(&state.right_dir)?;
    let full_planned = state
        .actions
        .iter()
        .filter(|row| row.direction != SyncDirection::Skip)
        .count();
    let exported_planned = indices(state, scope)
        .filter(|&i| state.actions[i].direction != SyncDirection::Skip)
        .count();
    let exported_rows = indices(state, scope).count();
    let report = Report {
        format: "commander.sync-plan-report",
        version: 1,
        comparison: if state.compare_contents {
            "shallow_name_size_mtime_and_bytes"
        } else {
            "shallow_name_size_mtime"
        },
        scope: scope.key(),
        left_folder: left,
        right_folder: right,
        policy: policy(state.policy),
        view_query: &state.review.query,
        view_match_mode: if state.review.masks {
            "whole_name_masks"
        } else {
            "contains_all_words"
        },
        view_category: state.review.scope.label(),
        view_sort: state.review.sort.key(),
        view_descending: state.review.descending,
        full_rows: state.actions.len(),
        exported_rows,
        full_planned,
        exported_planned,
        rows: Rows { state, scope },
    };
    let writer = Bounded(Vec::new());
    let bytes = match format {
        Format::Json => {
            let mut writer = writer;
            serde_json::to_writer_pretty(&mut writer, &report).map_err(|e| e.to_string())?;
            writer.0
        }
        Format::Csv => {
            let mut csv = csv::Writer::from_writer(writer);
            csv.write_record([
                "Scope",
                "Full rows",
                "Exported rows",
                "Full planned",
                "Exported planned",
                "Left folder",
                "Right folder",
                "Policy",
                "View query",
                "View category",
                "Original row",
                "Name",
                "Status",
                "Direction",
                "Left path",
                "Right path",
                "Source path",
                "Target folder",
                "Comparison (display)",
                "View sort",
                "View descending",
                "Content comparison",
                "Content error",
                "Comparison method",
                "View match mode",
            ])
            .map_err(|e| e.to_string())?;
            for index in indices(state, scope) {
                let row = row(state, index)?;
                csv.write_record([
                    scope.key().to_string(),
                    state.actions.len().to_string(),
                    exported_rows.to_string(),
                    full_planned.to_string(),
                    exported_planned.to_string(),
                    left.into(),
                    right.into(),
                    report.policy.into(),
                    state.review.query.clone(),
                    report.view_category.into(),
                    row.original_row.to_string(),
                    row.name.into(),
                    row.status.into(),
                    row.direction.into(),
                    row.left_path.unwrap_or("").into(),
                    row.right_path.unwrap_or("").into(),
                    row.source_path.unwrap_or("").into(),
                    row.target_folder.unwrap_or("").into(),
                    row.comparison_display.unwrap_or_default(),
                    report.view_sort.into(),
                    report.view_descending.to_string(),
                    row.content_comparison.into(),
                    row.content_error.unwrap_or("").into(),
                    report.comparison.into(),
                    report.view_match_mode.into(),
                ])
                .map_err(|e| e.to_string())?;
            }
            csv.flush().map_err(|e| e.to_string())?;
            csv.into_inner().map_err(|e| e.to_string())?.0
        }
    };
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> SyncState {
        SyncState {
            compare_contents: false,
            export_scope: Scope::Full,
            review: Default::default(),
            policy: SyncPolicy::TwoWay,
            durability: crate::operation::DurabilityProfile::Fast,
            version_retention: crate::operation::VersionRetentionPolicy::ALL[0],
            actions: vec![],
            left_dir: "/left".into(),
            right_dir: "/right".into(),
            left_show_hidden: false,
            right_show_hidden: false,
            guard: Default::default(),
            stamp: Some(crate::sync_guard::PlanStamp {
                compare_contents: false,
                left_root: "/left".into(),
                right_root: "/right".into(),
                left_show_hidden: false,
                right_show_hidden: false,
                source_fingerprint: 0,
                left_volume: None,
                right_volume: None,
            }),
            settings_fingerprint: 0,
            allow_large_plan: false,
            marker_enabled: false,
            marker_input: String::new(),
            error: None,
            dry_run: false,
            scan: None,
            kept_edits: 0,
        }
    }
    #[test]
    fn reports_distinguish_words_from_masks_without_changing_plan_or_csv_positions() {
        let mut state = state();
        state.actions = vec![
            crate::sync::test_action("notes.md", SyncStatus::LeftOnly, SyncDirection::ToRight),
            crate::sync::test_action("readme.md", SyncStatus::RightOnly, SyncDirection::ToLeft),
        ];
        state.review.query = "*.md;!read*".into();
        let original = state.actions.clone();
        for (masks, mode, count) in [
            (false, "contains_all_words", 0),
            (true, "whole_name_masks", 1),
        ] {
            state.review.masks = masks;
            state.review.refresh(&state.actions);
            for scope in [Scope::Full, Scope::Shown] {
                let report: serde_json::Value =
                    serde_json::from_str(&export(&state, scope, Format::Json).unwrap()).unwrap();
                assert_eq!(report["view_match_mode"], mode);
                assert_eq!(report["view_query"], "*.md;!read*");
                assert_eq!(
                    report["exported_rows"],
                    if scope == Scope::Full { 2 } else { count }
                );
                let csv = export(&state, scope, Format::Csv).unwrap();
                let mut reader = csv::Reader::from_reader(csv.as_bytes());
                let headers = reader.headers().unwrap().clone();
                assert_eq!(&headers[8], "View query");
                assert_eq!(&headers[24], "View match mode");
                for record in reader.records() {
                    let record = record.unwrap();
                    assert_eq!(&record[8], "*.md;!read*");
                    assert_eq!(&record[24], mode);
                }
            }
        }
        assert_eq!(state.actions, original);
    }
    #[test]
    fn content_reports_expose_applied_mode_results_errors_and_fail_closed_during_mode_changes() {
        let mut state = state();
        state.actions = vec![
            crate::sync::test_action("equal", SyncStatus::LeftNewer, SyncDirection::ToRight),
            crate::sync::test_action("unreadable", SyncStatus::Identical, SyncDirection::Skip),
        ];
        state.compare_contents = true;
        assert!(!ready(&state));
        state.stamp.as_mut().unwrap().compare_contents = true;
        state.actions[0].set_content(crate::sync_content::Comparison::Equal, state.policy);
        state.actions[1].set_content(
            crate::sync_content::Comparison::Unavailable("denied, \"target\"".into()),
            state.policy,
        );
        let value: serde_json::Value =
            serde_json::from_str(&export(&state, Scope::Full, Format::Json).unwrap()).unwrap();
        assert_eq!(value["comparison"], "shallow_name_size_mtime_and_bytes");
        assert_eq!(value["rows"][0]["content_comparison"], "equal");
        assert_eq!(value["rows"][0]["direction"], "skip");
        assert_eq!(value["rows"][1]["content_error"], "denied, \"target\"");
        let csv = export(&state, Scope::Full, Format::Csv).unwrap();
        let mut reader = csv::Reader::from_reader(csv.as_bytes());
        assert_eq!(&reader.headers().unwrap()[21], "Content comparison");
        let rows = reader.records().map(Result::unwrap).collect::<Vec<_>>();
        assert_eq!(&rows[0][21], "equal");
        assert_eq!(&rows[1][22], "denied, \"target\"");
    }

    #[test]
    fn shown_reports_follow_sorted_projection_and_full_reports_keep_original_order() {
        let mut state = state();
        state.actions = vec![
            crate::sync::test_action("b2", SyncStatus::LeftOnly, SyncDirection::ToRight),
            crate::sync::test_action("a10", SyncStatus::LeftOnly, SyncDirection::ToRight),
            crate::sync::test_action("a2", SyncStatus::LeftOnly, SyncDirection::ToRight),
            crate::sync::test_action("A2", SyncStatus::LeftOnly, SyncDirection::ToRight),
        ];
        state.review.sort = crate::sync_review::Sort::Name;
        state.review.descending = true;
        state.review.refresh(&state.actions);
        let shown: serde_json::Value =
            serde_json::from_str(&export(&state, Scope::Shown, Format::Json).unwrap()).unwrap();
        let full: serde_json::Value =
            serde_json::from_str(&export(&state, Scope::Full, Format::Json).unwrap()).unwrap();
        assert_eq!(shown["view_sort"], "name");
        assert_eq!(shown["view_descending"], true);
        assert_eq!(
            shown["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["original_row"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 4, 3]
        );
        assert_eq!(
            full["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["original_row"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        let csv = export(&state, Scope::Shown, Format::Csv).unwrap();
        let mut reader = csv::Reader::from_reader(csv.as_bytes());
        assert_eq!(&reader.headers().unwrap()[19], "View sort");
        let rows = reader.records().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(&rows[2][10], "4");
        assert_eq!(&rows[2][19], "name");
        assert_eq!(&rows[2][20], "true");
    }

    #[test]
    fn reports_preserve_full_and_shown_scope_original_rows_directions_paths_and_csv_controls() {
        let mut state = state();
        let name = "=notes,\"café\"\r\n.txt";
        state.actions = vec![
            crate::sync::test_action(name, SyncStatus::Differing, SyncDirection::ToLeft),
            crate::sync::test_action("notes skipped", SyncStatus::LeftOnly, SyncDirection::Skip),
            crate::sync::test_action("hidden", SyncStatus::RightOnly, SyncDirection::ToLeft),
            crate::sync::test_action("folder", SyncStatus::DirectoryPair, SyncDirection::Skip),
        ];
        state.review.query = "NOTES".into();
        state.review.refresh(&state.actions);
        let original = state.actions.clone();
        let full: serde_json::Value =
            serde_json::from_str(&export(&state, Scope::Full, Format::Json).unwrap()).unwrap();
        let shown: serde_json::Value =
            serde_json::from_str(&export(&state, Scope::Shown, Format::Json).unwrap()).unwrap();
        assert_eq!(full["exported_rows"], 4);
        assert_eq!(full["full_planned"], 2);
        assert_eq!(shown["exported_rows"], 2);
        assert_eq!(shown["exported_planned"], 1);
        assert_eq!(shown["scope"], "shown_rows");
        assert_eq!(shown["rows"][0]["name"], name);
        assert_eq!(shown["rows"][0]["source_path"], format!("/right/{name}"));
        assert_eq!(shown["rows"][0]["target_folder"], "/left");
        assert_eq!(shown["rows"][1]["source_path"], serde_json::Value::Null);
        assert_eq!(full["rows"][3]["status"], "uncompared_folder_pair");
        let csv = export(&state, Scope::Full, Format::Csv).unwrap();
        let mut reader = csv::Reader::from_reader(csv.as_bytes());
        let records = reader.records().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(&records[0][11], name);
        assert_eq!(&records[0][10], "1");
        assert_eq!(&records[0][13], "to_left");
        assert_eq!(state.actions, original);
        state.review.query = "absent".into();
        state.review.refresh(&state.actions);
        let empty: serde_json::Value =
            serde_json::from_str(&export(&state, Scope::Shown, Format::Json).unwrap()).unwrap();
        assert_eq!(empty["exported_rows"], 0);
        assert_eq!(empty["full_rows"], 4);
        assert_eq!(
            csv::Reader::from_reader(
                export(&state, Scope::Shown, Format::Csv)
                    .unwrap()
                    .as_bytes()
            )
            .records()
            .count(),
            0
        );
    }
    #[test]
    fn unavailable_mismatched_invalid_projection_and_oversized_reports_fail_without_a_prefix() {
        let mut state = state();
        state.actions = vec![crate::sync::test_action(
            "name",
            SyncStatus::LeftOnly,
            SyncDirection::ToRight,
        )];
        state.review.refresh(&state.actions);
        state.error = Some("scan failed".into());
        assert!(export(&state, Scope::Full, Format::Json).is_err());
        state.error = None;
        let stamp = state.stamp.take();
        assert!(export(&state, Scope::Full, Format::Csv).is_err());
        state.stamp = stamp;
        state.left_dir = "/changed".into();
        assert!(export(&state, Scope::Full, Format::Json).is_err());
        state.left_dir = "/left".into();
        state.review.indices = vec![0, 0];
        assert!(export(&state, Scope::Shown, Format::Json).is_err());
        state.review.indices = vec![1];
        assert!(export(&state, Scope::Shown, Format::Csv).is_err());
        state.actions[0].name = "x".repeat(LIMIT + 1);
        for format in Format::ALL {
            assert!(export(&state, Scope::Full, format).is_err());
        }
        state.actions[0].name = "\0".repeat(LIMIT / 5);
        assert!(export(&state, Scope::Full, Format::Json).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_retained_source_paths_are_refused_instead_of_exporting_lossy_identities() {
        use std::os::unix::ffi::OsStringExt;
        let temp = crate::testutil::TempDir::new();
        let valid = temp.file("valid", "data");
        let mut entry =
            crate::panel::FileEntry::from_meta(valid.clone(), &std::fs::metadata(valid).unwrap())
                .unwrap();
        // APFS refuses these names at creation; model the native PathBuf retained
        // by a provider without requiring the host filesystem to support it.
        entry.path = temp
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'n', 0xff]));
        let mut state = state();
        state.actions = crate::sync::sync_diff(&[entry], &[], state.policy);
        state.left_dir = temp.path().into();
        state.stamp.as_mut().unwrap().left_root = state.left_dir.clone();
        for format in Format::ALL {
            assert!(export(&state, Scope::Full, format).is_err());
        }
    }
}
