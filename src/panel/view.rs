use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use super::{FacetSet, SortColumn, SortOrder};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BrowserMode {
    #[default]
    List,
    Icons,
}

const DEFAULT_VIEW_MEMORY_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ViewConfig {
    sort_col: SortColumn,
    sort_order: SortOrder,
    show_hidden: bool,
    folders_first: bool,
    folders_by_name: bool,
    natural_name_sort: bool,
    case_sensitive_name_sort: bool,
    density: crate::density::Density,
    mode: BrowserMode,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            sort_col: SortColumn::Name,
            sort_order: SortOrder::Asc,
            show_hidden: false,
            folders_first: true,
            folders_by_name: false,
            natural_name_sort: true,
            case_sensitive_name_sort: false,
            density: crate::density::Density::default(),
            mode: BrowserMode::List,
        }
    }
}

impl ViewConfig {
    pub(crate) const fn mode(self) -> BrowserMode {
        self.mode
    }
    pub(crate) const fn with_mode(mut self, mode: BrowserMode) -> Self {
        self.mode = mode;
        self
    }

    pub(crate) const fn sort_column(self) -> SortColumn {
        self.sort_col
    }

    pub(crate) const fn sort_order(self) -> SortOrder {
        self.sort_order
    }

    pub(crate) const fn show_hidden(self) -> bool {
        self.show_hidden
    }

    pub(crate) const fn folders_first(self) -> bool {
        self.folders_first
    }

    pub(crate) const fn folders_by_name(self) -> bool {
        self.folders_by_name
    }
    pub(crate) const fn with_folders_by_name(mut self, enabled: bool) -> Self {
        self.folders_by_name = enabled;
        if enabled {
            self.folders_first = true;
        }
        self
    }

    pub(crate) const fn natural_name_sort(self) -> bool {
        self.natural_name_sort
    }

    pub(crate) const fn case_sensitive_name_sort(self) -> bool {
        self.case_sensitive_name_sort
    }
    pub(crate) const fn with_case_sensitive_name_sort(mut self, enabled: bool) -> Self {
        self.case_sensitive_name_sort = enabled;
        self
    }

    pub(crate) const fn density(self) -> crate::density::Density {
        self.density
    }

    pub(crate) const fn with_sort(mut self, column: SortColumn, order: SortOrder) -> Self {
        self.sort_col = column;
        self.sort_order = order;
        self
    }

    pub(crate) const fn with_show_hidden(mut self, show_hidden: bool) -> Self {
        self.show_hidden = show_hidden;
        self
    }

    pub(crate) const fn with_folders_first(mut self, folders_first: bool) -> Self {
        self.folders_first = folders_first;
        if !folders_first {
            self.folders_by_name = false;
        }
        self
    }

    pub(crate) const fn with_natural_name_sort(mut self, natural_name_sort: bool) -> Self {
        self.natural_name_sort = natural_name_sort;
        self
    }

    pub(crate) const fn with_density(mut self, density: crate::density::Density) -> Self {
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

    pub(super) const fn config(&self) -> ViewConfig {
        self.config
    }

    pub(super) const fn commit_config(&mut self, config: ViewConfig) {
        self.config = config;
    }

    pub(super) const fn sort_col(&self) -> SortColumn {
        self.config.sort_col
    }

    pub(super) const fn sort_order(&self) -> SortOrder {
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

    pub(super) const fn reverse_sort(&mut self) {
        self.config.sort_order = match self.config.sort_order {
            SortOrder::Asc => SortOrder::Desc,
            SortOrder::Desc => SortOrder::Asc,
        };
    }

    pub(super) const fn show_hidden(&self) -> bool {
        self.config.show_hidden
    }

    pub(super) const fn toggle_folders_first(&mut self) {
        self.config = self.config.with_folders_first(!self.config.folders_first);
    }

    pub(super) const fn toggle_case_sensitive_name_sort(&mut self) {
        self.config.case_sensitive_name_sort = !self.config.case_sensitive_name_sort;
    }

    pub(super) const fn toggle_folders_by_name(&mut self) {
        self.config = self
            .config
            .with_folders_by_name(!self.config.folders_by_name);
    }

    pub(super) const fn toggle_natural_sort(&mut self) {
        self.config.natural_name_sort = !self.config.natural_name_sort;
    }

    pub(super) const fn density(&self) -> crate::density::Density {
        self.config.density
    }

    pub(super) const fn set_density(&mut self, density: crate::density::Density) {
        self.config.density = density;
    }

    pub(super) fn search_query(&self) -> &str {
        &self.search_query
    }

    pub(super) fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
    }

    pub(super) const fn facets(&self) -> FacetSet {
        self.facets
    }

    pub(super) const fn facets_mut(&mut self) -> &mut FacetSet {
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

    #[test]
    fn switching_column_from_a_descending_sort_starts_ascending() {
        let mut state = ViewState::default();
        state.toggle_sort(SortColumn::Size);
        state.toggle_sort(SortColumn::Size);
        assert_eq!(
            (state.sort_col(), state.sort_order()),
            (SortColumn::Size, SortOrder::Desc)
        );

        state.toggle_sort(SortColumn::Name);
        assert_eq!(
            (state.sort_col(), state.sort_order()),
            (SortColumn::Name, SortOrder::Asc)
        );
        state.toggle_sort(SortColumn::Name);
        assert_eq!(state.sort_order(), SortOrder::Desc);
    }

    #[test]
    fn remembering_a_path_again_refreshes_its_recency_and_settings() {
        let mut state = ViewState::with_capacity(2);
        state.remember(Path::new("/a"), settings("a-old"));
        state.remember(Path::new("/b"), settings("b"));
        state.remember(Path::new("/a"), settings("a-new"));
        state.remember(Path::new("/c"), settings("c"));

        assert_eq!(state.remembered_len(), 2);
        assert!(state.restore(Path::new("/b")).is_none(), "b was oldest");
        assert_eq!(
            state.restore(Path::new("/a")).unwrap().search_query,
            "a-new"
        );
        assert_eq!(state.restore(Path::new("/c")).unwrap().search_query, "c");
    }
}
