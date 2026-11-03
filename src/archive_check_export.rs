//! Bounded clipboard reports for completed archive-check attempts; no filesystem reads.
use crate::archive_check::Report;
use std::{
    io::{self, Write},
    path::Path,
};
const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Copy)]
pub enum Format {
    Text,
    Json,
    Csv,
}
impl Format {
    pub const ALL: [Self; 3] = [Self::Text, Self::Json, Self::Csv];
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Copy check report",
            Self::Json => "Copy check report JSON",
            Self::Csv => "Copy check report CSV",
        }
    }
}
struct Output(Vec<u8>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_BYTES {
            return Err(io::Error::other(
                "Archive check report exceeds 4 MiB; nothing copied.",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn csv_json_cell(value: &str) -> Result<String, String> {
    let mut output = Output(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(|e| e.to_string())?;
    String::from_utf8(output.0).map_err(|e| e.to_string())
}
#[derive(serde::Serialize)]
struct JsonReport<'a> {
    schema: &'static str,
    archive: &'a str,
    archive_format: Option<&'static str>,
    recorded_at: Option<&'a str>,
    status: &'static str,
    strict_headers: Option<bool>,
    members: Option<usize>,
    decoded_bytes: Option<u64>,
    error: Option<&'a str>,
    content_comparison: Option<&'static str>,
    content_groups: Option<Vec<&'a crate::archive_check::ContentGroup>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_group_indices: Option<&'a [usize]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_group_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_group_totals: Option<crate::archive_check::ContentGroupTotals>,
}

/// Error-mode metadata is captured at operation start; a success uses its actual
/// report mode. Quoted text fields keep control characters from injecting lines.
pub fn format(
    path: &Path,
    result: &Result<Report, String>,
    requested_strict: Option<bool>,
    recorded_at: Option<&chrono::DateTime<chrono::Utc>>,
    format: Format,
) -> Result<String, String> {
    format_scoped(
        path,
        result,
        requested_strict,
        recorded_at,
        format,
        Scope::All,
    )
}

/// Export exactly one retained group, independent of view pages and row selection.
pub fn format_group(
    path: &Path,
    result: &Result<Report, String>,
    recorded_at: Option<&chrono::DateTime<chrono::Utc>>,
    group: usize,
    format: Format,
) -> Result<String, String> {
    if result
        .as_ref()
        .ok()
        .and_then(|r| r.content_groups.as_ref())
        .and_then(|g| g.get(group))
        .is_none()
    {
        return Err("Archive content group is unavailable; nothing copied.".into());
    }
    format_scoped(path, result, None, recorded_at, format, Scope::One(group))
}

/// Export complete groups in the current filtered display order, with original ranks.
/// Empty scope is explicit; invalid or repeated ranks refuse the whole report.
pub fn format_shown_groups(
    path: &Path,
    result: &Result<Report, String>,
    recorded_at: Option<&chrono::DateTime<chrono::Utc>>,
    ranks: &[usize],
    format: Format,
) -> Result<String, String> {
    format_scoped(path, result, None, recorded_at, format, Scope::Shown(ranks))
}
#[derive(Clone, Copy)]
enum Scope<'a> {
    All,
    One(usize),
    Shown(&'a [usize]),
}

fn format_scoped(
    path: &Path,
    result: &Result<Report, String>,
    requested_strict: Option<bool>,
    recorded_at: Option<&chrono::DateTime<chrono::Utc>>,
    format: Format,
    scope: Scope<'_>,
) -> Result<String, String> {
    let path_text = path
        .to_str()
        .ok_or("Archive check report requires a UTF-8 path; no lossy name was copied.")?;
    let error = result.as_ref().err().map(String::as_str);
    if path_text.len() > MAX_BYTES || error.is_some_and(|e| e.len() > MAX_BYTES) {
        return Err("Archive check report field exceeds 4 MiB; nothing copied.".into());
    }
    let (kind, members, decoded, strict) = match result {
        Ok(report) => (
            Some(report.kind.label()),
            Some(report.members),
            Some(report.decoded),
            Some(report.strict_headers),
        ),
        Err(_) => (
            crate::archive::kind_of(path).map(|k| k.label()),
            None,
            None,
            requested_strict,
        ),
    };
    let status = if result.is_ok() {
        "complete"
    } else {
        "incomplete"
    };
    let at = recorded_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let group_index = if let Scope::One(rank) = scope {
        Some(rank)
    } else {
        None
    };
    let group_indices = if let Scope::Shown(ranks) = scope {
        Some(ranks)
    } else {
        None
    };
    let source = result
        .as_ref()
        .ok()
        .and_then(|report| report.content_groups.as_deref());
    let groups = match (source, scope) {
        (Some(groups), Scope::All) => Some(groups.iter().enumerate().collect::<Vec<_>>()),
        (Some(groups), Scope::One(rank)) => Some(vec![(
            rank,
            groups
                .get(rank)
                .ok_or("Archive content group is unavailable; nothing copied.")?,
        )]),
        (Some(groups), Scope::Shown(ranks)) => {
            let mut seen = std::collections::BTreeSet::new();
            let mut selected = Vec::new();
            for &rank in ranks {
                if !seen.insert(rank) {
                    return Err("Repeated archive content group; nothing copied.".into());
                }
                selected.push((
                    rank,
                    groups
                        .get(rank)
                        .ok_or("Archive content group is unavailable; nothing copied.")?,
                ));
            }
            Some(selected)
        }
        (None, Scope::All) => None,
        (None, _) => {
            return Err("No completed content-group scan is available; nothing copied.".into());
        }
    };
    if let Some(groups) = &groups {
        for (_, group) in groups {
            for member in &group.members {
                if member.path.to_str().is_none() {
                    return Err("Archive check report requires UTF-8 member paths; no lossy name was copied.".into());
                }
            }
        }
    }
    let totals = groups.as_ref().map(|groups| {
        crate::archive_check::ContentGroupTotals::from_groups(
            groups.iter().map(|(_, group)| *group),
        )
    });
    let mut output = Output(Vec::new());
    match format {
        Format::Json => {
            let value = JsonReport {
                schema: "commander.archive-check/v1",
                archive: path_text,
                archive_format: kind,
                recorded_at: at.as_deref(),
                status,
                strict_headers: strict,
                members,
                decoded_bytes: decoded,
                error,
                content_comparison: groups.as_ref().map(|_| "sha256_and_size"),
                content_groups: groups
                    .as_ref()
                    .map(|groups| groups.iter().map(|(_, group)| *group).collect()),
                content_group_indices: group_indices,
                content_group_index: group_index,
                content_group_totals: totals,
            };
            serde_json::to_writer_pretty(&mut output, &value).map_err(|e| e.to_string())?;
            output.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        Format::Csv => {
            let mut csv = csv::WriterBuilder::new()
                .terminator(csv::Terminator::CRLF)
                .from_writer(&mut output);
            csv.write_record([
                "record_type",
                "scope",
                "archive_json",
                "archive_format",
                "recorded_at",
                "status",
                "strict_headers",
                "scan_members",
                "scan_decoded_bytes",
                "group_index",
                "sha256_json",
                "group_size_bytes",
                "group_members",
                "member_index",
                "member_size_bytes",
                "member_path_json",
                "error_json",
                "exported_groups",
                "exported_members",
                "payload_bytes",
                "extra_members",
                "extra_payload_bytes",
                "extra_payload_basis",
            ])
            .map_err(|e| e.to_string())?;
            let scope = match scope {
                Scope::All => "all_scan",
                Scope::One(_) => "single_group",
                Scope::Shown(_) => "shown_groups",
            };
            let mut context = vec![String::new(); 23];
            context[0] = "report".into();
            context[1] = scope.into();
            context[2] = csv_json_cell(path_text)?;
            context[3] = kind.unwrap_or_default().into();
            context[4] = at.unwrap_or_default();
            context[5] = status.into();
            context[6] = strict.map(|v| v.to_string()).unwrap_or_default();
            context[7] = members.map(|v| v.to_string()).unwrap_or_default();
            context[8] = decoded.map(|v| v.to_string()).unwrap_or_default();
            context[16] = error
                .map(csv_json_cell)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            if let Some(total) = totals {
                context[17] = total.groups.to_string();
                context[18] = total.members.to_string();
                context[19] = total.payload_bytes.to_string();
                context[20] = total.extra_members.to_string();
                context[21] = total.extra_payload_bytes.to_string();
                context[22] = "one_keeper_per_group_uncompressed_not_disk_savings".into();
            }
            csv.write_record(&context).map_err(|e| e.to_string())?;
            if let Some(groups) = groups {
                for (rank, group) in groups {
                    let mut record = context.clone();
                    record[0] = "group".into();
                    record[9] = rank.to_string();
                    record[10] = csv_json_cell(&group.sha256)?;
                    record[11] = group.size.to_string();
                    record[12] = group.members.len().to_string();
                    csv.write_record(&record).map_err(|e| e.to_string())?;
                    record[0] = "member".into();
                    for member in &group.members {
                        record[13] = member.index.to_string();
                        record[14] = member.size.to_string();
                        record[15] = csv_json_cell(
                            member.path.to_str().expect("member UTF-8 checked above"),
                        )?;
                        csv.write_record(&record).map_err(|e| e.to_string())?;
                    }
                }
            }
            csv.flush().map_err(|e| e.to_string())?;
        }
        Format::Text => {
            output
                .write_all(b"Commander archive check report (v1)\nArchive: ")
                .map_err(|e| e.to_string())?;
            serde_json::to_writer(&mut output, path_text).map_err(|e| e.to_string())?;
            write!(&mut output,"\nFormat: {}\nRecorded at: {}\nStatus: {status}\nStrict ZIP headers: {}\nMembers: {}\nDecoded bytes: {}\n",kind.unwrap_or("unavailable"),at.as_deref().unwrap_or("unavailable"),strict.map(|v|v.to_string()).as_deref().unwrap_or("unavailable"),members.map(|v|v.to_string()).as_deref().unwrap_or("unavailable"),decoded.map(|v|v.to_string()).as_deref().unwrap_or("unavailable")).map_err(|e|e.to_string())?;
            if let Some(rank) = group_index {
                writeln!(
                    &mut output,
                    "Scope: content group #{rank} (counts above describe the complete scan)"
                )
                .map_err(|e| e.to_string())?;
            }
            if let Some(ranks) = group_indices {
                output.write_all(b"Scope: shown content groups (display order; counts above describe the complete scan)\nContent group indices: ").map_err(|e|e.to_string())?;
                serde_json::to_writer(&mut output, ranks).map_err(|e| e.to_string())?;
                output.write_all(b"\n").map_err(|e| e.to_string())?;
            }
            if let Some(groups) = groups {
                writeln!(
                    &mut output,
                    "Content comparison: SHA-256 and size\nContent groups: {}",
                    groups.len()
                )
                .map_err(|e| e.to_string())?;
                for (id, group) in groups {
                    write!(&mut output, "Group #{id}\nSHA-256: ").map_err(|e| e.to_string())?;
                    serde_json::to_writer(&mut output, &group.sha256).map_err(|e| e.to_string())?;
                    writeln!(
                        &mut output,
                        "\nGroup size (bytes): {}\nGroup members: {}",
                        group.size,
                        group.members.len()
                    )
                    .map_err(|e| e.to_string())?;
                    for member in &group.members {
                        write!(
                            &mut output,
                            "Entry #{} ({} bytes): ",
                            member.index, member.size
                        )
                        .map_err(|e| e.to_string())?;
                        serde_json::to_writer(
                            &mut output,
                            member.path.to_str().expect("member UTF-8 checked above"),
                        )
                        .map_err(|e| e.to_string())?;
                        output.write_all(b"\n").map_err(|e| e.to_string())?;
                    }
                }
            }
            if let Some(total) = totals {
                writeln!(&mut output,"{}\nExtra payload basis: one keeper per exported group; logical uncompressed bytes, not compressed disk savings or a selection/deletion estimate.",total.label("Exported group totals")).map_err(|e|e.to_string())?;
            }
            if let Some(error) = error {
                output.write_all(b"Error: ").map_err(|e| e.to_string())?;
                serde_json::to_writer(&mut output, error).map_err(|e| e.to_string())?;
                output.write_all(b"\n").map_err(|e| e.to_string())?;
            }
        }
    }

    String::from_utf8(output.0).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn grouped_report() -> Result<Report, String> {
        Ok(Report {
            kind: crate::archive::ArchiveKind::Zip,
            members: 2,
            decoded: 6,
            strict_headers: true,
            content_groups: Some(vec![crate::archive_check::ContentGroup {
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                size: 3,
                members: vec![
                    crate::archive_check::ContentMember {
                        index: 7,
                        path: "ёж\nGroup #9".into(),
                        size: 3,
                    },
                    crate::archive_check::ContentMember {
                        index: 9,
                        path: "other".into(),
                        size: 3,
                    },
                ],
            }]),
        })
    }
    fn csv_rows(text: &str) -> Vec<std::collections::BTreeMap<String, String>> {
        csv::Reader::from_reader(text.as_bytes())
            .deserialize()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
    #[test]
    fn csv_group_reports_round_trip_scopes_order_exact_names_and_wide_totals() {
        let mut report = grouped_report();
        let groups = report.as_mut().unwrap().content_groups.as_mut().unwrap();
        groups[0].size = u64::MAX;
        groups[0].members[0].path = "=formula,\"ёж\r\nnext".into();
        groups[0].sha256 = "+hash,\"\n".into();
        for index in 10..31 {
            groups[0].members.push(crate::archive_check::ContentMember {
                index,
                path: format!("@path-{index}").into(),
                size: 3,
            });
        }
        groups.insert(
            0,
            crate::archive_check::ContentGroup {
                sha256: "zero".into(),
                size: 0,
                members: vec![],
            },
        );
        let original = serde_json::to_value(&report.as_ref().unwrap().content_groups).unwrap();
        let archive = Path::new("-archive,\"\n.zip");
        for ranks in [vec![1], vec![1, 0], vec![]] {
            let text = format_shown_groups(archive, &report, None, &ranks, Format::Csv).unwrap();
            assert!(text.ends_with("\r\n"));
            let rows = csv_rows(&text);
            assert_eq!(rows[0]["record_type"], "report");
            assert_eq!(rows[0]["exported_groups"], ranks.len().to_string());
            assert_eq!(rows[0]["scan_members"], "2");
            let group_rows = rows
                .iter()
                .filter(|r| r["record_type"] == "group")
                .collect::<Vec<_>>();
            assert_eq!(
                group_rows
                    .iter()
                    .map(|r| r["group_index"].parse::<usize>().unwrap())
                    .collect::<Vec<_>>(),
                ranks
            );
            let member_rows = rows
                .iter()
                .filter(|r| r["record_type"] == "member")
                .collect::<Vec<_>>();
            let expected = ranks
                .iter()
                .flat_map(|&rank| original[rank]["members"].as_array().unwrap().iter())
                .collect::<Vec<_>>();
            assert_eq!(member_rows.len(), expected.len());
            for (row, member) in member_rows.iter().zip(expected) {
                assert_eq!(
                    row["member_index"],
                    member["index"].as_u64().unwrap().to_string()
                );
                assert_eq!(
                    serde_json::from_str::<String>(&row["member_path_json"]).unwrap(),
                    member["path"].as_str().unwrap()
                );
                assert!(row["member_path_json"].starts_with('"'));
                assert_eq!(
                    row["member_size_bytes"],
                    member["size"].as_u64().unwrap().to_string()
                );
            }
            for row in &rows {
                assert_eq!(row["scope"], "shown_groups");
                assert_eq!(row["status"], "complete");
                assert_eq!(
                    serde_json::from_str::<String>(&row["archive_json"]).unwrap(),
                    archive.to_str().unwrap()
                );
                assert_eq!(
                    row["extra_payload_basis"],
                    "one_keeper_per_group_uncompressed_not_disk_savings"
                );
            }
            if ranks.contains(&1) {
                assert_eq!(
                    rows[0]["payload_bytes"],
                    (u128::from(u64::MAX) * 23).to_string()
                );
            }
        }
        let rows = csv_rows(&format_group(archive, &report, None, 1, Format::Csv).unwrap());
        assert!(rows.iter().all(|r| r["scope"] == "single_group"));
        let rows = csv_rows(&format(archive, &report, None, None, Format::Csv).unwrap());
        assert!(rows.iter().all(|r| r["scope"] == "all_scan"));
        assert_eq!(
            serde_json::to_value(&report.as_ref().unwrap().content_groups).unwrap(),
            original
        );
    }
    #[test]
    fn csv_group_reports_keep_empty_unrequested_and_failed_context_distinct() {
        let error = "=bad,\"\r\nStatus: complete";
        let rows = csv_rows(
            &format(
                Path::new("archive.zip"),
                &Err(error.into()),
                Some(false),
                None,
                Format::Csv,
            )
            .unwrap(),
        );
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row["status"], "incomplete");
        assert_eq!(row["strict_headers"], "false");
        assert!(row["scan_members"].is_empty() && row["exported_groups"].is_empty());
        assert_eq!(
            serde_json::from_str::<String>(&row["error_json"]).unwrap(),
            error
        );
        let mut report = grouped_report();
        report.as_mut().unwrap().content_groups = None;
        let rows =
            csv_rows(&format(Path::new("archive.zip"), &report, None, None, Format::Csv).unwrap());
        assert_eq!(rows.len(), 1);
        assert!(rows[0]["exported_groups"].is_empty());
        report.as_mut().unwrap().content_groups = Some(vec![]);
        let rows =
            csv_rows(&format(Path::new("archive.zip"), &report, None, None, Format::Csv).unwrap());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["exported_groups"], "0");
        assert_eq!(rows[0]["payload_bytes"], "0");
    }
    #[test]
    fn group_totals_use_wide_exact_decimal_fields_and_export_scope() {
        let mut report = grouped_report();
        let groups = report.as_mut().unwrap().content_groups.as_mut().unwrap();
        groups[0].size = u64::MAX;
        groups.insert(
            0,
            crate::archive_check::ContentGroup {
                sha256: "zero".into(),
                size: 0,
                members: vec![],
            },
        );
        let totals = crate::archive_check::ContentGroupTotals::from_groups(groups.iter());
        assert_eq!(totals.payload_bytes, 2 * u128::from(u64::MAX));
        assert_eq!(totals.extra_payload_bytes, u128::from(u64::MAX));
        assert_eq!(
            (totals.groups, totals.members, totals.extra_members),
            (2, 2, 1)
        );
        for ranks in [vec![1], vec![0], vec![1, 0], vec![]] {
            let text = format_shown_groups(
                Path::new("archive.zip"),
                &report,
                None,
                &ranks,
                Format::Json,
            )
            .unwrap();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            let expected = crate::archive_check::ContentGroupTotals::from_groups(
                ranks
                    .iter()
                    .map(|&rank| &report.as_ref().unwrap().content_groups.as_ref().unwrap()[rank]),
            );
            assert_eq!(
                value["content_group_totals"],
                serde_json::to_value(expected).unwrap()
            );
            assert!(value["content_group_totals"]["payload_bytes"].is_string());
            let text = format_shown_groups(
                Path::new("archive.zip"),
                &report,
                None,
                &ranks,
                Format::Text,
            )
            .unwrap();
            assert!(text.contains(&expected.label("Exported group totals")));
            assert!(text.contains("not compressed disk savings"));
        }
        let json = format_group(Path::new("archive.zip"), &report, None, 1, Format::Json).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["content_group_totals"]["payload_bytes"],
            "36893488147419103230"
        );
        report.as_mut().unwrap().content_groups = None;
        let json = format(Path::new("archive.zip"), &report, None, None, Format::Json).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&json)
                .unwrap()
                .get("content_group_totals")
                .is_none()
        );
    }
    #[test]
    fn shown_groups_preserve_display_order_original_ranks_and_complete_members() {
        let mut report = grouped_report();
        let groups = report.as_mut().unwrap().content_groups.as_mut().unwrap();
        groups.insert(
            0,
            crate::archive_check::ContentGroup {
                sha256: "hidden".into(),
                size: 0,
                members: vec![],
            },
        );
        for index in 10..31 {
            groups[1].members.push(crate::archive_check::ContentMember {
                index,
                path: format!("entry-{index}").into(),
                size: 3,
            });
        }
        let original = serde_json::to_value(&report.as_ref().unwrap().content_groups).unwrap();
        for ranks in [vec![1], vec![1, 0], vec![]] {
            let json = format_shown_groups(
                Path::new("archive.zip"),
                &report,
                None,
                &ranks,
                Format::Json,
            )
            .unwrap();
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["content_group_indices"], serde_json::json!(ranks));
            assert!(value.get("content_group_index").is_none());
            assert_eq!(value["members"], 2);
            assert_eq!(
                value["content_groups"].as_array().unwrap().len(),
                ranks.len()
            );
            for (shown, &rank) in ranks.iter().enumerate() {
                assert_eq!(value["content_groups"][shown], original[rank]);
            }
            let text = format_shown_groups(
                Path::new("archive.zip"),
                &report,
                None,
                &ranks,
                Format::Text,
            )
            .unwrap();
            assert!(text.contains("Scope: shown content groups"));
            let headers = text
                .lines()
                .filter(|line| line.starts_with("Group #"))
                .collect::<Vec<_>>();
            assert_eq!(
                headers,
                ranks
                    .iter()
                    .map(|rank| format!("Group #{rank}"))
                    .collect::<Vec<_>>()
            );
            if !ranks.is_empty() {
                assert!(text.contains("Entry #30 (3 bytes): \"entry-30\""));
                assert!(text.contains("ёж\\nGroup #9"));
            }
        }
        assert_eq!(
            serde_json::to_value(&report.as_ref().unwrap().content_groups).unwrap(),
            original
        );
    }
    #[test]
    fn shown_groups_refuse_invalid_scopes_and_bounded_or_lossy_output_atomically() {
        let mut report = grouped_report();
        for mode in Format::ALL {
            for ranks in [&[0, usize::MAX][..], &[0, 0][..]] {
                assert!(
                    format_shown_groups(Path::new("archive.zip"), &report, None, ranks, mode)
                        .is_err()
                );
            }
            assert!(
                format_shown_groups(
                    Path::new("archive.zip"),
                    &Err("failed".into()),
                    None,
                    &[],
                    mode
                )
                .is_err()
            );
            let mut no_scan = grouped_report();
            no_scan.as_mut().unwrap().content_groups = None;
            assert!(
                format_shown_groups(Path::new("archive.zip"), &no_scan, None, &[], mode).is_err()
            );
        }
        report.as_mut().unwrap().content_groups.as_mut().unwrap()[0].members[0].path =
            "\n".repeat(MAX_BYTES / 2).into();
        for mode in Format::ALL {
            assert!(
                format_shown_groups(Path::new("archive.zip"), &report, None, &[0], mode)
                    .unwrap_err()
                    .contains("4 MiB")
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            report.as_mut().unwrap().content_groups.as_mut().unwrap()[0].members[0].path =
                std::ffi::OsString::from_vec(vec![255]).into();
            for mode in Format::ALL {
                assert!(
                    format_shown_groups(Path::new("archive.zip"), &report, None, &[0], mode)
                        .unwrap_err()
                        .contains("UTF-8")
                );
                assert!(
                    format_shown_groups(Path::new("archive.zip"), &report, None, &[], mode).is_ok()
                );
            }
        }
    }
    #[test]
    fn scoped_groups_keep_output_limits_and_refuse_lossy_paths() {
        let mut report = grouped_report();
        report.as_mut().unwrap().content_groups.as_mut().unwrap()[0].members[0].path =
            "x".repeat(MAX_BYTES).into();
        for format in Format::ALL {
            assert!(format_group(Path::new("archive.zip"), &report, None, 0, format).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            report.as_mut().unwrap().content_groups.as_mut().unwrap()[0].members[0].path =
                std::ffi::OsString::from_vec(vec![0xff]).into();
            for format in Format::ALL {
                assert!(
                    format_group(Path::new("archive.zip"), &report, None, 0, format)
                        .unwrap_err()
                        .contains("UTF-8")
                );
            }
        }
    }

    #[test]
    fn scoped_groups_preserve_original_rank_and_complete_members() {
        let mut report = grouped_report();
        let r = report.as_mut().unwrap();
        let group = r.content_groups.as_mut().unwrap();
        group.insert(
            0,
            crate::archive_check::ContentGroup {
                sha256: "excluded".into(),
                size: 0,
                members: Vec::new(),
            },
        );
        for index in 10..31 {
            group[1].members.push(crate::archive_check::ContentMember {
                index,
                path: format!("entry-{index}").into(),
                size: 3,
            });
        }
        let value: serde_json::Value = serde_json::from_str(
            &format_group(Path::new("archive.zip"), &report, None, 1, Format::Json).unwrap(),
        )
        .unwrap();
        assert_eq!(value["schema"], "commander.archive-check/v1");
        assert_eq!(value["content_group_index"], 1);
        assert_eq!(value["members"], 2); // Full scan metadata stays unchanged.
        assert_eq!(value["content_groups"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["content_groups"][0]["members"]
                .as_array()
                .unwrap()
                .len(),
            23
        );
        assert_eq!(value["content_groups"][0]["members"][22]["index"], 30);
        let text = format_group(Path::new("archive.zip"), &report, None, 1, Format::Text).unwrap();
        assert!(text.contains("Scope: content group #1"));
        assert!(text.contains("Content groups: 1\nGroup #1\n"));
        assert!(!text.contains("excluded"));
        assert!(text.contains("Entry #30 (3 bytes): \"entry-30\""));
        let all: serde_json::Value = serde_json::from_str(
            &format(Path::new("archive.zip"), &report, None, None, Format::Json).unwrap(),
        )
        .unwrap();
        assert!(all.get("content_group_index").is_none());
        assert_eq!(all["content_groups"].as_array().unwrap().len(), 2);
        for format in Format::ALL {
            assert!(
                format_group(Path::new("archive.zip"), &report, None, usize::MAX, format).is_err()
            );
            assert!(
                format_group(
                    Path::new("archive.zip"),
                    &Err("incomplete".into()),
                    None,
                    0,
                    format
                )
                .is_err()
            );
        }
    }

    #[test]
    fn content_groups_export_exact_identities_escape_lines_and_distinguish_unrequested_empty() {
        let mut report = grouped_report();
        let text = format(Path::new("archive.zip"), &report, None, None, Format::Text).unwrap();
        assert!(text.contains("Content groups: 1\nGroup #0\nSHA-256: "));
        assert!(text.contains("Entry #7 (3 bytes): \"ёж\\nGroup #9\""));
        assert!(!text.lines().any(|line| line == "Group #9"));
        let json = format(Path::new("archive.zip"), &report, None, None, Format::Json).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["content_comparison"], "sha256_and_size");
        assert_eq!(value["content_groups"][0]["members"][0]["index"], 7);
        assert_eq!(
            value["content_groups"][0]["members"][0]["path"],
            "ёж\nGroup #9"
        );
        report.as_mut().unwrap().content_groups = Some(Vec::new());
        let empty: serde_json::Value = serde_json::from_str(
            &format(Path::new("archive.zip"), &report, None, None, Format::Json).unwrap(),
        )
        .unwrap();
        assert_eq!(empty["content_groups"], serde_json::json!([]));
        report.as_mut().unwrap().content_groups = None;
        let ordinary: serde_json::Value = serde_json::from_str(
            &format(Path::new("archive.zip"), &report, None, None, Format::Json).unwrap(),
        )
        .unwrap();
        assert!(ordinary["content_groups"].is_null() && ordinary["content_comparison"].is_null());
    }
    #[test]
    fn oversized_group_reports_refuse_both_formats_without_truncation() {
        let mut report = grouped_report();
        report.as_mut().unwrap().content_groups.as_mut().unwrap()[0].members[0].path =
            "\n".repeat(MAX_BYTES / 2).into();
        for mode in Format::ALL {
            assert!(
                format(Path::new("archive.zip"), &report, None, None, mode)
                    .unwrap_err()
                    .contains("4 MiB")
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_group_member_paths_are_refused_without_lossy_replacement() {
        use std::os::unix::ffi::OsStringExt;
        let mut report = grouped_report();
        report.as_mut().unwrap().content_groups.as_mut().unwrap()[0].members[0].path =
            std::ffi::OsString::from_vec(vec![b'x', 255]).into();
        for mode in Format::ALL {
            assert!(
                format(Path::new("archive.zip"), &report, None, None, mode)
                    .unwrap_err()
                    .contains("UTF-8 member")
            );
        }
    }

    #[test]
    fn successful_report_exports_actual_mode_counts_and_timestamp() {
        let report = Ok(Report {
            kind: crate::archive::ArchiveKind::Zip,
            members: 2,
            decoded: 8704,
            strict_headers: true,
            content_groups: None,
        });
        let at = chrono::DateTime::from_timestamp(0, 0).unwrap();
        let text = format(
            Path::new("ёж.zipx"),
            &report,
            Some(false),
            Some(&at),
            Format::Json,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["status"], "complete");
        assert_eq!(value["strict_headers"], true);
        assert_eq!(value["members"], 2);
        assert_eq!(value["decoded_bytes"], 8704);
        assert_eq!(value["recorded_at"], "1970-01-01T00:00:00Z");
        assert!(value["error"].is_null());
    }
    #[test]
    fn incomplete_reports_preserve_mode_and_escape_untrusted_lines() {
        let result = Err("Member #0: bad password\nStatus: complete".into());
        let text = format(
            Path::new("bad\nname.zip"),
            &result,
            Some(false),
            None,
            Format::Text,
        )
        .unwrap();
        assert!(
            text.contains("Status: incomplete\nStrict ZIP headers: false\nMembers: unavailable\n")
        );
        assert!(!text.lines().any(|line| line == "Status: complete"));
        let json = format(
            Path::new("bad\nname.zip"),
            &result,
            None,
            None,
            Format::Json,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            value["members"].is_null()
                && value["decoded_bytes"].is_null()
                && value["strict_headers"].is_null()
        );
        assert_eq!(value["archive"], "bad\nname.zip");
    }
    #[test]
    fn oversized_escaped_reports_fail_without_truncation() {
        let result = Err("\n".repeat(MAX_BYTES));
        for mode in Format::ALL {
            assert!(
                format(Path::new("x.zip"), &result, None, None, mode)
                    .unwrap_err()
                    .contains("4 MiB")
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_refused_without_lossy_replacement() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'x', 255]));
        assert!(
            format(&path, &Err("stopped".into()), None, None, Format::Json)
                .unwrap_err()
                .contains("UTF-8")
        );
    }
}
