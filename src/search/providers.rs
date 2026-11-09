//! Search providers: filesystem walk, archive members and the content index.

use super::*;

pub(super) struct FilesystemProvider {
    root: PathBuf,
}

impl FilesystemProvider {
    pub(super) const fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl SearchProvider for FilesystemProvider {
    fn id(&self) -> &'static str {
        "live-search"
    }

    fn label(&self) -> &'static str {
        "Live + archives"
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn visit(&mut self, request: &ProviderRequest, emit: &mut dyn FnMut(ProviderRecord) -> bool) {
        let mut archive_budget = crate::archive::SearchBudget::default();
        let mut walk = jwalk::WalkDir::new(&self.root).skip_hidden(false);
        if let Some(depth) = request.max_depth {
            walk = walk.max_depth(depth);
        }
        if !request.exclusions.is_empty()
            || !request.directory_exclusions.is_empty()
            || request.hidden_mode == crate::query::HiddenMode::Exclude
        {
            let exclusions = Arc::clone(&request.exclusions);
            let directory_exclusions = Arc::clone(&request.directory_exclusions);
            let root = self.root.clone();
            let exclude_hidden = request.hidden_mode == crate::query::HiddenMode::Exclude;
            walk = walk.process_read_dir(move |_, _, _, children| {
                children.retain(|result| {
                    result.as_ref().map_or(true, |entry| {
                        let name = entry.file_name.to_string_lossy();
                        entry.depth == 0
                            || ((!exclude_hidden || !dot_name(&entry.file_name))
                                && !exclusions.iter().any(|pattern| pattern.is_match(&name))
                                && !(entry.file_type.is_dir()
                                    && directory_exclusions.iter().any(|pattern| {
                                        pattern.matches_directory(&entry.path(), &root)
                                    })))
                    })
                });
            });
        }
        for item in walk {
            if request.cancelled() {
                break;
            }
            let item = match item {
                Ok(item) => item,
                // A folder the walk could not read: its children are missing
                // from the results, so count it instead of dropping it.
                Err(_) => {
                    request.note_unsearched();
                    if !emit(None) {
                        break;
                    }
                    continue;
                }
            };
            let path = item.path();
            if path == self.root {
                continue;
            }
            let Ok(metadata) = item.metadata() else {
                if !emit(None) {
                    break;
                }
                continue;
            };
            let identity = FileIdentity::from_metadata(&path, &metadata);
            let accessed = metadata.accessed().ok();
            let Some(entry) = FileEntry::from_meta(path, &metadata) else {
                if !emit(None) {
                    break;
                }
                continue;
            };
            let archive_path = (request.search_archives
                && !entry.is_dir
                && request.max_depth.is_none_or(|max| {
                    search_depth(&entry.path, self.root()).is_some_and(|depth| depth < max)
                })
                && crate::archive::is_supported(&entry.path)
                && crate::trust::allows_auto_archive_inspect(&entry.path))
            .then(|| entry.path.clone());
            let is_empty = if request.requires_empty {
                crate::fs_util::observed_empty(&entry.path, &metadata)
            } else {
                None
            };
            let is_broken_symlink = request
                .requires_broken_symlink
                .then(|| crate::fs_util::observed_broken_symlink(&entry.path, &metadata))
                .flatten();
            if !emit(Some(SearchCandidate {
                location: SearchLocation::Native(entry.path.clone()),
                entry,
                is_symlink: Some(metadata.file_type().is_symlink()),
                is_executable: crate::query::executable_file(&metadata),
                is_regular: Some(metadata.is_file()),
                unix_mode: crate::query::unix_mode(&metadata.permissions()),
                owner_uid: crate::query::native_uid(&metadata),
                owner_gid: crate::query::native_gid(&metadata),
                device_id: entry_identity_parts(&metadata).map(|p| p.0),
                inode: entry_identity_parts(&metadata).map(|p| p.1),
                file_type_bits: crate::query::native_file_type_bits(&metadata),
                is_empty,
                is_broken_symlink,
                identity,
                accessed,
                content: CandidateContent::OnDisk,
            })) {
                break;
            }
            if let Some(archive_path) = archive_path
                && !ArchiveProvider::visit_path(&archive_path, request, emit, &mut archive_budget)
            {
                break;
            }
        }
        request.note_archive_budget(&archive_budget);
    }
}

/// Bounded archive topology proof for empty-directory queries. Known children
/// prove nonemptiness even if another part of the inspection is incomplete.
pub(super) struct EmptyDirectoryTracker {
    occupied: std::collections::HashSet<PathBuf>,
    pub(super) bytes: usize,
    pub(super) limit: usize,
    pub(super) complete: bool,
}

impl EmptyDirectoryTracker {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            occupied: Default::default(),
            bytes: 0,
            limit,
            complete: true,
        }
    }
    fn reserve(&mut self, bytes: usize) -> bool {
        if bytes > self.limit.saturating_sub(self.bytes) {
            self.complete = false;
            return false;
        }
        self.bytes += bytes;
        true
    }
    fn key(path: &Path) -> PathBuf {
        // The archive visitor already rejects escaping paths. Ignore a leading
        // ./ as well as redundant separators when comparing enclosed names.
        path.components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect()
    }
    pub(super) fn observe(&mut self, path: &Path) {
        let path = Self::key(path);
        for ancestor in path
            .ancestors()
            .skip(1)
            .take_while(|path| !path.as_os_str().is_empty())
        {
            if self.occupied.contains(ancestor) {
                continue;
            }
            if !self.reserve(
                ancestor
                    .as_os_str()
                    .as_encoded_bytes()
                    .len()
                    .saturating_add(128),
            ) {
                break;
            }
            self.occupied.insert(ancestor.to_path_buf());
        }
    }
    pub(super) fn reserve_directory(&mut self, path: &Path) -> bool {
        self.reserve(
            path.as_os_str()
                .as_encoded_bytes()
                .len()
                .saturating_mul(2)
                .saturating_add(512),
        )
    }
    pub(super) fn empty(&self, path: &Path, inspection_complete: bool) -> Option<bool> {
        if self.occupied.contains(&Self::key(path)) {
            Some(false)
        } else if self.complete && inspection_complete {
            Some(true)
        } else {
            None
        }
    }
}

pub(super) struct ArchiveProvider {
    archive: PathBuf,
}

impl ArchiveProvider {
    pub(super) fn visit_path(
        archive_path: &Path,
        request: &ProviderRequest,
        emit: &mut dyn FnMut(ProviderRecord) -> bool,
        budget: &mut crate::archive::SearchBudget,
    ) -> bool {
        if !request.search_archives || !crate::trust::allows_auto_archive_inspect(archive_path) {
            return true;
        }
        let metadata = match std::fs::metadata(archive_path) {
            Ok(metadata) => metadata,
            Err(_) => return emit(None),
        };
        let archive_modified = metadata.modified().ok();
        let accessed = metadata.accessed().ok();
        // A member without its own time shows the archive's; format it once.
        let archive_modified_str = modified_label(archive_modified);
        let mut keep_going = true;
        let result = {
            let mut emit_member = |member: crate::archive::SearchMember, is_empty: Option<bool>| {
                if (request.hidden_mode == crate::query::HiddenMode::Exclude
                    && dot_hidden_path(&member.member.path, Path::new("")))
                    || excluded_path(&member.member.path, Path::new(""), &request.exclusions)
                    || excluded_directory_path(
                        &member.member.path,
                        Path::new(""),
                        member.member.is_dir,
                        &request.directory_exclusions,
                    )
                {
                    return true;
                }
                // The virtual path ends in the member's own name.
                if member.member.path.file_name().is_none() {
                    keep_going = emit(None);
                    return keep_going;
                }
                let virtual_path =
                    crate::archive::virtual_member_path(archive_path, &member.member.path);
                let (modified, modified_str) = match member.member.modified {
                    Some(own) => (Some(own), modified_label(Some(own))),
                    None => (archive_modified, archive_modified_str.clone()),
                };
                let Some(entry) = synthetic_entry(
                    virtual_path,
                    member.member.is_dir,
                    member.member.size,
                    modified,
                    modified_str,
                ) else {
                    keep_going = emit(None);
                    return keep_going;
                };
                keep_going = emit(Some(SearchCandidate {
                    is_symlink: Some(member.member.is_symlink),
                    is_executable: member.member.is_executable,
                    is_regular: Some(member.member.is_regular),
                    unix_mode: member.member.unix_mode,
                    owner_uid: member.member.owner_uid,
                    owner_gid: member.member.owner_gid,
                    device_id: None,
                    inode: None,
                    file_type_bits: Some(member.member.file_type_bits),
                    is_empty,
                    is_broken_symlink: (!member.member.is_symlink).then_some(false),
                    entry,
                    identity: FileIdentity::Archive {
                        archive: archive_path.to_path_buf(),
                        member: member.member.path.clone(),
                        index: member.member.index,
                    },
                    accessed,
                    content: CandidateContent::Inline(member.content),
                    location: SearchLocation::ArchiveMember {
                        archive: archive_path.to_path_buf(),
                        member: member.member.path,
                        index: member.member.index,
                    },
                }));
                keep_going
            };
            let mut directories = Vec::new();
            let mut topology = EmptyDirectoryTracker::new(8 * 1024 * 1024);
            let mut stopped = false;
            let result = crate::archive::visit_members_with_password(
                archive_path,
                request.requires_content,
                budget,
                request.archive_password.as_ref().map(|p| p.as_bytes()),
                || request.cancelled(),
                |member| {
                    if request.requires_empty {
                        // Count children before presentation/traversal filters: a hidden
                        // or excluded child still makes its real archive folder nonempty.
                        topology.observe(&member.member.path);
                        if member.member.is_dir {
                            if topology.reserve_directory(&member.member.path) {
                                directories.push(member);
                            } else {
                                // Other known OR branches may still prove this row.
                                let proceed = emit_member(member, None);
                                stopped |= !proceed;
                                return proceed;
                            }
                            return !request.cancelled();
                        }
                    }
                    let is_empty = Some(member.member.is_regular && member.member.size == 0);
                    let proceed = emit_member(member, is_empty);
                    stopped |= !proceed;
                    proceed
                },
            );
            let complete = result.as_ref().is_ok_and(|summary| {
                !summary.cancelled
                    && !summary.members_not_searched
                    && summary.unsafe_members == 0
                    && summary.unreadable_members == 0
            }) && !budget.truncated
                && !request.cancelled();
            if !topology.complete {
                request.archives_capped.set(true);
            }
            if !stopped {
                for member in directories {
                    if request.cancelled() {
                        break;
                    }
                    let is_empty = topology.empty(&member.member.path, complete);
                    if !emit_member(member, is_empty) {
                        break;
                    }
                }
            }
            result
        };
        match result {
            // Content search does not open this format: its members were
            // skipped, so count it rather than drop them silently.
            Ok(summary) if summary.members_not_searched => request.note_unsearched(),
            Ok(_) => {}
            Err(error) => {
                // A corrupt or encrypted archive: its members are not searched.
                request.note_unsearched();
                request.note_archive_error(&error);
                log::info!(target: "commander::search", "archive not searched: {error}");
                if keep_going {
                    keep_going = emit(None);
                }
            }
        }
        keep_going && !request.cancelled()
    }
}

impl SearchProvider for ArchiveProvider {
    fn id(&self) -> &'static str {
        "archive-search"
    }

    fn label(&self) -> &'static str {
        "Archives"
    }

    fn root(&self) -> &Path {
        &self.archive
    }

    fn visit(&mut self, request: &ProviderRequest, emit: &mut dyn FnMut(ProviderRecord) -> bool) {
        let mut budget = crate::archive::SearchBudget::default();
        Self::visit_path(&self.archive, request, emit, &mut budget);
        request.note_archive_budget(&budget);
    }
}

pub(super) struct IndexedProvider {
    pub(super) index: Arc<crate::content_index::RootIndex>,
}

impl SearchProvider for IndexedProvider {
    fn id(&self) -> &'static str {
        "indexed-search"
    }

    fn label(&self) -> &'static str {
        "Index + archives"
    }

    fn root(&self) -> &Path {
        &self.index.root
    }

    fn visit(&mut self, request: &ProviderRequest, emit: &mut dyn FnMut(ProviderRecord) -> bool) {
        let mut archive_budget = crate::archive::SearchBudget::default();
        for (document, indexed) in self.index.documents.iter().enumerate() {
            if request.cancelled() {
                break;
            }
            let Some(entry) = indexed.entry() else {
                if !emit(None) {
                    break;
                }
                continue;
            };
            if request.max_depth.is_some_and(|max| {
                search_depth(&entry.path, self.root()).is_none_or(|depth| depth > max)
            }) {
                continue;
            }
            if (request.hidden_mode == crate::query::HiddenMode::Exclude
                && dot_hidden_path(&entry.path, self.root()))
                || excluded_path(&entry.path, self.root(), &request.exclusions)
                || excluded_directory_path(
                    &entry.path,
                    self.root(),
                    entry.is_dir,
                    &request.directory_exclusions,
                )
            {
                continue;
            }
            let archive_path = (request.search_archives
                && !entry.is_dir
                && request.max_depth.is_none_or(|max| {
                    search_depth(&entry.path, self.root()).is_some_and(|depth| depth < max)
                })
                && crate::archive::is_supported(&entry.path)
                && crate::trust::allows_auto_archive_inspect(&entry.path))
            .then(|| entry.path.clone());
            if !emit(Some(SearchCandidate {
                location: SearchLocation::Native(entry.path.clone()),
                entry,
                is_symlink: indexed.is_symlink,
                is_executable: indexed.is_executable,
                is_regular: indexed.is_regular,
                unix_mode: indexed.unix_mode,
                owner_uid: indexed.owner_uid,
                owner_gid: indexed.owner_gid,
                device_id: indexed.volume,
                inode: indexed.file_id,
                file_type_bits: indexed.file_type_bits(),
                is_empty: indexed.is_empty,
                is_broken_symlink: indexed.broken_symlink(),
                identity: FileIdentity::from_indexed(indexed),
                accessed: indexed.accessed(),
                content: CandidateContent::Indexed {
                    index: Arc::clone(&self.index),
                    document,
                },
            })) {
                break;
            }
            if let Some(archive_path) = archive_path
                && !ArchiveProvider::visit_path(&archive_path, request, emit, &mut archive_budget)
            {
                break;
            }
        }
        request.note_archive_budget(&archive_budget);
    }
}
