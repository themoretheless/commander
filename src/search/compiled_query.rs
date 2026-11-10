//! Query compilation and per-candidate metadata and content evaluation.

use super::*;

#[derive(Clone, Copy, Default)]
pub(super) struct ObservedMetadata {
    pub(super) is_symlink: Option<bool>,
    pub(super) is_executable: Option<bool>,
    pub(super) is_regular: Option<bool>,
    pub(super) unix_mode: Option<u32>,
    pub(super) owner_uid: Option<u64>,
    pub(super) owner_gid: Option<u64>,
    pub(super) device_id: Option<u64>,
    pub(super) inode: Option<u64>,
    pub(super) file_type_bits: Option<u32>,
    pub(super) is_empty: Option<bool>,
    pub(super) is_broken_symlink: Option<bool>,
}

impl ObservedMetadata {
    pub(super) fn matches_types(
        self,
        entry: &FileEntry,
        types: &[crate::query::TypeCondition],
    ) -> Option<bool> {
        use crate::query::TypeCondition;
        let mut unknown = false;
        for kind in types {
            let matched = match kind {
                TypeCondition::File => Some(!entry.is_dir),
                TypeCondition::Empty => self.is_empty,
                TypeCondition::Regular => self.is_regular,
                TypeCondition::Symlink => self.is_symlink,
                TypeCondition::BrokenSymlink => self.is_broken_symlink,
                TypeCondition::Executable => self.is_executable,
                TypeCondition::Special(kind) => self.file_type_bits.map(|bits| bits == kind.bits()),
                TypeCondition::Kind(kind) => {
                    Some(crate::selection_summary::kind_of(entry) == *kind)
                }
            };
            match matched {
                Some(true) => return Some(true),
                None => unknown = true,
                Some(false) => {}
            }
        }
        if unknown { None } else { Some(false) }
    }
}

pub(super) struct CompiledQuery {
    case_sensitive: bool,
    pub(super) search_archives: bool,
    pub(super) hidden_mode: crate::query::HiddenMode,
    pub(super) exclusions: Arc<Vec<regex::Regex>>,
    pub(super) directory_exclusions: Arc<Vec<crate::query::DirectoryMask>>,
    pub(super) max_depth: Option<usize>,
    pub(super) predicates: Vec<CompiledPredicate>,
    pub(super) requires_content: bool,
    pub(super) requires_empty: bool,
    pub(super) requires_broken_symlink: bool,
    requires_complete_content: bool,
    requires_boundary_context: bool,
    pub(super) requires_symlink_kind: bool,
    pub(super) requires_executable_kind: bool,
    pub(super) requires_regular_kind: bool,
    pub(super) requires_unix_mode: bool,
    pub(super) requires_file_type_bits: bool,
    pub(super) requires_allocation: bool,
    pub(super) requires_links: bool,
}

#[derive(Clone, Copy)]
pub(super) enum TimestampField {
    Modified,
    Accessed,
    Created,
    Changed,
}
impl TimestampField {
    pub(super) fn observed(self, entry: &FileEntry) -> Option<SystemTime> {
        match self {
            Self::Modified => entry.modified,
            Self::Accessed => entry.accessed,
            Self::Created => entry.created,
            Self::Changed => entry.changed,
        }
    }
}
pub(super) enum CompiledPredicate {
    UnixPermissions {
        pattern: crate::query::PermissionPattern,
        detail: String,
    },
    Timestamp {
        field: TimestampField,
        after: bool,
        boundary: SystemTime,
        detail: String,
    },
    NameGlob {
        original: String,
        regex: regex::Regex,
    },
    ExcludeFileGlob {
        original: String,
        regex: regex::Regex,
    },
    NotText {
        path: bool,
        matcher: TextMatcher,
        detail: String,
    },
    Name(TextMatcher),
    AnyName(Vec<TextMatcher>),
    Path(TextMatcher),
    Content(TextMatcher),
    AnyContent(Vec<TextMatcher>),
    NotContent {
        matcher: TextMatcher,
        detail: String,
    },
    Metadata(Predicate),
}

pub(super) enum TextMatcher {
    WholeName {
        original: String,
        regex: regex::Regex,
    },
    WholeLine {
        original: String,
        regex: regex::Regex,
    },
    WholeWord {
        original: String,
        regex: regex::Regex,
    },
    PanelFilter {
        value: String,
        case_sensitive: bool,
    },
    Exact {
        original: String,
        regex: regex::Regex,
    },
    Fuzzy {
        value: String,
        case_sensitive: bool,
    },
    Regex {
        original: String,
        regex: regex::Regex,
    },
}

pub(super) struct TextMatch {
    score: i32,
    pub(super) matched_ranges: Vec<(usize, usize)>,
    detail: String,
}

impl TextMatcher {
    #[cfg(test)]
    pub(super) fn new(mode: MatchMode, value: String) -> Result<Self, QueryError> {
        Self::new_case(mode, value, false)
    }

    fn new_case(mode: MatchMode, value: String, case_sensitive: bool) -> Result<Self, QueryError> {
        match mode {
            MatchMode::Exact => {
                // An escaped literal always parses, but a huge pasted token
                // can still exceed the compiled-size limit.
                let regex = regex::RegexBuilder::new(&regex::escape(&value))
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|error| QueryError {
                        token: value.clone(),
                        message: error.to_string(),
                    })?;
                Ok(Self::Exact {
                    original: value,
                    regex,
                })
            }
            MatchMode::Fuzzy => Ok(Self::Fuzzy {
                value,
                case_sensitive,
            }),
            MatchMode::Regex => {
                let regex = regex::RegexBuilder::new(&value)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|error| QueryError {
                        token: value.clone(),
                        message: error.to_string(),
                    })?;
                Ok(Self::Regex {
                    original: value,
                    regex,
                })
            }
        }
    }

    fn evaluate_bounded(&self, candidate: &str, boundary_unknown: bool) -> Option<TextMatch> {
        let (original, regex, whole_line) = match self {
            Self::WholeWord { original, regex } => (original, regex, false),
            Self::WholeLine { original, regex } => (original, regex, true),
            _ => return self.evaluate(candidate),
        };
        // A lossy replacement at a truncated UTF-8 tail is not a proven delimiter.
        let candidate = if boundary_unknown {
            candidate.trim_end_matches('\u{fffd}')
        } else {
            candidate
        };
        // An empty file has no lines; EOF after a terminator is no extra line.
        if whole_line && candidate.is_empty() {
            return None;
        }
        let found = regex.find_iter(candidate).find(|found| {
            (!boundary_unknown || found.end() < candidate.len())
                && (!whole_line || found.start() < candidate.len())
        })?;
        let range = byte_range_to_char_range(candidate, found.start(), found.end());
        Some(TextMatch {
            score: 120 - range.0.min(100) as i32,
            matched_ranges: vec![range],
            detail: format!(
                "whole {} literal {original:?}",
                if whole_line { "line" } else { "word" }
            ),
        })
    }

    pub(super) fn evaluate(&self, candidate: &str) -> Option<TextMatch> {
        match self {
            Self::WholeWord { .. } | Self::WholeLine { .. } => {
                self.evaluate_bounded(candidate, false)
            }
            Self::PanelFilter {
                value,
                case_sensitive,
            } => {
                if crate::name_filter::is_fuzzy_query(value) {
                    return crate::fuzzy::score_case(value, candidate, *case_sensitive).map(
                        |matched| TextMatch {
                            score: matched.score,
                            matched_ranges: matched.matched_ranges,
                            detail: format!("pane filter {value:?}"),
                        },
                    );
                }
                if !crate::name_filter::is_match_case(value, candidate, *case_sensitive) {
                    return None;
                }
                let ranges =
                    crate::name_filter::matched_ranges_case(value, candidate, *case_sensitive);
                let position = ranges.first().map_or(0, |range| range.0.min(100)) as i32;
                Some(TextMatch {
                    score: 120 - position,
                    matched_ranges: ranges,
                    detail: format!("pane filter {value:?}"),
                })
            }
            Self::Exact { original, regex } | Self::WholeName { original, regex } => {
                let found = regex.find(candidate)?;
                let ranges = vec![byte_range_to_char_range(
                    candidate,
                    found.start(),
                    found.end(),
                )];
                let position = ranges[0].0.min(100) as i32;
                Some(TextMatch {
                    score: 120 - position,
                    matched_ranges: ranges,
                    detail: if matches!(self, Self::WholeName { .. }) {
                        format!("whole name literal {original:?}")
                    } else {
                        format!("literal {original:?}")
                    },
                })
            }
            Self::Fuzzy {
                value,
                case_sensitive,
            } => crate::fuzzy::score_case(value, candidate, *case_sensitive).map(|matched| {
                TextMatch {
                    score: matched.score,
                    matched_ranges: matched.matched_ranges,
                    detail: format!("fuzzy {value:?}"),
                }
            }),
            Self::Regex { original, regex } => regex.find(candidate).map(|found| TextMatch {
                score: 100,
                matched_ranges: vec![byte_range_to_char_range(
                    candidate,
                    found.start(),
                    found.end(),
                )],
                detail: format!("regex {original:?}"),
            }),
        }
    }
}

/// Whether `entry` passes `query`'s metadata stage (everything but content),
/// matching `path:` against the full path.
#[cfg(test)]
pub fn query_matches_metadata(query: &Query, entry: &FileEntry, now: SystemTime) -> bool {
    CompiledQuery::new(query.clone()).is_ok_and(|compiled| {
        compiled
            .evaluate_metadata(
                entry,
                Path::new(""),
                now,
                ObservedMetadata {
                    is_symlink: Some(entry.is_symlink()),
                    is_broken_symlink: entry.symlink.as_ref().and_then(|info| match info.state {
                        crate::panel::LinkTargetState::Available => Some(false),
                        crate::panel::LinkTargetState::Missing => Some(true),
                        crate::panel::LinkTargetState::Unavailable(_) => None,
                    }),
                    unix_mode: entry.permissions.as_ref().and_then(crate::query::unix_mode),
                    is_regular: entry.hard_link_count().map(|_| true),
                    owner_uid: entry.owner_uid,
                    owner_gid: entry.owner_gid,
                    device_id: entry.filesystem_id().map(|p| p.0),
                    inode: entry.filesystem_id().map(|p| p.1),
                    ..Default::default()
                },
            )
            .is_some()
    })
}

impl CompiledQuery {
    pub(super) fn new(query: Query) -> Result<Self, QueryError> {
        query.validate()?;
        let requires_content = query.requires_content();
        let requires_empty = query.predicates.iter().any(|predicate| match predicate {
            Predicate::Empty => true,
            Predicate::TypesAny(types) => types.contains(&crate::query::TypeCondition::Empty),
            _ => false,
        });
        let requires_broken_symlink = query.predicates.iter().any(|p| match p {
            Predicate::BrokenSymlinks => true,
            Predicate::TypesAny(types) => {
                types.contains(&crate::query::TypeCondition::BrokenSymlink)
            }
            _ => false,
        });
        let requires_symlink_kind = query.predicates.contains(&Predicate::SymlinksOnly);
        let requires_executable_kind = query.predicates.contains(&Predicate::ExecutableFiles);
        let requires_regular_kind = query.predicates.contains(&Predicate::RegularFiles);
        let requires_file_type_bits = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::SpecialFileType(_)));
        let requires_unix_mode = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::UnixPermissions(_)));
        let requires_links = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::MinLinks(_) | Predicate::MaxLinks(_)));
        let requires_allocation = query
            .predicates
            .iter()
            .any(|p| matches!(p, Predicate::MinAllocated(_) | Predicate::MaxAllocated(_)));
        let case_sensitive = query.case_sensitive();
        let requires_boundary_context = query.predicates.iter().any(|predicate| {
            matches!(
                predicate,
                Predicate::ContentWord(_)
                    | Predicate::ContentWordExcludes(_)
                    | Predicate::ContentLine(_)
                    | Predicate::ContentLineExcludes(_)
            )
        });
        let requires_complete_content = query.predicates.iter().any(|predicate| {
            matches!(
                predicate,
                Predicate::ContentExcludes(_)
                    | Predicate::ContentWordExcludes(_)
                    | Predicate::ContentLineExcludes(_)
            )
        });
        let hidden_mode = query
            .predicates
            .iter()
            .find_map(|predicate| match predicate {
                Predicate::Hidden(mode) => Some(*mode),
                _ => None,
            })
            .unwrap_or_default();
        let search_archives = !query.predicates.contains(&Predicate::SearchArchives(false));
        let max_depth = query
            .predicates
            .iter()
            .filter_map(|predicate| match predicate {
                Predicate::MaxDepth(depth) => Some(*depth),
                _ => None,
            })
            .min();
        let exclusions = Arc::new(
            query
                .predicates
                .iter()
                .filter_map(|predicate| match predicate {
                    Predicate::ExcludeGlob(value) => {
                        Some(crate::query::compile_name_glob(value, case_sensitive))
                    }
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        let directory_exclusions = Arc::new(
            query
                .predicates
                .iter()
                .filter_map(|predicate| match predicate {
                    Predicate::ExcludeDirGlob(value) => {
                        Some(crate::query::compile_directory_mask(value, case_sensitive))
                    }
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        let mut predicates = Vec::with_capacity(query.predicates.len());
        for predicate in query.predicates {
            let compiled = match predicate {
                Predicate::UnixPermissions(value) => {
                    let pattern = crate::query::parse_permissions(&value)?;
                    CompiledPredicate::UnixPermissions {
                        pattern,
                        detail: format!("perm:{value}"),
                    }
                }
                Predicate::NameGlob(value) => CompiledPredicate::NameGlob {
                    regex: crate::query::compile_name_glob(&value, case_sensitive)?,
                    original: value,
                },
                Predicate::ExcludeFileGlob(value) => CompiledPredicate::ExcludeFileGlob {
                    regex: crate::query::compile_name_glob(&value, case_sensitive)?,
                    original: value,
                },
                Predicate::NameFilter(value) => CompiledPredicate::Name(TextMatcher::PanelFilter {
                    value,
                    case_sensitive,
                }),
                Predicate::NameExact(value) => CompiledPredicate::Name(TextMatcher::WholeName {
                    regex: crate::query::compile_exact_name(&value, case_sensitive)?,
                    original: value,
                }),
                Predicate::NameNotExact(value) => CompiledPredicate::NotText {
                    path: false,
                    detail: format!("no whole name literal match for {value:?}"),
                    matcher: TextMatcher::WholeName {
                        regex: crate::query::compile_exact_name(&value, case_sensitive)?,
                        original: value,
                    },
                },
                Predicate::NameExcludes(value) => CompiledPredicate::NotText {
                    path: false,
                    detail: format!("no {} name match for {value:?}", query.mode.label()),
                    matcher: TextMatcher::new_case(query.mode, value, case_sensitive)?,
                },
                Predicate::PathExcludes(value) => CompiledPredicate::NotText {
                    path: true,
                    detail: format!("no {} path match for {value:?}", query.mode.label()),
                    matcher: TextMatcher::new_case(query.mode, value, case_sensitive)?,
                },
                Predicate::NameContains(value) => CompiledPredicate::Name(TextMatcher::new_case(
                    query.mode,
                    value,
                    case_sensitive,
                )?),
                Predicate::PathContains(value) => CompiledPredicate::Path(TextMatcher::new_case(
                    query.mode,
                    value,
                    case_sensitive,
                )?),
                Predicate::NameAny(values) => CompiledPredicate::AnyName(
                    values
                        .into_iter()
                        .map(|value| TextMatcher::new_case(query.mode, value, case_sensitive))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Predicate::ContentAny(values) => CompiledPredicate::AnyContent(
                    values
                        .into_iter()
                        .map(|value| TextMatcher::new_case(query.mode, value, case_sensitive))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Predicate::ContentLine(value) => {
                    CompiledPredicate::Content(TextMatcher::WholeLine {
                        regex: crate::query::compile_content_line(&value, case_sensitive)?,
                        original: value,
                    })
                }
                Predicate::ContentLineExcludes(value) => CompiledPredicate::NotContent {
                    detail: format!("no whole line literal content match for {value:?}"),
                    matcher: TextMatcher::WholeLine {
                        regex: crate::query::compile_content_line(&value, case_sensitive)?,
                        original: value,
                    },
                },
                Predicate::ContentWord(value) => {
                    CompiledPredicate::Content(TextMatcher::WholeWord {
                        regex: crate::query::compile_content_word(&value, case_sensitive)?,
                        original: value,
                    })
                }
                Predicate::ContentWordExcludes(value) => CompiledPredicate::NotContent {
                    detail: format!("no whole word literal content match for {value:?}"),
                    matcher: TextMatcher::WholeWord {
                        regex: crate::query::compile_content_word(&value, case_sensitive)?,
                        original: value,
                    },
                },
                Predicate::ContentExcludes(value) => CompiledPredicate::NotContent {
                    detail: format!("no {} content match for {value:?}", query.mode.label()),
                    matcher: TextMatcher::new_case(query.mode, value, case_sensitive)?,
                },
                Predicate::ContentContains(value) => CompiledPredicate::Content(
                    TextMatcher::new_case(query.mode, value, case_sensitive)?,
                ),
                Predicate::Extension(value) => {
                    CompiledPredicate::Metadata(Predicate::Extension(if case_sensitive {
                        value
                    } else {
                        value.to_lowercase()
                    }))
                }
                Predicate::ExtensionsAny(values) => {
                    CompiledPredicate::Metadata(Predicate::ExtensionsAny(
                        values
                            .into_iter()
                            .map(|value| {
                                if case_sensitive {
                                    value
                                } else {
                                    value.to_lowercase()
                                }
                            })
                            .collect(),
                    ))
                }
                Predicate::ModifiedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Modified,
                    after: true,
                    detail: format!("after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::ModifiedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Modified,
                    after: false,
                    detail: format!("before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::AccessedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Accessed,
                    after: true,
                    detail: format!("accessed-after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::AccessedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Accessed,
                    after: false,
                    detail: format!("accessed-before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::CreatedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Created,
                    after: true,
                    detail: format!("created-after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::CreatedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Created,
                    after: false,
                    detail: format!("created-before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::ChangedAfter(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Changed,
                    after: true,
                    detail: format!("changed-after:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                Predicate::ChangedBefore(value) => CompiledPredicate::Timestamp {
                    field: TimestampField::Changed,
                    after: false,
                    detail: format!("changed-before:{value}"),
                    boundary: crate::query::parse_absolute_time(&value)?,
                },
                metadata => CompiledPredicate::Metadata(metadata),
            };
            predicates.push(compiled);
        }
        Ok(Self {
            case_sensitive,
            search_archives,
            hidden_mode,
            exclusions,
            directory_exclusions,
            max_depth,
            predicates,
            requires_content,
            requires_empty,
            requires_broken_symlink,
            requires_complete_content,
            requires_boundary_context,
            requires_symlink_kind,
            requires_executable_kind,
            requires_regular_kind,
            requires_unix_mode,
            requires_file_type_bits,
            requires_allocation,
            requires_links,
        })
    }

    /// `root` is the search root: `path:` matches the path below it, as the
    /// Find list shows it, so terms in the root itself do not match every hit.
    pub(super) fn evaluate_metadata(
        &self,
        entry: &FileEntry,
        root: &Path,
        now: SystemTime,
        observed: ObservedMetadata,
    ) -> Option<MatchExplanation> {
        if self.hidden_mode != crate::query::HiddenMode::Include {
            let hidden = dot_hidden_path(&entry.path, root);
            if (self.hidden_mode == crate::query::HiddenMode::Only) != hidden {
                return None;
            }
        }
        if excluded_path(&entry.path, root, &self.exclusions)
            || (entry.is_dir
                && self
                    .directory_exclusions
                    .iter()
                    .any(|pattern| pattern.matches_name(&entry.name)))
        {
            return None;
        }
        let mut explanation = MatchExplanation::default();
        for predicate in &self.predicates {
            match predicate {
                CompiledPredicate::UnixPermissions { pattern, detail } => {
                    let mode = observed.unix_mode?;
                    if !pattern.matches(mode) {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: detail.clone(),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::NameGlob { original, regex } => {
                    if !regex.is_match(&entry.name) {
                        return None;
                    }
                    explanation.total_score += 100;
                    explanation.components.push(MatchComponent {
                        field: "name",
                        detail: format!("glob {original:?}"),
                        score: 100,
                        matched_ranges: vec![(0, entry.name.chars().count())],
                    });
                }
                CompiledPredicate::ExcludeFileGlob { original, regex } => {
                    if !entry.is_dir && regex.is_match(&entry.name) {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: format!("excludefile:{original}"),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::NotText {
                    path,
                    matcher,
                    detail,
                } => {
                    let candidate = if *path {
                        entry
                            .path
                            .strip_prefix(root)
                            .unwrap_or(&entry.path)
                            .to_string_lossy()
                    } else {
                        std::borrow::Cow::Borrowed(entry.name.as_str())
                    };
                    if matcher.evaluate(&candidate).is_some() {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: if *path { "path" } else { "name" },
                        detail: detail.clone(),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::AnyName(matchers) => {
                    let matched = matchers
                        .iter()
                        .find_map(|matcher| matcher.evaluate(&entry.name))?;
                    explanation.total_score += matched.score;
                    explanation.components.push(MatchComponent {
                        field: "name",
                        detail: format!("any name: {}", matched.detail),
                        score: matched.score,
                        matched_ranges: matched.matched_ranges,
                    });
                }
                CompiledPredicate::Name(matcher) => {
                    let matched = matcher.evaluate(&entry.name)?;
                    explanation.total_score += matched.score;
                    explanation.components.push(MatchComponent {
                        field: "name",
                        detail: matched.detail,
                        score: matched.score,
                        matched_ranges: matched.matched_ranges,
                    });
                }
                CompiledPredicate::Path(matcher) => {
                    let path = entry
                        .path
                        .strip_prefix(root)
                        .unwrap_or(&entry.path)
                        .to_string_lossy();
                    let matched = matcher.evaluate(&path)?;
                    explanation.total_score += matched.score;
                    explanation.components.push(MatchComponent {
                        field: "path",
                        detail: matched.detail,
                        score: matched.score,
                        matched_ranges: matched.matched_ranges,
                    });
                }
                CompiledPredicate::Content(_)
                | CompiledPredicate::AnyContent(_)
                | CompiledPredicate::NotContent { .. } => {}
                CompiledPredicate::Timestamp {
                    field,
                    after,
                    boundary,
                    detail,
                } => {
                    let modified = field.observed(entry)?;
                    if !if *after {
                        modified > *boundary
                    } else {
                        modified < *boundary
                    } {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: detail.clone(),
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
                CompiledPredicate::Metadata(predicate) => {
                    let matches = match predicate {
                        Predicate::DeviceId { id, exclude } => observed
                            .device_id
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::Inode { id, exclude } => observed
                            .inode
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::OwnerId { id, exclude } => observed
                            .owner_uid
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::GroupId { id, exclude } => observed
                            .owner_gid
                            .is_some_and(|actual| (actual == *id) != *exclude),
                        Predicate::Empty => observed.is_empty == Some(true),
                        Predicate::TypesAny(types) => {
                            observed.matches_types(entry, types) == Some(true)
                        }
                        Predicate::SymlinksOnly => observed.is_symlink == Some(true),
                        Predicate::BrokenSymlinks => observed.is_broken_symlink == Some(true),
                        Predicate::ExecutableFiles => observed.is_executable == Some(true),
                        Predicate::RegularFiles => observed.is_regular == Some(true),
                        Predicate::SpecialFileType(kind) => {
                            observed.file_type_bits == Some(kind.bits())
                        }
                        Predicate::Extension(value) if self.case_sensitive => {
                            !entry.is_dir && crate::query::extension_matches(&entry.name, value)
                        }
                        Predicate::ExtensionsAny(values) if self.case_sensitive => {
                            !entry.is_dir
                                && values.iter().any(|value| {
                                    crate::query::extension_matches(&entry.name, value)
                                })
                        }
                        Predicate::MinDepth(min) => {
                            search_depth(&entry.path, root).is_some_and(|depth| depth >= *min)
                        }
                        Predicate::MaxDepth(max) => {
                            search_depth(&entry.path, root).is_some_and(|depth| depth <= *max)
                        }
                        Predicate::MinLinks(_) | Predicate::MaxLinks(_) => {
                            observed.is_regular == Some(true)
                                && observed.is_symlink == Some(false)
                                && !entry.is_dir
                                && predicate.matches_metadata(entry, now)
                        }
                        _ => predicate.matches_metadata(entry, now),
                    };
                    if !matches {
                        return None;
                    }
                    explanation.components.push(MatchComponent {
                        field: "filter",
                        detail: match predicate {
                            Predicate::DeviceId { .. } => format!(
                                "{} (observed device {})",
                                predicate.chip(),
                                observed.device_id?
                            ),
                            Predicate::Inode { .. } => {
                                format!("{} (observed inode {})", predicate.chip(), observed.inode?)
                            }
                            Predicate::MinLinks(_) | Predicate::MaxLinks(_) => format!(
                                "{} (observed {} hard links)",
                                predicate.chip(),
                                entry.hard_link_count()?
                            ),
                            Predicate::MinAllocated(_) | Predicate::MaxAllocated(_) => format!(
                                "{} (observed {} allocated bytes)",
                                predicate.chip(),
                                entry.allocated_bytes()?
                            ),
                            _ => predicate.chip(),
                        },
                        score: 0,
                        matched_ranges: Vec::new(),
                    });
                }
            }
        }
        Some(explanation)
    }

    pub(super) fn evaluate_content(
        &self,
        entry: &FileEntry,
        source: &CandidateContent,
        explanation: &mut MatchExplanation,
    ) -> ContentVerdict {
        if !self.requires_content {
            return ContentVerdict::Matched;
        }
        if entry.is_dir {
            return ContentVerdict::NotMatched;
        }
        match source {
            CandidateContent::OnDisk => {
                let Some((mut file, size)) = open_regular_file(&entry.path) else {
                    return ContentVerdict::Skipped;
                };
                let Some(bytes) = read_text_bytes(&mut file, size) else {
                    return ContentVerdict::Skipped;
                };
                let mut boundary_unknown = size > MAX_CONTENT_BYTES;
                if self.requires_complete_content || self.requires_boundary_context {
                    let mut extra = [0u8; 1];
                    let eof = file.read(&mut extra).ok() == Some(0);
                    if self.requires_complete_content && (size > MAX_CONTENT_BYTES || !eof) {
                        return ContentVerdict::Skipped;
                    }
                    boundary_unknown = !eof;
                }
                let content = crate::search_text::decode(&bytes);
                self.evaluate_content_text_with_boundaries(
                    &content,
                    size > MAX_CONTENT_BYTES,
                    boundary_unknown,
                    explanation,
                )
            }
            CandidateContent::Indexed { index, document } => {
                let Some(document) = index.documents.get(*document) else {
                    return ContentVerdict::Skipped;
                };
                let Some(content) = document.content.as_deref() else {
                    return ContentVerdict::Skipped;
                };
                if self.requires_complete_content && !document.content_complete {
                    return ContentVerdict::Skipped;
                }
                self.evaluate_content_text_with_boundaries(
                    content,
                    false,
                    !document.content_complete,
                    explanation,
                )
            }
            CandidateContent::Inline(content) => {
                let Some(content) = content.as_deref() else {
                    return ContentVerdict::Skipped;
                };
                self.evaluate_content_text(content, false, explanation)
            }
        }
    }

    pub(super) fn evaluate_content_text(
        &self,
        content: &str,
        partial: bool,
        explanation: &mut MatchExplanation,
    ) -> ContentVerdict {
        self.evaluate_content_text_with_boundaries(content, partial, partial, explanation)
    }

    fn evaluate_content_text_with_boundaries(
        &self,
        content: &str,
        partial: bool,
        boundary_unknown: bool,
        explanation: &mut MatchExplanation,
    ) -> ContentVerdict {
        if (partial || boundary_unknown) && self.requires_complete_content {
            return ContentVerdict::Skipped;
        }
        for predicate in &self.predicates {
            if let CompiledPredicate::NotContent { matcher, detail } = predicate {
                if matcher.evaluate(content).is_some() {
                    return ContentVerdict::NotMatched;
                }
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: detail.clone(),
                    score: 0,
                    matched_ranges: Vec::new(),
                });
            }
            if let CompiledPredicate::AnyContent(matchers) = predicate {
                let Some(matched) = matchers
                    .iter()
                    .find_map(|matcher| matcher.evaluate(content))
                else {
                    return ContentVerdict::NotMatched;
                };
                explanation.total_score += matched.score;
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: if partial {
                        format!("any content: {}, first 16MB", matched.detail)
                    } else {
                        format!("any content: {}", matched.detail)
                    },
                    score: matched.score,
                    matched_ranges: matched.matched_ranges,
                });
            }
            if let CompiledPredicate::Content(matcher) = predicate {
                let Some(matched) = matcher.evaluate_bounded(content, boundary_unknown) else {
                    return if boundary_unknown
                        && matches!(
                            matcher,
                            TextMatcher::WholeWord { .. } | TextMatcher::WholeLine { .. }
                        ) {
                        ContentVerdict::Skipped
                    } else {
                        ContentVerdict::NotMatched
                    };
                };
                explanation.total_score += matched.score;
                explanation.components.push(MatchComponent {
                    field: "content",
                    detail: if partial {
                        format!("{}, first 16MB", matched.detail)
                    } else if boundary_unknown
                        && matches!(
                            matcher,
                            TextMatcher::WholeWord { .. } | TextMatcher::WholeLine { .. }
                        )
                    {
                        format!("{}, bounded cached text", matched.detail)
                    } else {
                        matched.detail
                    },
                    score: matched.score,
                    matched_ranges: matched.matched_ranges,
                });
            }
        }
        ContentVerdict::Matched
    }
}

/// Leading bytes checked for NULs to tell binary files from text.
pub(super) const BINARY_SNIFF_BYTES: u64 = 8_192;

/// Up to `MAX_CONTENT_BYTES` of a text file, or `None` for a binary file or
/// a read error. The head is sniffed before the rest is read, so a binary
/// file costs one small read instead of up to 16 MB.
pub(super) fn read_text_bytes(reader: impl Read, size: u64) -> Option<Vec<u8>> {
    let mut reader = reader.take(MAX_CONTENT_BYTES);
    let mut bytes = Vec::with_capacity(usize::try_from(size.min(BINARY_SNIFF_BYTES)).ok()?);
    Read::by_ref(&mut reader)
        .take(BINARY_SNIFF_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    if crate::search_text::binary_head(&bytes) {
        return None;
    }
    bytes.reserve(
        usize::try_from(size.min(MAX_CONTENT_BYTES))
            .unwrap_or(0)
            .saturating_sub(bytes.len()),
    );
    reader.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Open `path` for a content read only if it is a regular file, returning the
/// handle and its size. FIFOs, sockets and devices are refused: opening a
/// FIFO for reading blocks until a writer appears, which would hang the
/// search worker. The path is stat-ed first so devices are never opened;
/// unix then opens non-blocking (and without acquiring a controlling tty) and
/// re-checks the type on the handle itself.
pub(super) fn open_regular_file(path: &Path) -> Option<(std::fs::File, u64)> {
    // Refuse devices, FIFOs and sockets before opening: opening a tty, serial
    // or tape device can have side effects. The fstat below covers the race
    // where the path is swapped after this check.
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    metadata.is_file().then_some((file, metadata.len()))
}
