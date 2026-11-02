//! Bounded read-only JSON of the retained duplicate scan and current proposal.
use crate::{
    app::DupState,
    dedup::{DupGroup, GroupSort},
};
use serde::{
    Serialize, Serializer,
    ser::{Error, SerializeSeq},
};
use std::{
    io::{self, Write},
    path::Path,
};
const LIMIT: usize = 1024 * 1024;
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > LIMIT.saturating_sub(self.0.len()) {
            return Err(io::Error::other(
                "Duplicate JSON reports are limited to 1 MiB",
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
        .ok_or("JSON report needs lossless UTF-8 paths")?;
    if !path.is_absolute() || text.contains('\0') {
        return Err("JSON report needs absolute paths without NUL".into());
    }
    Ok(text)
}
#[derive(Serialize)]
struct Context<'a> {
    root: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_root: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    only_cross_root_groups: Option<bool>,
    scope: &'static str,
    metadata_source: &'static str,
    max_subfolder_levels: Option<u32>,
    configured_subfolder_levels: Option<u32>,
    min_bytes: u64,
    max_bytes: Option<u64>,
    names: &'a crate::duplicate_name_filter::Rules,
    minimum_copies: usize,
    keep_policy: &'static str,
    group_sort: &'static str,
}
#[derive(Serialize)]
struct Summary {
    groups: usize,
    files: usize,
    proposed_trash_files: usize,
    proposed_trash_bytes: u64,
    scan_issues: usize,
    partial: bool,
}
#[derive(Serialize)]
struct Document<'a> {
    format: &'static str,
    version: u32,
    snapshot: &'static str,
    context: Context<'a>,
    summary: Summary,
    groups: Groups<'a>,
}
struct Groups<'a> {
    state: &'a DupState,
    order: &'a [usize],
}
#[derive(Serialize)]
struct Group<'a> {
    group: usize,
    original_group: usize,
    size_bytes: u64,
    files: Files<'a>,
}
struct Files<'a> {
    group: &'a DupGroup,
    keeper: Option<usize>,
}
#[derive(Serialize)]
struct File<'a> {
    member: usize,
    path: &'a str,
    action: &'static str,
}
impl Serialize for Groups<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.order.len()))?;
        for (position, &index) in self.order.iter().enumerate() {
            let group = &self.state.groups[index];
            seq.serialize_element(&Group {
                group: position + 1,
                original_group: index + 1,
                size_bytes: group.size,
                files: Files {
                    group,
                    keeper: self.state.keep.get(index).copied().flatten(),
                },
            })?;
        }
        seq.end()
    }
}
impl Serialize for Files<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.group.files.len()))?;
        for (index, file) in self.group.files.iter().enumerate() {
            seq.serialize_element(&File {
                member: index + 1,
                path: path(&file.path).map_err(S::Error::custom)?,
                action: crate::dedup::plan_action(self.group, self.keeper, index),
            })?;
        }
        seq.end()
    }
}
pub fn json(state: &DupState) -> Result<String, String> {
    if state.scan.is_some() {
        return Err("Wait for the duplicate scan to finish before copying a report".into());
    }
    if state.groups.iter().all(|group| group.files.is_empty()) {
        return Err("No duplicate records to copy".into());
    }
    let order = if state.sort == GroupSort::Scan {
        std::borrow::Cow::Owned((0..state.groups.len()).collect::<Vec<_>>())
    } else {
        std::borrow::Cow::Borrowed(state.group_order.as_slice())
    };
    crate::dedup::validate_group_order(state.groups.len(), &order)?;
    let document = Document {
        format: "commander_duplicate_report",
        version: 1,
        snapshot: "retained_duplicate_scan_proposal",
        context: Context {
            root: path(&state.root)?,
            additional_root: if state.include_other {
                Some(path(&state.other_root)?)
            } else {
                None
            },
            only_cross_root_groups: state.include_other.then_some(state.cross_only),
            scope: if state.include_other && state.recursive {
                "two_trees"
            } else if state.include_other {
                "two_folders"
            } else if state.recursive {
                "recursive"
            } else {
                "listing"
            },
            metadata_source: if state.recursive {
                "fresh_tree_enumeration"
            } else if state.fresh_enumeration {
                "fresh_root_enumeration"
            } else {
                "retained_pane_listing"
            },
            max_subfolder_levels: state.recursive.then_some(state.max_depth).flatten(),
            configured_subfolder_levels: state.max_depth,
            min_bytes: state.sizes.min,
            max_bytes: state.sizes.max,
            names: &state.names.rules,
            minimum_copies: state.min_copies.get(),
            keep_policy: state.policy.report_key(),
            group_sort: match state.sort {
                GroupSort::Scan => "scan",
                GroupSort::Savings => "savings",
                GroupSort::Size => "size",
                GroupSort::Copies => "copies",
            },
        },
        summary: Summary {
            groups: state.groups.len(),
            files: state.groups.iter().map(|g| g.files.len()).sum(),
            proposed_trash_files: crate::dedup::delete_count(&state.groups, &state.keep),
            proposed_trash_bytes: crate::dedup::bytes_reclaimed(&state.groups, &state.keep),
            scan_issues: state.scan_issues,
            partial: state.scan_issues > 0,
        },
        groups: Groups {
            state,
            order: &order,
        },
    };
    let mut output = Bounded(Vec::new());
    serde_json::to_writer_pretty(&mut output, &document).map_err(|e| e.to_string())?;
    String::from_utf8(output.0).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dedup::{FileKey, KeepPolicy};
    fn state() -> DupState {
        let group = |size, names: &[&str]| DupGroup {
            size,
            files: names
                .iter()
                .map(|name| FileKey {
                    path: std::path::PathBuf::from("/report-root").join(name),
                    identity: crate::panel::ListingIdentity::Unavailable,
                    size,
                    hash: 1,
                    modified: None,
                })
                .collect(),
        };
        DupState {
            history: Default::default(),
            other_root: "/other-root".into(),
            include_other: false,
            cross_only: false,
            fresh_enumeration: true,
            min_copies: Default::default(),
            copies_draft: "1".into(),
            name_validation: Default::default(),
            name_draft: crate::duplicate_name_filter::Rules {
                include: "[".into(),
                ..Default::default()
            },
            names: crate::duplicate_name_filter::Rules {
                include: "*.bin".into(),
                exclude: "skip*".into(),
                case: true,
                grouping: crate::duplicate_name_group::Component::ParentName,
                grouping_ignore_ascii_case: true,
                extension: crate::duplicate_extension::Mode::IgnoreAsciiCase,
            }
            .compile()
            .unwrap(),
            groups: vec![
                group(10, &["a,\n\"ß.bin", "b.bin", "c.bin"]),
                group(64, &["d.bin", "e.bin"]),
            ],
            keep: vec![Some(1), None],
            policy: KeepPolicy::Newest,
            sort: GroupSort::Savings,
            group_order: vec![1, 0],
            root: "/report-root".into(),
            recursive: true,
            max_depth: Some(1),
            depth_draft: "bad".into(),
            sizes: crate::dedup_scan::SizeRange {
                min: 1,
                max: Some(128),
            },
            min_size: "bad".into(),
            max_size: "bad".into(),
            scan_issues: 2,
            scan: None,
        }
    }
    #[test]
    fn reports_bind_each_ranking_policy_to_valid_keeper_actions() {
        let mut state = state();
        for (policy, key) in [
            (KeepPolicy::FirstName, "first_name"),
            (KeepPolicy::LastName, "last_name"),
            (KeepPolicy::ShortestName, "shortest_name"),
            (KeepPolicy::LongestName, "longest_name"),
            (KeepPolicy::ShallowestPath, "shallowest_path"),
            (KeepPolicy::DeepestPath, "deepest_path"),
        ] {
            state.policy = policy;
            state.keep = crate::dedup::keep_plan(&state.groups, policy);
            let value: serde_json::Value = serde_json::from_str(&json(&state).unwrap()).unwrap();
            assert_eq!(value["context"]["keep_policy"], key);
            assert_eq!(value["summary"]["proposed_trash_files"], 3);
            for group in value["groups"].as_array().unwrap() {
                assert_eq!(
                    group["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|f| f["action"] == "keep")
                        .count(),
                    1
                );
            }
            state.keep.fill(None);
            let value: serde_json::Value = serde_json::from_str(&json(&state).unwrap()).unwrap();
            assert_eq!(value["context"]["keep_policy"], key);
            assert_eq!(value["summary"]["proposed_trash_files"], 0);
        }
    }

    #[test]
    fn json_round_trips_paths_actions_order_and_applied_scope_without_io_or_drafts() {
        let mut state = state();
        let before = json(&state).unwrap();
        let value: serde_json::Value = serde_json::from_str(&before).unwrap();
        assert_eq!(value["format"], "commander_duplicate_report");
        assert_eq!(value["version"], 1);
        assert_eq!(value["context"]["minimum_copies"], 2);
        assert_eq!(value["context"]["max_subfolder_levels"], 1);
        assert_eq!(value["context"]["names"]["include"], "*.bin");
        assert_eq!(value["context"]["names"]["extension"], "IgnoreAsciiCase");
        assert_eq!(value["context"]["names"]["grouping"], "ParentName");
        assert_eq!(
            value["context"]["names"]["grouping_ignore_ascii_case"],
            true
        );
        assert_eq!(value["context"]["min_bytes"], 1);
        assert_eq!(value["context"]["max_bytes"], 128);
        assert_eq!(value["summary"]["scan_issues"], 2);
        assert_eq!(value["summary"]["partial"], true);
        assert_eq!(value["summary"]["proposed_trash_files"], 2);
        assert_eq!(value["summary"]["proposed_trash_bytes"], 20);
        assert_eq!(value["groups"][0]["original_group"], 2);
        assert_eq!(value["groups"][1]["original_group"], 1);
        let json_rows: Vec<Vec<String>> = value["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| {
                group["files"].as_array().unwrap().iter().map(|file| {
                    vec![
                        group["group"].to_string(),
                        file["action"].as_str().unwrap().into(),
                        file["path"].as_str().unwrap().into(),
                        group["size_bytes"].to_string(),
                    ]
                })
            })
            .collect();
        let csv = crate::dedup::plan_csv_in_order(&state.groups, &state.keep, &state.group_order)
            .unwrap();
        let csv_rows: Vec<Vec<String>> = csv::Reader::from_reader(csv.as_bytes())
            .records()
            .map(|r| r.unwrap().iter().map(str::to_owned).collect())
            .collect();
        assert_eq!(json_rows, csv_rows);
        state.name_draft.include = "*.txt".into();
        state.depth_draft = "999".into();
        state.copies_draft = "5".into();
        state.min_size = "500".into();
        assert_eq!(json(&state).unwrap(), before);
        state.include_other = true;
        state.cross_only = true;
        let multi: serde_json::Value = serde_json::from_str(&json(&state).unwrap()).unwrap();
        assert_eq!(multi["context"]["additional_root"], "/other-root");
        assert_eq!(multi["context"]["scope"], "two_trees");
        assert_eq!(multi["context"]["only_cross_root_groups"], true);
        state.other_root = "relative".into();
        assert!(json(&state).is_err());
        state.include_other = false;
        state.recursive = false;
        let value: serde_json::Value = serde_json::from_str(&json(&state).unwrap()).unwrap();
        assert_eq!(value["context"]["scope"], "listing");
        assert!(value["context"].get("only_cross_root_groups").is_none());
        assert!(value["context"]["max_subfolder_levels"].is_null());
        assert_eq!(value["context"]["configured_subfolder_levels"], 1);
        state.keep = vec![Some(999)];
        let value: serde_json::Value = serde_json::from_str(&json(&state).unwrap()).unwrap();
        assert_eq!(value["summary"]["proposed_trash_files"], 0);
        assert!(value["groups"].as_array().unwrap().iter().all(|g| {
            g["files"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["action"] == "skip")
        }));
    }
    #[test]
    fn reports_refuse_bad_order_paths_empty_busy_and_oversized_output_atomically() {
        let mut s = state();
        s.group_order = vec![0, 0];
        assert!(json(&s).is_err());
        s.group_order = vec![1, 0];
        for path in ["relative", "/bad\0path"] {
            s.groups[0].files[0].path = path.into();
            assert!(json(&s).is_err());
        }
        s.groups[0].files[0].path = format!("/{}", "é".repeat(LIMIT)).into();
        assert!(json(&s).unwrap_err().contains("1 MiB"));
        let mut s = state();
        s.root = "relative".into();
        assert!(json(&s).is_err());
        let mut s = state();
        s.scan = Some(crate::dedup_scan::spawn_filtered(
            vec![],
            Default::default(),
            Default::default(),
            Default::default(),
            || {},
        ));
        assert!(json(&s).unwrap_err().contains("finish"));
        let mut s = state();
        s.groups.clear();
        assert!(json(&s).unwrap_err().contains("No duplicate"));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut s = state();
            s.groups[0].files[0].path = std::ffi::OsString::from_vec(b"/bad\xff".to_vec()).into();
            assert!(json(&s).is_err());
        }
    }
}
