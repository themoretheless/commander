//! Contextual quick actions for the bottom status rail.
//! The UI owns the buttons; this module only decides which actions fit the
//! current workspace scope.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickAction {
    AddToShelf,
    DrainShelf,
    CopyNames,
    BatchRename,
    ClearSelection,
    ClearFilters,
    OpenPalette,
    FindFiles,
    RecentFolders,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickActionSpec {
    pub action: QuickAction,
    pub label: String,
    pub hint: &'static str,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuickActionContext {
    pub selected_count: usize,
    pub shelf_count: usize,
    pub has_filters: bool,
}

pub fn actions(ctx: QuickActionContext) -> Vec<QuickActionSpec> {
    let mut specs = Vec::new();

    if ctx.shelf_count > 0 {
        specs.push(spec(
            QuickAction::DrainShelf,
            format!("Drain {}", ctx.shelf_count),
            "Copy staged shelf items into the active folder",
        ));
    }
    if ctx.has_filters {
        specs.push(spec(
            QuickAction::ClearFilters,
            "Clear filters",
            "Reset text and facet filters in the active panel",
        ));
    }
    if ctx.selected_count > 0 {
        specs.push(spec(
            QuickAction::AddToShelf,
            format!("Shelf +{}", ctx.selected_count),
            "Stage the current selection on the shelf",
        ));
        specs.push(spec(
            QuickAction::CopyNames,
            "Copy names",
            "Copy selected file names to the clipboard",
        ));
        specs.push(spec(
            QuickAction::BatchRename,
            "Batch rename",
            "Open the rename studio for the current selection",
        ));
        specs.push(spec(
            QuickAction::ClearSelection,
            "Clear sel",
            "Drop the active panel selection",
        ));
    } else {
        specs.push(spec(
            QuickAction::OpenPalette,
            "Actions",
            "Open the command palette",
        ));
        specs.push(spec(
            QuickAction::FindFiles,
            "Find",
            "Search recursively from the active folder",
        ));
        specs.push(spec(
            QuickAction::RecentFolders,
            "Recent",
            "Jump to a recent folder",
        ));
    }

    specs.truncate(4);
    specs
}

fn spec(action: QuickAction, label: impl Into<String>, hint: &'static str) -> QuickActionSpec {
    QuickActionSpec {
        action,
        label: label.into(),
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(specs: &[QuickActionSpec]) -> Vec<QuickAction> {
        specs.iter().map(|s| s.action).collect()
    }

    #[test]
    fn idle_scope_offers_navigation_starters() {
        let specs = actions(QuickActionContext::default());
        assert_eq!(
            kinds(&specs),
            vec![
                QuickAction::OpenPalette,
                QuickAction::FindFiles,
                QuickAction::RecentFolders
            ]
        );
    }

    #[test]
    fn selection_scope_prioritizes_selection_workflows() {
        let specs = actions(QuickActionContext {
            selected_count: 3,
            shelf_count: 0,
            has_filters: false,
        });
        assert_eq!(
            kinds(&specs),
            vec![
                QuickAction::AddToShelf,
                QuickAction::CopyNames,
                QuickAction::BatchRename,
                QuickAction::ClearSelection
            ]
        );
        assert_eq!(specs[0].label, "Shelf +3");
    }

    #[test]
    fn shelf_and_filters_take_precedence_over_idle_actions() {
        let specs = actions(QuickActionContext {
            selected_count: 0,
            shelf_count: 2,
            has_filters: true,
        });
        assert_eq!(
            kinds(&specs),
            vec![
                QuickAction::DrainShelf,
                QuickAction::ClearFilters,
                QuickAction::OpenPalette,
                QuickAction::FindFiles
            ]
        );
    }
}
