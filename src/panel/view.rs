use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{FacetSet, SortColumn, SortOrder};

const DEFAULT_VIEW_MEMORY_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewConfig {
    sort_col: SortColumn,
    sort_order: SortOrder,
    show_hidden: bool,
    folders_first: bool,
    natural_name_sort: bool,
    density: crate::density::Density,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            sort_col: SortColumn::Name,
            sort_order: SortOrder::Asc,
            show_hidden: false,
            folders_first: true,
            natural_name_sort: true,
            density: crate::density::Density::default(),
        }
    }
}

impl ViewConfig {
    pub(crate) fn sort_column(self) -> SortColumn {
        self.sort_col
    }

    pub(crate) fn sort_order(self) -> SortOrder {
        self.sort_order
    }

    pub(crate) fn show_hidden(self) -> bool {
        self.show_hidden
    }

    pub(crate) fn folders_first(self) -> bool {
        self.folders_first
    }

    pub(crate) fn natural_name_sort(self) -> bool {
        self.natural_name_sort
    }

    pub(crate) fn density(self) -> crate::density::Density {
        self.density
    }

    pub(crate) fn with_sort(mut self, column: SortColumn, order: SortOrder) -> Self {
        self.sort_col = column;
        self.sort_order = order;
        self
    }

    pub(crate) fn with_show_hidden(mut self, show_hidden: bool) -> Self {
        self.show_hidden = show_hidden;
        self
    }

    pub(crate) fn with_folders_first(mut self, folders_first: bool) -> Self {
        self.folders_first = folders_first;
        self
    }

    pub(crate) fn with_natural_name_sort(mut self, natural_name_sort: bool) -> Self {
        self.natural_name_sort = natural_name_sort;
        self
    }

    pub(crate) fn with_density(mut self, density: crate::density::Density) -> Self {
        self.density = density;
        self
    }
}

/// A directory's complete remembered view, including transient filters and
/// scroll focus. Keeping this in one value avoids restoring a half-old view.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ViewSettings {
    pub(super) config: ViewConfig,
    pub(super) search_query: String,
    pub(super) facets: FacetSet,
    pub(super) cursor_path: Option<PathBuf>,
    pub(super) scroll_anchor: usize,
}

pub(super) struct ViewState {
    config: ViewConfig,
    search_query: String,
    facets: FacetSet,
    remembered: HashMap<PathBuf, ViewSettings>,
    recency: VecDeque<PathBuf>,
    capacity: usize,
}

impl Default for ViewState {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_VIEW_MEMORY_CAPACITY)
    }
}

impl ViewState {
    fn with_capacity(capacity: usize) -> Self {
        Self::with_config_and_capacity(ViewConfig::default(), capacity)
    }

    pub(super) fn with_config(config: ViewConfig) -> Self {
        Self::with_config_and_capacity(config, DEFAULT_VIEW_MEMORY_CAPACITY)
    }

    fn with_config_and_capacity(config: ViewConfig, capacity: usize) -> Self {
        Self {
            config,
            search_query: String::new(),
            facets: FacetSet::default(),
            remembered: HashMap::new(),
            recency: VecDeque::new(),
            capacity,
        }
    }

    pub(super) fn config(&self) -> ViewConfig {
        self.config
    }

    pub(super) fn commit_config(&mut self, config: ViewConfig) {
        self.config = config;
    }

    pub(super) fn sort_col(&self) -> SortColumn {
        self.config.sort_col
    }

    pub(super) fn sort_order(&self) -> SortOrder {
        self.config.sort_order
    }

    pub(super) fn toggle_sort(&mut self, column: SortColumn) {
        if self.config.sort_col == column {
            self.config.sort_order = match self.config.sort_order {
                SortOrder::Asc => SortOrder::Desc,
                SortOrder::Desc => SortOrder::Asc,
            };
        } else {
            self.config.sort_col = column;
            self.config.sort_order = SortOrder::Asc;
        }
    }

    pub(super) fn reverse_sort(&mut self) {
        self.config.sort_order = match self.config.sort_order {
            SortOrder::Asc => SortOrder::Desc,
            SortOrder::Desc => SortOrder::Asc,
        };
    }

    pub(super) fn show_hidden(&self) -> bool {
        self.config.show_hidden
    }

    pub(super) fn toggle_folders_first(&mut self) {
        self.config.folders_first = !self.config.folders_first;
    }

    pub(super) fn toggle_natural_sort(&mut self) {
        self.config.natural_name_sort = !self.config.natural_name_sort;
    }

    pub(super) fn density(&self) -> crate::density::Density {
        self.config.density
    }

    pub(super) fn set_density(&mut self, density: crate::density::Density) {
        self.config.density = density;
    }

    pub(super) fn search_query(&self) -> &str {
        &self.search_query
    }

    pub(super) fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
    }

    pub(super) fn facets(&self) -> FacetSet {
        self.facets
    }

    pub(super) fn facets_mut(&mut self) -> &mut FacetSet {
        &mut self.facets
    }

    pub(super) fn clear_filters(&mut self) {
        self.search_query.clear();
        self.facets = FacetSet::default();
    }

    pub(super) fn remember(&mut self, path: &Path, settings: ViewSettings) {
        if self.capacity == 0 {
            return;
        }
        self.recency.retain(|candidate| candidate != path);
        self.recency.push_back(path.to_path_buf());
        self.remembered.insert(path.to_path_buf(), settings);
        while self.remembered.len() > self.capacity {
            if let Some(oldest) = self.recency.pop_front() {
                self.remembered.remove(&oldest);
            }
        }
    }

    pub(super) fn restore(&mut self, path: &Path) -> Option<ViewSettings> {
        let settings = self.remembered.get(path)?.clone();
        self.recency.retain(|candidate| candidate != path);
        self.recency.push_back(path.to_path_buf());
        self.config = settings.config;
        self.search_query = settings.search_query.clone();
        self.facets = settings.facets;
        Some(settings)
    }

    #[cfg(test)]
    fn remembered_len(&self) -> usize {
        self.remembered.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(label: &str) -> ViewSettings {
        ViewSettings {
            config: ViewConfig::default(),
            search_query: label.to_string(),
            facets: FacetSet::default(),
            cursor_path: None,
            scroll_anchor: 0,
        }
    }

    #[test]
    fn memory_is_bounded_and_recent_reads_refresh_recency() {
        let mut state = ViewState::with_capacity(2);
        state.remember(Path::new("/a"), settings("a"));
        state.remember(Path::new("/b"), settings("b"));
        assert_eq!(state.restore(Path::new("/a")).unwrap().search_query, "a");
        state.remember(Path::new("/c"), settings("c"));

        assert_eq!(state.remembered_len(), 2);
        assert!(state.restore(Path::new("/b")).is_none());
        assert_eq!(state.restore(Path::new("/a")).unwrap().search_query, "a");
        assert_eq!(state.restore(Path::new("/c")).unwrap().search_query, "c");
    }

    #[test]
    fn remembered_filter_is_restored_with_config() {
        let mut state = ViewState::default();
        let mut remembered = settings("report");
        remembered.facets.kind = Some(super::super::KindFacet::Docs);
        remembered.config = remembered.config.with_show_hidden(true);
        state.remember(Path::new("/docs"), remembered);

        let restored = state.restore(Path::new("/docs")).unwrap();
        assert_eq!(state.search_query(), "report");
        assert_eq!(state.facets().kind, Some(super::super::KindFacet::Docs));
        assert!(state.show_hidden());
        assert_eq!(restored.scroll_anchor, 0);
    }
}
