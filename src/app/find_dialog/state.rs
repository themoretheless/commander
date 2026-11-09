//! Field transitions the find dialog applies after its UI pass.

use super::*;

/// Field transitions the find dialog applies after its UI pass. They are
/// plain state edits, kept out of the frame code so they can be tested.
impl FindState {
    pub(super) fn visible_result_count(&self) -> usize {
        self.results
            .iter()
            .filter(|hit| !self.hidden_results.contains(&hit.location))
            .count()
    }

    pub(super) fn set_time_pivot(&mut self, pivot: crate::search::TimePivot, clock: SystemTime) {
        if self.time_pivot == pivot {
            return;
        }
        let highlighted = self
            .highlighted_result
            .and_then(|row| self.visible_result_order(clock).get(row).copied())
            .map(|index| self.results[index].location.clone());
        self.time_pivot = pivot;
        self.highlighted_result = highlighted.and_then(|location| {
            self.visible_result_order(clock)
                .iter()
                .position(|&index| self.results[index].location == location)
        });
        self.highlight_scroll_pending = self.highlighted_result.is_some();
    }

    pub(super) fn set_result_sort(&mut self, sort: crate::search_order::Sort, clock: SystemTime) {
        if self.result_sort == sort {
            return;
        }
        let highlighted = self
            .highlighted_result
            .and_then(|row| self.visible_result_order(clock).get(row).copied())
            .map(|index| self.results[index].location.clone());
        self.result_sort = sort;
        self.highlighted_result = highlighted.and_then(|location| {
            self.visible_result_order(clock)
                .iter()
                .position(|&index| self.results[index].location == location)
        });
        self.highlight_scroll_pending = self.highlighted_result.is_some();
    }
    pub(super) fn visible_result_order(&self, clock: SystemTime) -> Vec<usize> {
        let mut order =
            crate::search_order::order(&self.results, self.time_pivot, self.result_sort, clock);
        order.retain(|&index| !self.hidden_results.contains(&self.results[index].location));
        order
    }

    pub(super) fn visible_selected_result_count(&self) -> usize {
        self.results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && self.selected_results.contains(&hit.location)
            })
            .count()
    }

    pub(super) fn selected_result_tab_targets(&self, clock: SystemTime) -> Vec<PathBuf> {
        self.visible_result_order(clock)
            .into_iter()
            .filter(|&index| {
                self.selected_results
                    .contains(&self.results[index].location)
            })
            .map(|index| self.results[index].reveal_path().to_path_buf())
            .collect()
    }

    pub(super) fn prepare_result_copy(
        &mut self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Option<(String, usize)> {
        self.result_action_error = None;
        if matches!(
            self.result_copy_format,
            FindCopyFormat::DetailsJson | FindCopyFormat::RelativePaths
        ) {
            let result = if self.result_copy_format == FindCopyFormat::RelativePaths {
                self.result_relative_paths_clipboard(clock, selected_only)
            } else {
                self.result_json(clock, selected_only)
            };
            match result {
                Ok(value) => value,
                Err(error) => {
                    self.result_action_error = Some(error);
                    None
                }
            }
        } else if selected_only {
            self.selected_result_paths_clipboard(clock)
        } else {
            self.visible_result_paths_clipboard(clock)
        }
    }
    fn result_relative_paths_clipboard(
        &self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Result<Option<(String, usize)>, String> {
        let hits: Vec<_> = self
            .visible_result_order(clock)
            .into_iter()
            .filter(|&i| {
                !selected_only || self.selected_results.contains(&self.results[i].location)
            })
            .map(|i| &self.results[i])
            .collect();
        if hits.is_empty() {
            return Ok(None);
        }
        let Some((_, _, root)) = &self.last_query else {
            return Err("Relative paths need results bound to an applied search root".into());
        };
        let relative = |path: &std::path::Path| -> Result<String, String> {
            let part = path
                .strip_prefix(root)
                .map_err(|_| "A result is outside the applied search root".to_string())?;
            if !root.is_absolute()
                || part.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                return Err(
                    "Relative paths need an absolute root and contained lexical paths".into(),
                );
            }
            if part.as_os_str().is_empty() {
                return Ok(".".into());
            }
            part.to_str()
                .map(str::to_owned)
                .ok_or_else(|| "A relative path cannot be represented as UTF-8 text".into())
        };
        let lines: Result<Vec<_>, String> = hits
            .iter()
            .map(|hit| match &hit.location {
                crate::search::SearchLocation::Native(path) => relative(path),
                crate::search::SearchLocation::ArchiveMember {
                    archive, member, ..
                } => {
                    if member.is_absolute()
                        || member.components().any(|c| {
                            matches!(
                                c,
                                std::path::Component::ParentDir
                                    | std::path::Component::RootDir
                                    | std::path::Component::Prefix(_)
                            )
                        })
                    {
                        return Err("An archive member has no contained relative path".into());
                    }
                    let archive = relative(archive)?;
                    let member = member.to_str().ok_or_else(|| {
                        "An archive member cannot be represented as UTF-8 text".to_string()
                    })?;
                    Ok(format!("{archive}!/{member}"))
                }
            })
            .collect();
        let paths: Vec<_> = lines?.into_iter().map(PathBuf::from).collect();
        Ok(Some((
            crate::clipboard::format(&paths, crate::clipboard::PathStyle::FullPath, None),
            hits.len(),
        )))
    }

    pub(super) fn result_json(
        &self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Result<Option<(String, usize)>, String> {
        let Some((expression, mode, root)) = &self.last_query else {
            return Err("Search JSON needs results bound to an applied query".into());
        };
        let hits: Vec<_> = self
            .visible_result_order(clock)
            .into_iter()
            .filter(|&i| {
                !selected_only || self.selected_results.contains(&self.results[i].location)
            })
            .map(|i| &self.results[i])
            .collect();
        if selected_only && hits.is_empty() {
            return Ok(None);
        }
        let draft_differs = self.root != *root
            || self.mode != *mode
            || self
                .build_query()
                .map_or(true, |query| query.to_expression() != *expression);
        let context = crate::search_report::Context {
            expression,
            mode: *mode,
            root,
            generation: self.generation,
            source: &self.search_source,
            completed: self.result_complete,
            running: self.searching(),
            pending_rerun: self.pending_rerun,
            draft_differs,
            scanned: self.scanned,
            matched: self.matched,
            skipped: self.content_skipped,
            truncated: self.truncated,
            archive_error: self.archive_error.as_deref(),
            received_rows: self.results.len(),
            visible_rows: self.visible_result_count(),
            visible_selected_rows: self.visible_selected_result_count(),
            pivot: self.time_pivot,
            result_sort: self.result_sort,
            selected_only,
            ordered_at_utc: crate::listing_export::iso_8601_utc_precision(
                clock,
                chrono::SecondsFormat::AutoSi,
            ),
        };
        let count = hits.len();
        crate::search_report::json(&context, &hits).map(|text| Some((text, count)))
    }

    pub(super) fn visible_result_paths_clipboard(
        &self,
        clock: SystemTime,
    ) -> Option<(String, usize)> {
        self.result_paths_clipboard(clock, false)
    }

    pub(super) fn selected_result_paths_clipboard(
        &self,
        clock: SystemTime,
    ) -> Option<(String, usize)> {
        self.result_paths_clipboard(clock, true)
    }

    fn result_paths_clipboard(
        &self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Option<(String, usize)> {
        let hits: Vec<_> = self
            .visible_result_order(clock)
            .into_iter()
            .filter(|&index| {
                !selected_only
                    || self
                        .selected_results
                        .contains(&self.results[index].location)
            })
            .map(|index| &self.results[index])
            .collect();
        if hits.is_empty() {
            return None;
        }
        let text = match self.result_copy_format {
            FindCopyFormat::RelativePaths => {
                return self
                    .result_relative_paths_clipboard(clock, selected_only)
                    .ok()
                    .flatten();
            }
            FindCopyFormat::DetailsJson => {
                return self.result_json(clock, selected_only).ok().flatten();
            }
            FindCopyFormat::ParentFolders => {
                let paths = hits
                    .iter()
                    .map(|hit| std::path::absolute(hit.reveal_path()).ok())
                    .collect::<Option<Vec<_>>>()?;
                let paths: Vec<_> = paths
                    .into_iter()
                    .filter(|path| path.parent().is_some())
                    .collect();
                if paths.is_empty() {
                    return None;
                }
                let text =
                    crate::clipboard::format(&paths, crate::clipboard::PathStyle::ParentPath, None);
                let count = text.lines().count();
                return Some((text, count));
            }
            FindCopyFormat::FileUrls => {
                let paths = hits
                    .iter()
                    .map(|hit| {
                        let path = hit.reveal_path();
                        if path.is_absolute() {
                            Some(path.to_path_buf())
                        } else {
                            std::path::absolute(path).ok()
                        }
                    })
                    .collect::<Option<Vec<_>>>()?;
                crate::clipboard::format(&paths, crate::clipboard::PathStyle::FileUrl, None)
            }
            FindCopyFormat::DetailsCsv => crate::listing_export::search_csv(
                &hits.iter().map(|hit| &hit.entry).collect::<Vec<_>>(),
            ),
            FindCopyFormat::ShellArguments => hits
                .iter()
                .map(|hit| crate::clipboard::shell_quote(&hit.reveal_path().to_string_lossy()))
                .collect::<Vec<_>>()
                .join(" "),
            FindCopyFormat::Paths
            | FindCopyFormat::Names
            | FindCopyFormat::NamesWithoutExtension => {
                let paths: Vec<_> = hits.iter().map(|hit| hit.entry.path.clone()).collect();
                let style = if self.result_copy_format == FindCopyFormat::Names {
                    crate::clipboard::PathStyle::NameOnly
                } else if self.result_copy_format == FindCopyFormat::NamesWithoutExtension {
                    crate::clipboard::PathStyle::NameWithoutExtension
                } else {
                    crate::clipboard::PathStyle::FullPath
                };
                crate::clipboard::format(&paths, style, None)
            }
        };
        Some((text, hits.len()))
    }

    pub(super) fn select_all_visible_results(&mut self) {
        self.selected_results = self
            .results
            .iter()
            .filter(|hit| !self.hidden_results.contains(&hit.location))
            .map(|hit| hit.location.clone())
            .collect();
        self.result_selection_anchor = None;
    }

    pub(super) fn invert_visible_result_selection(&mut self) {
        self.selected_results = self
            .results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && !self.selected_results.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        self.result_selection_anchor = None;
    }

    pub(super) fn results_match_current_root(&self) -> bool {
        self.last_query
            .as_ref()
            .is_none_or(|(_, _, root)| root == &self.root)
    }

    pub(super) fn remember_result_selection(&mut self) -> bool {
        if !self.results_match_current_root() {
            return false;
        }
        let locations: std::collections::HashSet<_> = self
            .results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && self.selected_results.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        if locations.is_empty() {
            return false;
        }
        self.remembered_result_selection = Some(FindResultSelection {
            root: self.root.clone(),
            locations,
        });
        true
    }

    pub(super) fn restorable_result_selection_count(&self) -> usize {
        if !self.results_match_current_root() {
            return 0;
        }
        let Some(remembered) = self
            .remembered_result_selection
            .as_ref()
            .filter(|saved| saved.root == self.root)
        else {
            return 0;
        };
        self.results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && remembered.locations.contains(&hit.location)
            })
            .count()
    }

    pub(super) fn restore_result_selection(&mut self) -> bool {
        if !self.results_match_current_root() {
            return false;
        }
        let Some(remembered) = self
            .remembered_result_selection
            .as_ref()
            .filter(|saved| saved.root == self.root)
        else {
            return false;
        };
        let locations: std::collections::HashSet<_> = self
            .results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && remembered.locations.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        if locations.is_empty() {
            return false;
        }
        self.selected_results = locations;
        self.result_selection_anchor = None;
        true
    }

    pub(super) fn clear_result_selection(&mut self) {
        self.selected_results.clear();
        self.result_selection_anchor = None;
    }

    pub(super) fn select_only_result(
        &mut self,
        location: crate::search::SearchLocation,
        clock: SystemTime,
    ) -> bool {
        let order = self.visible_result_order(clock);
        let Some(row) = order
            .iter()
            .position(|&index| self.results[index].location == location)
        else {
            return false;
        };
        self.selected_results.clear();
        self.selected_results.insert(location.clone());
        self.result_selection_anchor = Some(location);
        self.highlighted_result = Some(row);
        self.highlight_scroll_pending = false;
        true
    }

    pub(super) fn select_result(
        &mut self,
        location: crate::search::SearchLocation,
        range: bool,
        clock: SystemTime,
    ) {
        let order = self.visible_result_order(clock);
        let Some(target) = order
            .iter()
            .position(|&index| self.results[index].location == location)
        else {
            return;
        };
        if range
            && let Some(anchor) = self.result_selection_anchor.as_ref().and_then(|anchor| {
                order
                    .iter()
                    .position(|&index| &self.results[index].location == anchor)
            })
        {
            for &index in &order[anchor.min(target)..=anchor.max(target)] {
                self.selected_results
                    .insert(self.results[index].location.clone());
            }
        } else {
            if !self.selected_results.remove(&location) {
                self.selected_results.insert(location.clone());
            }
            self.result_selection_anchor = Some(location);
        }
    }

    pub(super) fn highlighted_result_target(&self, clock: SystemTime) -> Option<PathBuf> {
        let index = *self
            .visible_result_order(clock)
            .get(self.highlighted_result?)?;
        self.results
            .get(index)
            .map(|hit| hit.reveal_path().to_path_buf())
    }

    pub(super) fn hide_selected_results(&mut self) -> usize {
        let targets: std::collections::HashSet<_> = self
            .results
            .iter()
            .filter(|hit| {
                self.selected_results.contains(&hit.location)
                    && !self.hidden_results.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        let count = targets.len();
        if count == 0 {
            return 0;
        }
        self.selected_results
            .retain(|location| !targets.contains(location));
        if self
            .result_selection_anchor
            .as_ref()
            .is_some_and(|anchor| targets.contains(anchor))
        {
            self.result_selection_anchor = None;
        }
        if self
            .explanation_open
            .as_ref()
            .is_some_and(|key| targets.contains(&key.1))
        {
            self.explanation_open = None;
        }
        self.hidden_results.extend(targets);
        self.highlighted_result = None;
        self.highlight_scroll_pending = false;
        count
    }

    pub(super) fn hide_result(&mut self, location: crate::search::SearchLocation) {
        if !self.results.iter().any(|hit| hit.location == location) {
            return;
        }
        if self
            .explanation_open
            .as_ref()
            .is_some_and(|key| key.1 == location)
        {
            self.explanation_open = None;
        }
        self.selected_results.remove(&location);
        if self.result_selection_anchor.as_ref() == Some(&location) {
            self.result_selection_anchor = None;
        }
        self.hidden_results.insert(location);
        self.highlighted_result = None;
        self.highlight_scroll_pending = false;
    }

    pub(super) fn restore_hidden_results(&mut self) {
        self.hidden_results.clear();
        self.highlighted_result = None;
        self.highlight_scroll_pending = false;
    }

    /// The exclusion editor's lines as paths, blank lines dropped.
    pub(super) fn exclusion_paths(&self) -> Vec<PathBuf> {
        self.index_exclusions
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    /// Load a history entry's query and root, with `exclusions` formatted
    /// for that root, and close the history list.
    pub(super) fn replay(&mut self, entry: crate::search::QueryHistoryEntry, exclusions: String) {
        self.expression = entry.expression;
        self.mode = entry.mode;
        self.root = entry.root;
        self.index_exclusions = exclusions;
        self.index_rerun_after_build = false;
        self.history_open = false;
    }

    /// Search under `root` instead, with `exclusions` formatted for it.
    pub(super) fn change_root(&mut self, root: PathBuf, exclusions: String) {
        self.root = root;
        self.index_exclusions = exclusions;
        self.index_rerun_after_build = false;
    }

    /// Drop the query's `index`th predicate chip; false when the query does
    /// not parse or has no such predicate.
    pub(super) fn remove_predicate(&mut self, index: usize) -> bool {
        let Ok(mut query) = self.build_query() else {
            return false;
        };
        if index >= query.predicates.len() {
            return false;
        }
        query.predicates.remove(index);
        self.expression = query.to_expression();
        true
    }
}
