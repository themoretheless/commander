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
}
impl Format {
    pub const ALL: [Self; 2] = [Self::Text, Self::Json];
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Copy check report",
            Self::Json => "Copy check report JSON",
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
    content_groups: Option<&'a [crate::archive_check::ContentGroup]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_group_index: Option<usize>,
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
    format_scoped(path, result, requested_strict, recorded_at, format, None)
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
    format_scoped(path, result, None, recorded_at, format, Some(group))
}

fn format_scoped(
    path: &Path,
    result: &Result<Report, String>,
    requested_strict: Option<bool>,
    recorded_at: Option<&chrono::DateTime<chrono::Utc>>,
    format: Format,
    group_index: Option<usize>,
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
    let groups = result
        .as_ref()
        .ok()
        .and_then(|report| report.content_groups.as_deref())
        .map(|groups| group_index.map_or(groups, |rank| &groups[rank..rank + 1]));
    if let Some(groups) = groups {
        for group in groups {
            for member in &group.members {
                if member.path.to_str().is_none() {
                    return Err("Archive check report requires UTF-8 member paths; no lossy name was copied.".into());
                }
            }
        }
    }
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
                content_comparison: groups.map(|_| "sha256_and_size"),
                content_groups: groups,
                content_group_index: group_index,
            };
            serde_json::to_writer_pretty(&mut output, &value).map_err(|e| e.to_string())?;
            output.write_all(b"\n").map_err(|e| e.to_string())?;
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
            if let Some(groups) = groups {
                writeln!(
                    &mut output,
                    "Content comparison: SHA-256 and size\nContent groups: {}",
                    groups.len()
                )
                .map_err(|e| e.to_string())?;
                for (id, group) in groups.iter().enumerate() {
                    let id = group_index.unwrap_or(id);
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
