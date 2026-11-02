//! Review projection and explicit direction edits; original plan identities are retained.
use crate::sync::{SyncAction, SyncDirection, SyncStatus};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    All,
    Planned,
    Skipped,
    Conflicts,
    OneSided,
    PairedFiles,
    MatchingMetadata,
    Folders,
    SameContents,
    DifferentContents,
    UnverifiedContents,
}
impl Scope {
    pub const ALL: [Self; 11] = [
        Self::All,
        Self::Planned,
        Self::Skipped,
        Self::Conflicts,
        Self::OneSided,
        Self::PairedFiles,
        Self::MatchingMetadata,
        Self::Folders,
        Self::SameContents,
        Self::DifferentContents,
        Self::UnverifiedContents,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All rows",
            Self::Planned => "Planned",
            Self::Skipped => "Skipped",
            Self::Conflicts => "Conflicts",
            Self::OneSided => "One-sided",
            Self::PairedFiles => "Paired files",
            Self::MatchingMetadata => "Matching metadata",
            Self::Folders => "Folder pairs",
            Self::SameContents => "Same contents",
            Self::DifferentContents => "Different contents",
            Self::UnverifiedContents => "Unverified contents",
        }
    }
    fn accepts(self, row: &SyncAction) -> bool {
        match self {
            Self::All => true,
            Self::Planned => row.direction != SyncDirection::Skip,
            Self::Skipped => row.direction == SyncDirection::Skip,
            Self::Conflicts => {
                row.content.error().is_some()
                    || matches!(
                        row.status,
                        SyncStatus::Differing | SyncStatus::TypeConflict | SyncStatus::CaseConflict
                    )
            }
            Self::OneSided => matches!(row.status, SyncStatus::LeftOnly | SyncStatus::RightOnly),
            Self::PairedFiles => matches!(
                row.status,
                SyncStatus::LeftNewer
                    | SyncStatus::RightNewer
                    | SyncStatus::Differing
                    | SyncStatus::Identical
            ),
            Self::MatchingMetadata => {
                row.status == SyncStatus::Identical
                    && row.content == crate::sync_content::Comparison::Unchecked
            }
            Self::SameContents => row.content == crate::sync_content::Comparison::Equal,
            Self::DifferentContents => row.content == crate::sync_content::Comparison::Different,
            Self::UnverifiedContents => row.content.error().is_some(),
            Self::Folders => row.status == SyncStatus::DirectoryPair,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Skip,
    Defaults,
    Reverse,
}
impl Edit {
    pub const ALL: [Self; 3] = [Self::Skip, Self::Defaults, Self::Reverse];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Skip => "Skip shown",
            Self::Defaults => "Restore shown defaults",
            Self::Reverse => "Reverse shown copies",
        }
    }
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Skip => "Skip every currently shown row. Hidden rows keep their directions.",
            Self::Defaults => {
                "Reapply the current policy to shown rows' retained statuses. Does not rescan files."
            }
            Self::Reverse => {
                "Reverse shown planned copies where the opposite source exists. Skipped and unavailable directions stay unchanged."
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    PlanOrder,
    Name,
    Status,
    Direction,
}
impl Sort {
    pub const ALL: [Self; 4] = [Self::PlanOrder, Self::Name, Self::Status, Self::Direction];
    pub const fn label(self) -> &'static str {
        match self {
            Self::PlanOrder => "Plan order",
            Self::Name => "Name",
            Self::Status => "Status",
            Self::Direction => "Direction",
        }
    }
    pub const fn key(self) -> &'static str {
        match self {
            Self::PlanOrder => "plan_order",
            Self::Name => "name",
            Self::Status => "status",
            Self::Direction => "direction",
        }
    }
}
fn status_rank(row: &SyncAction) -> u8 {
    if row.content.error().is_some() {
        return 0;
    }
    if row.content == crate::sync_content::Comparison::Equal {
        return 9;
    }
    match row.status {
        SyncStatus::TypeConflict => 0,
        SyncStatus::CaseConflict => 1,
        SyncStatus::Differing => 2,
        SyncStatus::LeftOnly => 3,
        SyncStatus::RightOnly => 4,
        SyncStatus::LeftNewer => 5,
        SyncStatus::RightNewer => 6,
        SyncStatus::DirectoryPair => 7,
        SyncStatus::Identical => 8,
    }
}
fn direction_rank(direction: SyncDirection) -> u8 {
    match direction {
        SyncDirection::ToLeft => 0,
        SyncDirection::ToRight => 1,
        SyncDirection::Skip => 2,
    }
}
struct Key {
    query: String,
    masks: bool,
    scope: Scope,
    sort: Sort,
    descending: bool,
    actions: Vec<SyncAction>,
}
#[derive(Default)]
pub struct Review {
    pub query: String,
    pub masks: bool,
    pub scope: Scope,
    pub sort: Sort,
    pub descending: bool,
    pub indices: Vec<usize>,
    pub hidden_planned: usize,
    key: Option<Key>,
}
impl Review {
    pub fn refresh(&mut self, actions: &[SyncAction]) -> bool {
        if self.key.as_ref().is_some_and(|key| {
            key.query == self.query
                && key.masks == self.masks
                && key.scope == self.scope
                && key.sort == self.sort
                && key.descending == self.descending
                && key.actions == actions
        }) {
            return false;
        }
        let valid = self.query.chars().take(129).count() <= 128;
        let query = self.query.to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        let terms = crate::panel::parse_mask(&self.query);
        self.indices.clear();
        self.hidden_planned = 0;
        for (index, row) in actions.iter().enumerate() {
            let name = row.name.to_lowercase();
            let matches = if self.masks {
                let has_includes = terms.iter().any(|(_, exclude)| !exclude);
                (!has_includes
                    || terms.iter().any(|(pattern, exclude)| {
                        !exclude && crate::panel::glob_match(pattern, &name)
                    }))
                    && !terms.iter().any(|(pattern, exclude)| {
                        *exclude && crate::panel::glob_match(pattern, &name)
                    })
            } else {
                words.iter().all(|word| name.contains(word))
            };
            if valid && self.scope.accepts(row) && matches {
                self.indices.push(index);
            } else if row.direction != SyncDirection::Skip {
                self.hidden_planned += 1;
            }
        }
        let mut ranked: Vec<_> = self
            .indices
            .iter()
            .map(|&index| (index, actions[index].name.to_lowercase()))
            .collect();
        ranked.sort_by(|(a, an), (b, bn)| {
            let primary = match self.sort {
                Sort::PlanOrder => a.cmp(b),
                Sort::Name => crate::natural_sort::compare(an, bn),
                Sort::Status => status_rank(&actions[*a]).cmp(&status_rank(&actions[*b])),
                Sort::Direction => direction_rank(actions[*a].direction)
                    .cmp(&direction_rank(actions[*b].direction)),
            };
            let primary = if self.descending {
                primary.reverse()
            } else {
                primary
            };
            primary
                .then_with(|| crate::natural_sort::compare(an, bn))
                .then_with(|| actions[*a].name.cmp(&actions[*b].name))
                .then_with(|| a.cmp(b))
        });
        self.indices = ranked.into_iter().map(|(index, _)| index).collect();
        self.key = Some(Key {
            query: self.query.clone(),
            masks: self.masks,
            scope: self.scope,
            sort: self.sort,
            descending: self.descending,
            actions: actions.to_vec(),
        });
        true
    }
    /// Capture targets once, then edit them. Rows leaving Planned/Skipped scope
    /// during the operation cannot cause subsequent original rows to be missed.
    pub fn edit_shown(
        &mut self,
        actions: &mut [SyncAction],
        policy: crate::sync::SyncPolicy,
        edit: Edit,
    ) -> usize {
        self.key = None;
        self.refresh(actions);
        let mut changed = 0;
        for &index in &self.indices {
            let row = &mut actions[index];
            let target = match edit {
                Edit::Skip => SyncDirection::Skip,
                Edit::Defaults => row.default_direction(policy),
                Edit::Reverse => {
                    if matches!(
                        row.status,
                        SyncStatus::DirectoryPair
                            | SyncStatus::TypeConflict
                            | SyncStatus::CaseConflict
                    ) {
                        continue;
                    }
                    match row.direction {
                        SyncDirection::ToLeft => SyncDirection::ToRight,
                        SyncDirection::ToRight => SyncDirection::ToLeft,
                        SyncDirection::Skip => continue,
                    }
                }
            };
            if row.direction != target && row.allows(target) {
                row.direction = target;
                changed += 1;
            }
        }
        self.refresh(actions);
        changed
    }
    pub fn show(&mut self, ui: &mut egui::Ui, actions: &[SyncAction]) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Find plan rows");
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .desired_width(180.0)
                    .char_limit(129)
                    .hint_text(if self.masks { "*.jpg;*.png;!thumb*" } else { "Name contains all words" }),
            );
            let masks = ui.checkbox(&mut self.masks, "Name masks")
                .on_hover_text("Whole-name * and ? masks, separated by commas or semicolons. Includes are OR; !exclusions win. Case insensitive. View only; use Skip shown to exclude matching actions.");
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("sync-review", "masks")), masks.rect));
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = masks;
            let reset = ui.button("Reset view");
            if reset.clicked() {
                self.query.clear();
                self.masks = false;
                self.scope = Scope::All;
                self.sort = Sort::PlanOrder;
                self.descending = false;
            }
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new(("sync-review", "query")), search.rect);
                data.insert_temp(egui::Id::new(("sync-review", "reset")), reset.rect);
            });
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = search;
        });
        ui.horizontal_wrapped(|ui| {
            for scope in Scope::ALL {
                if matches!(
                    scope,
                    Scope::SameContents | Scope::DifferentContents | Scope::UnverifiedContents
                ) && !actions
                    .iter()
                    .any(|a| a.content != crate::sync_content::Comparison::Unchecked)
                {
                    continue;
                }
                let response = ui.selectable_value(&mut self.scope, scope, scope.label());
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new(("sync-review", scope.label())), response.rect)
                });
                #[cfg(not(any(test, feature = "visual-qa")))]
                let _ = response;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Sort rows");
            for sort in Sort::ALL {
                let response=ui.selectable_value(&mut self.sort,sort,sort.label()).on_hover_text(match sort {Sort::Status=>"Conflicts, one-sided rows, newer files, folder pairs, then metadata matches. Names break ties.",Sort::Direction=>"To left, to right, then skipped. Names break ties.",_=>"Changes the review order only; the original plan stays unchanged."});
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new(("sync-sort",sort.key())),response.rect));
                #[cfg(not(any(test,feature="visual-qa")))]
                let _=response;
            }
            let descending=ui.checkbox(&mut self.descending,"Descending");
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new(("sync-sort","descending")),descending.rect));
            #[cfg(not(any(test,feature="visual-qa")))]
            let _=descending;
        });
        self.refresh(actions);
        ui.small(format!(
            "{} / {} rows shown · {} hidden planned actions",
            self.indices.len(),
            actions.len(),
            self.hidden_planned
        ));
        ui.small("View filter only: Sync and All direction buttons include hidden rows.");
        if self.query.chars().take(129).count() > 128 {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Use at most 128 search characters",
            );
        } else if self.indices.is_empty() && !actions.is_empty() {
            ui.small("No matching rows. Reset view to show the full plan.");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rows() -> Vec<SyncAction> {
        use crate::sync::test_action;
        vec![
            test_action("text.txt", SyncStatus::Identical, SyncDirection::Skip),
            test_action(
                "Архив FINAL.zip",
                SyncStatus::LeftOnly,
                SyncDirection::ToRight,
            ),
            test_action("collision", SyncStatus::TypeConflict, SyncDirection::Skip),
            test_action("other.zip", SyncStatus::RightOnly, SyncDirection::ToLeft),
            test_action("folder", SyncStatus::DirectoryPair, SyncDirection::Skip),
        ]
    }
    #[test]
    fn name_masks_preserve_plan_until_explicit_shown_edit_and_cache_mode_changes() {
        let mut actions = rows();
        let original = actions.clone();
        let mut view = Review {
            query: "*.ZIP;!other*".into(),
            masks: true,
            ..Default::default()
        };
        assert!(view.refresh(&actions));
        assert_eq!(view.indices, vec![1]);
        assert_eq!(actions, original);
        assert_eq!(view.hidden_planned, 1);
        assert!(!view.refresh(&actions));
        view.masks = false;
        assert!(view.refresh(&actions));
        assert!(view.indices.is_empty());
        view.masks = true;
        assert_eq!(
            view.edit_shown(&mut actions, crate::sync::SyncPolicy::TwoWay, Edit::Skip),
            1
        );
        assert_eq!(actions[1].direction, SyncDirection::Skip);
        for i in [0, 2, 3, 4] {
            assert_eq!(actions[i], original[i]);
        }
        view.query = "!*.zip".into();
        view.refresh(&actions);
        assert_eq!(view.indices, vec![0, 2, 4]);
        view.query = "*.txt;oth?r.zip".into();
        view.refresh(&actions);
        assert_eq!(view.indices, vec![0, 3]);
        view.query.clear();
        view.refresh(&actions);
        assert_eq!(view.indices.len(), actions.len());
    }
    #[test]
    fn content_scopes_and_defaults_distinguish_verified_rows_from_metadata_and_unknowns() {
        use crate::sync::{SyncPolicy, test_action};
        use crate::sync_content::Comparison;
        let mut rows = vec![
            test_action("meta", SyncStatus::Identical, SyncDirection::Skip),
            test_action("different", SyncStatus::Identical, SyncDirection::Skip),
            test_action("equal", SyncStatus::LeftNewer, SyncDirection::ToRight),
            test_action("unknown", SyncStatus::Identical, SyncDirection::Skip),
            test_action("only", SyncStatus::LeftOnly, SyncDirection::ToRight),
        ];
        rows[1].set_content(Comparison::Different, SyncPolicy::TwoWay);
        rows[2].set_content(Comparison::Equal, SyncPolicy::TwoWay);
        rows[3].set_content(
            Comparison::Unavailable("unreadable".into()),
            SyncPolicy::TwoWay,
        );
        let mut review = Review::default();
        for (scope, expected) in [
            (Scope::MatchingMetadata, vec![0]),
            (Scope::SameContents, vec![2]),
            (Scope::DifferentContents, vec![1]),
            (Scope::UnverifiedContents, vec![3]),
            (Scope::Conflicts, vec![1, 3]),
        ] {
            review.scope = scope;
            review.refresh(&rows);
            assert_eq!(review.indices, expected);
        }
        review.scope = Scope::All;
        review.sort = Sort::Status;
        review.refresh(&rows);
        assert_eq!(review.indices, vec![3, 1, 4, 0, 2]);
        rows[2].direction = SyncDirection::ToRight;
        review.scope = Scope::SameContents;
        review.refresh(&rows);
        review.edit_shown(&mut rows, SyncPolicy::TwoWay, Edit::Defaults);
        assert_eq!(rows[2].direction, SyncDirection::Skip);
    }

    #[test]
    fn sort_is_natural_stable_and_cached_without_reordering_the_plan() {
        use crate::sync::test_action;
        let actions = vec![
            test_action("file10", SyncStatus::LeftOnly, SyncDirection::ToRight),
            test_action("file2", SyncStatus::LeftOnly, SyncDirection::ToRight),
            test_action("FILE2", SyncStatus::LeftOnly, SyncDirection::ToRight),
        ];
        let original = actions.clone();
        let mut view = Review {
            sort: Sort::Name,
            ..Default::default()
        };
        assert!(view.refresh(&actions));
        assert_eq!(view.indices, [2, 1, 0]);
        assert!(!view.refresh(&actions));
        view.descending = true;
        assert!(view.refresh(&actions));
        assert_eq!(view.indices, [0, 2, 1]);
        view.sort = Sort::PlanOrder;
        assert!(view.refresh(&actions));
        assert_eq!(view.indices, [2, 1, 0]);
        view.descending = false;
        view.refresh(&actions);
        assert_eq!(view.indices, [0, 1, 2]);
        assert_eq!(actions, original);
    }
    #[test]
    fn status_direction_sort_and_sorted_shown_edits_keep_original_targets() {
        use crate::sync::{SyncPolicy, test_action};
        let mut actions = vec![
            test_action("z10", SyncStatus::Identical, SyncDirection::Skip),
            test_action("z2", SyncStatus::LeftOnly, SyncDirection::ToRight),
            test_action("A", SyncStatus::TypeConflict, SyncDirection::Skip),
            test_action("hidden", SyncStatus::RightOnly, SyncDirection::ToLeft),
        ];
        let hidden = actions[3].clone();
        let mut view = Review {
            sort: Sort::Status,
            ..Default::default()
        };
        view.refresh(&actions);
        assert_eq!(view.indices, [2, 1, 3, 0]);
        view.sort = Sort::Direction;
        view.refresh(&actions);
        assert_eq!(view.indices, [3, 1, 2, 0]);
        view.query = "z".into();
        view.sort = Sort::Name;
        view.descending = true;
        view.refresh(&actions);
        assert_eq!(view.indices, [0, 1]);
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Defaults),
            0
        );
        view.scope = Scope::Planned;
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Skip),
            1
        );
        assert!(view.indices.is_empty());
        assert_eq!(actions[3], hidden);
        view.query.clear();
        view.scope = Scope::All;
        view.sort = Sort::Direction;
        view.descending = false;
        assert!(view.refresh(&actions));
        assert_eq!(view.indices, [3, 2, 1, 0]);
    }

    #[test]
    fn shown_edits_capture_targets_before_scope_changes_and_never_edit_hidden_rows() {
        use crate::sync::{SyncPolicy, test_action};
        let mut actions = vec![
            test_action("visible one", SyncStatus::LeftNewer, SyncDirection::ToRight),
            test_action("visible two", SyncStatus::RightNewer, SyncDirection::ToLeft),
            test_action(
                "visible conflict",
                SyncStatus::Differing,
                SyncDirection::ToRight,
            ),
            test_action("hidden", SyncStatus::LeftNewer, SyncDirection::ToRight),
        ];
        let original = actions.clone();
        let mut view = Review {
            query: "visible".into(),
            scope: Scope::Planned,
            ..Default::default()
        };
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Skip),
            3
        );
        assert!(view.indices.is_empty());
        assert_eq!(actions[3], original[3]);
        view.scope = Scope::Skipped;
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Defaults),
            2
        );
        assert_eq!(view.indices, [2]);
        view.scope = Scope::All;
        view.refresh(&actions);
        // The editor derives targets again instead of trusting public cached IDs.
        view.indices = vec![3];
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Reverse),
            2
        );
        assert_eq!(actions[0].direction, SyncDirection::ToLeft);
        assert_eq!(actions[1].direction, SyncDirection::ToRight);
        assert_eq!(actions[2].direction, SyncDirection::Skip);
        assert_eq!(actions[3], original[3]);
        for (before, after) in original.iter().zip(&actions) {
            let mut expected = before.clone();
            expected.direction = after.direction;
            assert_eq!(*after, expected);
        }
        view.query = "missing".into();
        let before = actions.clone();
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Skip),
            0
        );
        assert_eq!(actions, before);
    }

    #[test]
    fn reverse_requires_actual_opposite_source_and_defaults_follow_each_policy_without_rechecking_files()
     {
        use crate::sync::{SyncPolicy, test_action};
        let temp = crate::testutil::TempDir::new();
        let path = temp.file("one-sided", "data");
        let entry =
            crate::panel::FileEntry::from_meta(path.clone(), &std::fs::metadata(path).unwrap())
                .unwrap();
        let mut actions = crate::sync::sync_diff(&[entry], &[], SyncPolicy::TwoWay);
        let original = actions.clone();
        let mut view = Review::default();
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Reverse),
            0
        );
        assert_eq!(actions, original);
        actions.extend([
            test_action(
                "case conflict",
                SyncStatus::CaseConflict,
                SyncDirection::ToRight,
            ),
            test_action(
                "type conflict",
                SyncStatus::TypeConflict,
                SyncDirection::ToRight,
            ),
            test_action(
                "folder pair",
                SyncStatus::DirectoryPair,
                SyncDirection::Skip,
            ),
        ]);
        let before = actions.clone();
        assert_eq!(
            view.edit_shown(&mut actions, SyncPolicy::TwoWay, Edit::Reverse),
            0
        );
        assert_eq!(actions, before);
        for policy in [
            SyncPolicy::TwoWay,
            SyncPolicy::MirrorLeftToRight,
            SyncPolicy::MirrorRightToLeft,
        ] {
            let mut pairs = vec![
                test_action("left-newer", SyncStatus::LeftNewer, SyncDirection::Skip),
                test_action("right-newer", SyncStatus::RightNewer, SyncDirection::Skip),
                test_action("differing", SyncStatus::Differing, SyncDirection::ToRight),
                test_action("matching", SyncStatus::Identical, SyncDirection::ToLeft),
            ];
            view.edit_shown(&mut pairs, policy, Edit::Defaults);
            for row in pairs {
                assert_eq!(
                    row.direction,
                    crate::sync::default_direction(row.status, policy)
                );
            }
        }
        assert_eq!(
            std::fs::read_to_string(temp.path().join("one-sided")).unwrap(),
            "data"
        );
    }

    #[test]
    fn query_projection_keeps_original_ids_and_counts_hidden_planned_without_mutating_plan() {
        let mut actions = rows();
        let original = actions.clone();
        let mut view = Review {
            query: "FINAL АРХИВ".into(),
            scope: Scope::Planned,
            ..Default::default()
        };
        assert!(view.refresh(&actions));
        assert_eq!(view.indices, [1]);
        assert_eq!(view.hidden_planned, 1);
        assert_eq!(actions, original);
        assert!(!view.refresh(&actions));
        actions[view.indices[0]].direction = SyncDirection::Skip;
        assert!(view.refresh(&actions));
        assert!(view.indices.is_empty());
        assert_eq!(actions[3], original[3]);
        assert_eq!(view.hidden_planned, 1);
        view.scope = Scope::Skipped;
        view.refresh(&actions);
        assert_eq!(view.indices, [1]);
        view.query = "x".repeat(129);
        view.refresh(&actions);
        assert!(view.indices.is_empty());
        assert_eq!(view.hidden_planned, 1);
        view.query.clear();
        view.scope = Scope::All;
        view.refresh(&actions);
        assert_eq!(view.indices, [0, 1, 2, 3, 4]);
        assert_eq!(view.hidden_planned, 0);
    }
    #[test]
    fn scopes_distinguish_metadata_matches_conflicts_and_uncompared_directories() {
        let actions = rows();
        let mut view = Review::default();
        for (scope, expected) in [
            (Scope::All, vec![0, 1, 2, 3, 4]),
            (Scope::Planned, vec![1, 3]),
            (Scope::Skipped, vec![0, 2, 4]),
            (Scope::Conflicts, vec![2]),
            (Scope::OneSided, vec![1, 3]),
            (Scope::PairedFiles, vec![0]),
            (Scope::MatchingMetadata, vec![0]),
            (Scope::Folders, vec![4]),
        ] {
            view.scope = scope;
            view.refresh(&actions);
            assert_eq!(view.indices, expected);
        }
        let mut changed = actions.clone();
        changed[0].status = SyncStatus::Differing;
        view.scope = Scope::Conflicts;
        assert!(view.refresh(&changed));
        assert_eq!(view.indices, [0, 2]);
        changed.swap(0, 2);
        assert!(view.refresh(&changed));
        assert_eq!(view.indices, [0, 2]);
    }
}
