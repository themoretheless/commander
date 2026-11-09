//! Selection history, statistics and per-folder selection memory.

use super::*;

pub(super) const MAX_ARCHIVE_HISTORY: usize = 64;
pub(super) const MAX_ARCHIVE_HISTORY_BYTES: usize = 256 * 1024;
#[derive(Clone)]
pub(super) struct ArchiveLocation {
    pub(super) folder: Option<PathBuf>,
    pub(super) focused: Option<usize>,
}
impl ArchiveLocation {
    pub(super) fn bytes(&self) -> usize {
        self.folder
            .as_ref()
            .map_or(0, |path| path.as_os_str().as_encoded_bytes().len())
    }
}

/// Selection edits only, bound to one immutable listing; no filesystem undo.
#[derive(Default)]
pub struct SelectionHistory {
    pub(super) current: Option<Vec<usize>>,
    members: usize,
    pub(super) undo: std::collections::VecDeque<Vec<usize>>,
    pub(super) redo: Vec<Vec<usize>>,
}
impl SelectionHistory {
    pub(super) const MAX_IDS: usize = 65_536;
    const MAX_STEPS: usize = 32;
    pub(super) fn reset(&mut self, selected: &std::collections::BTreeSet<usize>, members: usize) {
        *self = Self {
            members,
            ..Default::default()
        };
        if selected.len() <= Self::MAX_IDS && selected.iter().all(|&id| id < members) {
            self.current = Some(selected.iter().copied().collect());
        }
    }
    pub(crate) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub(super) fn observe(&mut self, selected: &std::collections::BTreeSet<usize>, members: usize) {
        if self.members != members
            || self.current.is_none()
            || selected.len() > Self::MAX_IDS
            || selected.iter().any(|&id| id >= members)
        {
            self.reset(selected, members);
            return;
        }
        let previous = self.current.as_ref().unwrap();
        if previous.iter().copied().eq(selected.iter().copied()) {
            return;
        }
        self.redo.clear();
        self.undo.push_back(self.current.take().unwrap());
        self.current = Some(selected.iter().copied().collect());
        while self.undo.len() > Self::MAX_STEPS
            || self.undo.iter().map(Vec::len).sum::<usize>() + selected.len() > Self::MAX_IDS
        {
            self.undo.pop_front();
        }
    }
    pub(super) fn restore(
        &mut self,
        selected: &mut std::collections::BTreeSet<usize>,
        members: usize,
        redo: bool,
    ) {
        self.observe(selected, members);
        let next = if redo {
            self.redo.pop()
        } else {
            self.undo.pop_back()
        };
        let Some(next) = next else {
            return;
        };
        let current = self.current.replace(next.clone()).unwrap();
        if redo {
            self.undo.push_back(current);
        } else {
            self.redo.push(current);
        }
        *selected = next.into_iter().collect();
    }
}

/// One explicit snapshot, owned by the immutable archive listing.
#[derive(Default)]
pub(super) struct SelectionMemory(pub(super) Option<Vec<usize>>);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct SelectionStats {
    pub(super) files: usize,
    pub(super) folders: usize,
    pub(super) links: usize,
    pub(super) other: usize,
    pub(super) unpacked: u128,
    pub(super) packed: Option<u128>,
}
impl SelectionStats {
    pub(super) fn compute(
        listing: &crate::archive::ArchiveListing,
        ids: &std::collections::BTreeSet<usize>,
    ) -> Self {
        let mut stats = Self {
            packed: (listing.kind == crate::archive::ArchiveKind::Zip).then_some(0),
            ..Default::default()
        };
        for member in ids.iter().filter_map(|&id| listing.members.get(id)) {
            if member.is_symlink {
                stats.links += 1;
            } else if member.is_dir {
                stats.folders += 1;
            } else if member.is_regular {
                stats.files += 1;
                stats.unpacked += u128::from(member.size);
                if let Some(packed) = &mut stats.packed {
                    *packed += u128::from(member.compressed_size);
                }
            } else {
                stats.other += 1;
            }
        }
        stats
    }
    pub(super) fn label(&self) -> String {
        let packed = self.packed.map_or_else(
            || "packed size unavailable".to_owned(),
            |bytes| format!("{bytes} packed bytes"),
        );
        format!(
            "Selected: {} files · {} directories · {} links · {} other | {} unpacked bytes · {packed}",
            self.files, self.folders, self.links, self.other, self.unpacked
        )
    }
}

#[derive(Default)]
pub(super) struct SelectionStatsCache {
    pub(super) ids: Vec<usize>,
    stats: SelectionStats,
    pub(super) initialized: bool,
}
impl SelectionStatsCache {
    pub(super) fn get(
        &mut self,
        listing: &crate::archive::ArchiveListing,
        selected: &std::collections::BTreeSet<usize>,
    ) -> &SelectionStats {
        if !self.initialized || !self.ids.iter().eq(selected.iter()) {
            self.ids = selected.iter().copied().collect();
            self.stats = SelectionStats::compute(listing, selected);
            self.initialized = true;
        }
        &self.stats
    }
}

impl SelectionMemory {
    pub(super) fn save(&mut self, selected: &std::collections::BTreeSet<usize>, real_count: usize) {
        self.0 = Some(
            selected
                .iter()
                .copied()
                .filter(|&id| id < real_count)
                .collect(),
        );
    }

    pub(super) fn restore(
        &self,
        selected: &mut std::collections::BTreeSet<usize>,
        real_count: usize,
    ) {
        if let Some(saved) = &self.0 {
            selected.clear();
            selected.extend(saved.iter().copied().filter(|&id| id < real_count));
        }
    }
}
