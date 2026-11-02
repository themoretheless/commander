//! Bounded machine-readable snapshots of retained Find results; no filesystem I/O.
use crate::{
    query::MatchMode,
    search::{SearchHit, SearchLocation, TimePivot},
};
use serde::{
    Serialize, Serializer,
    ser::{Error, SerializeSeq},
};
use std::{
    io::{self, Write},
    path::{Component, Path},
    time::SystemTime,
};
const LIMIT: usize = 1024 * 1024;
struct Output(Vec<u8>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > LIMIT.saturating_sub(self.0.len()) {
            return Err(io::Error::other("Search JSON reports are limited to 1 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(tag = "encoding", rename_all = "snake_case")]
enum EncodedPath<'a> {
    Utf8 {
        text: &'a str,
    },
    #[cfg(unix)]
    UnixBytesHex {
        hex: String,
    },
    #[cfg(windows)]
    WindowsUtf16 {
        units: Vec<u16>,
    },
}
fn path(path: &Path, absolute: bool) -> Result<EncodedPath<'_>, String> {
    if path.as_os_str().is_empty()
        || path.is_absolute() != absolute
        || (!absolute
            && path
                .components()
                .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)))
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || path.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err("Search JSON needs absolute native/root paths and enclosed relative archive members without NUL or traversal".into());
    }
    if let Some(text) = path.to_str() {
        return Ok(EncodedPath::Utf8 { text });
    }
    #[cfg(unix)]
    {
        use std::{fmt::Write, os::unix::ffi::OsStrExt};
        let bytes = path.as_os_str().as_bytes();
        if bytes.len() > LIMIT / 2 {
            return Err("Search JSON path exceeds the report budget".into());
        }
        let mut hex = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(&mut hex, "{byte:02x}").expect("String writes cannot fail");
        }
        Ok(EncodedPath::UnixBytesHex { hex })
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().len() > LIMIT / 4 {
            return Err("Search JSON path exceeds the report budget".into());
        }
        Ok(EncodedPath::WindowsUtf16 {
            units: path.as_os_str().encode_wide().collect(),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("Search JSON cannot losslessly encode this platform's path".into())
    }
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Location<'a> {
    Native {
        path: EncodedPath<'a>,
    },
    ArchiveMember {
        archive: EncodedPath<'a>,
        member: EncodedPath<'a>,
        index: usize,
    },
}
#[derive(Serialize)]
struct NativeId {
    device_id: u64,
    file_id: u64,
}
#[derive(Serialize)]
struct ComponentRow<'a> {
    field: &'a str,
    detail: &'a str,
    score: i32,
    matched_character_ranges: &'a [(usize, usize)],
}
#[derive(Serialize)]
struct Row<'a> {
    row: usize,
    display_name: &'a str,
    location: Location<'a>,
    native_id: Option<NativeId>,
    is_directory: bool,
    listed_size_bytes: u64,
    allocated_bytes: Option<u64>,
    hard_links: Option<u64>,
    modified_utc: Option<String>,
    accessed_utc: Option<String>,
    created_utc: Option<String>,
    changed_utc: Option<String>,
    score: i32,
    explanation: Vec<ComponentRow<'a>>,
}
struct Rows<'a>(&'a [&'a SearchHit]);
fn utc(time: SystemTime) -> Option<String> {
    let value = crate::listing_export::iso_8601_utc_precision(time, chrono::SecondsFormat::AutoSi);
    (!value.is_empty()).then_some(value)
}
impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for (index, hit) in self.0.iter().enumerate() {
            let location = match &hit.location {
                SearchLocation::Native(p) => {
                    if p != &hit.entry.path {
                        return Err(S::Error::custom(
                            "Native result path disagrees with its location",
                        ));
                    }
                    Location::Native {
                        path: path(p, true).map_err(S::Error::custom)?,
                    }
                }
                SearchLocation::ArchiveMember {
                    archive,
                    member,
                    index,
                } => Location::ArchiveMember {
                    archive: path(archive, true).map_err(S::Error::custom)?,
                    member: path(member, false).map_err(S::Error::custom)?,
                    index: *index,
                },
            };
            let native_id = if matches!(hit.location, SearchLocation::Native(_)) {
                match hit.identity {
                    crate::search::FileIdentity::Native { volume, file } => Some(NativeId {
                        device_id: volume,
                        file_id: file,
                    }),
                    _ => None,
                }
            } else {
                None
            };
            seq.serialize_element(&Row {
                row: index + 1,
                display_name: &hit.entry.name,
                location,
                native_id,
                is_directory: hit.entry.is_dir,
                listed_size_bytes: hit.entry.size,
                allocated_bytes: hit.entry.allocated_bytes(),
                hard_links: hit.entry.hard_link_count(),
                modified_utc: hit.entry.modified.and_then(utc),
                accessed_utc: hit.accessed.and_then(utc),
                created_utc: if matches!(hit.location, SearchLocation::Native(_)) {
                    hit.entry.created.and_then(utc)
                } else {
                    None
                },
                changed_utc: if matches!(hit.location, SearchLocation::Native(_)) {
                    hit.entry.changed.and_then(utc)
                } else {
                    None
                },
                score: hit.explanation.total_score,
                explanation: hit
                    .explanation
                    .components
                    .iter()
                    .map(|c| ComponentRow {
                        field: c.field,
                        detail: &c.detail,
                        score: c.score,
                        matched_character_ranges: &c.matched_ranges,
                    })
                    .collect(),
            })?;
        }
        seq.end()
    }
}
/// All context describes the query that produced the retained rows, not the editor draft.
#[derive(Serialize)]
pub struct Context<'a> {
    pub expression: &'a str,
    pub mode: MatchMode,
    #[serde(skip)]
    pub root: &'a Path,
    pub generation: u64,
    pub source: &'a str,
    pub completed: bool,
    pub running: bool,
    pub pending_rerun: bool,
    pub draft_differs: bool,
    pub scanned: usize,
    pub matched: usize,
    pub skipped: usize,
    pub truncated: bool,
    pub archive_error: Option<&'a str>,
    pub received_rows: usize,
    pub visible_rows: usize,
    pub visible_selected_rows: usize,
    pub pivot: TimePivot,
    pub result_sort: crate::search_order::Sort,
    pub selected_only: bool,
    pub ordered_at_utc: String,
}
#[derive(Serialize)]
struct Document<'a> {
    format: &'static str,
    version: u32,
    snapshot: &'static str,
    scope: &'static str,
    partial: bool,
    root: EncodedPath<'a>,
    context: &'a Context<'a>,
    exported_rows: usize,
    rows: Rows<'a>,
}
pub fn json(context: &Context<'_>, hits: &[&SearchHit]) -> Result<String, String> {
    let mut output = Output(Vec::new());
    let document = Document {
        format: "commander.search-report",
        version: 1,
        snapshot: "retained search observations",
        scope: if context.selected_only {
            "visible_selected_results"
        } else {
            "visible_results"
        },
        partial: !context.completed
            || context.running
            || context.truncated
            || context.skipped > 0
            || context.archive_error.is_some(),
        root: path(context.root, true)?,
        context,
        exported_rows: hits.len(),
        rows: Rows(hits),
    };
    serde_json::to_writer_pretty(&mut output, &document).map_err(|e| e.to_string())?;
    String::from_utf8(output.0).map_err(|e| e.to_string())
}
