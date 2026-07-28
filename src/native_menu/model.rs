use crate::ports::{ContextMenuAction, ContextMenuFailure, ContextMenuResult, ContextMenuTarget};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TagColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Gray,
}

impl TagColor {
    pub const ALL: [Self; 7] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Gray,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Gray => "Gray",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum MenuItemId {
    Open,
    OpenWith,
    OpenWithApplication(usize),
    QuickLook,
    GetInfo,
    Tags,
    Tag(TagColor),
    Duplicate,
    Compress,
    CopyPath,
    Share,
    ShareService(usize),
    Reveal,
    MoveToTrash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuItemState {
    Off,
    On,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeyEquivalent {
    pub key: String,
    pub command: bool,
    pub option: bool,
    pub control: bool,
    pub shift: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuIntent {
    Open,
    OpenWith { application: PathBuf },
    QuickLook,
    GetInfo,
    ToggleTag { tag: String },
    Duplicate,
    Compress,
    CopyPath,
    Share { service: String },
    Reveal,
    MoveToTrash,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MenuItem {
    pub id: MenuItemId,
    pub title: String,
    pub accessible_label: String,
    pub enabled: bool,
    pub state: MenuItemState,
    pub key_equivalent: Option<KeyEquivalent>,
    #[serde(skip)]
    pub intent: Option<MenuIntent>,
    pub children: Vec<MenuNode>,
}

impl MenuItem {
    fn action(id: MenuItemId, title: impl Into<String>, enabled: bool, intent: MenuIntent) -> Self {
        let title = title.into();
        Self {
            id,
            accessible_label: title.clone(),
            title,
            enabled,
            state: MenuItemState::Off,
            key_equivalent: None,
            intent: Some(intent),
            children: Vec::new(),
        }
    }

    fn submenu(
        id: MenuItemId,
        title: impl Into<String>,
        enabled: bool,
        children: Vec<MenuNode>,
    ) -> Self {
        let title = title.into();
        Self {
            id,
            accessible_label: title.clone(),
            title,
            enabled,
            state: MenuItemState::Off,
            key_equivalent: None,
            intent: None,
            children,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "node")]
pub enum MenuNode {
    Item(MenuItem),
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuInvocation {
    pub id: u64,
    pub bound_target: ContextMenuTarget,
    pub tree: Vec<MenuNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuSelection {
    pub invocation_id: u64,
    pub bound_target: PathBuf,
    pub item_id: MenuItemId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DynamicMenuEntries {
    pub open_with: Vec<(String, PathBuf)>,
    pub applied_tags: BTreeSet<String>,
    pub tags_available: bool,
    /// Localized title plus the stable AppKit service name.
    pub share_services: Vec<(String, String)>,
}

fn clipped_name(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    format!("{}...", name.chars().take(keep).collect::<String>())
}

pub fn build_invocation(
    id: u64,
    target: ContextMenuTarget,
    display_name: &str,
    mut dynamic: DynamicMenuEntries,
) -> MenuInvocation {
    dynamic
        .open_with
        .sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    dynamic.open_with.dedup();
    dynamic.share_services.sort();
    dynamic.share_services.dedup();

    let open_with = dynamic
        .open_with
        .into_iter()
        .enumerate()
        .map(|(index, (title, application))| {
            MenuNode::Item(MenuItem::action(
                MenuItemId::OpenWithApplication(index),
                title,
                true,
                MenuIntent::OpenWith { application },
            ))
        })
        .collect::<Vec<_>>();
    let tags = TagColor::ALL
        .into_iter()
        .map(|color| {
            let tag = color.label().to_string();
            let state = if dynamic.applied_tags.contains(&tag) {
                MenuItemState::On
            } else {
                MenuItemState::Off
            };
            let mut item = MenuItem::action(
                MenuItemId::Tag(color),
                color.label(),
                dynamic.tags_available,
                MenuIntent::ToggleTag { tag },
            );
            item.state = state;
            MenuNode::Item(item)
        })
        .collect::<Vec<_>>();
    let share = dynamic
        .share_services
        .into_iter()
        .enumerate()
        .map(|(index, (title, service))| {
            MenuNode::Item(MenuItem::action(
                MenuItemId::ShareService(index),
                title,
                true,
                MenuIntent::Share { service },
            ))
        })
        .collect::<Vec<_>>();
    let target_available = target.expected.exists;
    let compress_title = format!("Compress \"{}\"", clipped_name(display_name, 42));

    let tree = vec![
        MenuNode::Item(MenuItem::action(
            MenuItemId::Open,
            "Open",
            target_available,
            MenuIntent::Open,
        )),
        MenuNode::Item(MenuItem::submenu(
            MenuItemId::OpenWith,
            "Open With",
            !open_with.is_empty(),
            open_with,
        )),
        MenuNode::Item(MenuItem::action(
            MenuItemId::QuickLook,
            "Quick Look",
            target_available,
            MenuIntent::QuickLook,
        )),
        MenuNode::Separator,
        MenuNode::Item(MenuItem::action(
            MenuItemId::GetInfo,
            "Get Info",
            target_available,
            MenuIntent::GetInfo,
        )),
        MenuNode::Item(MenuItem::submenu(
            MenuItemId::Tags,
            "Tags",
            dynamic.tags_available,
            tags,
        )),
        MenuNode::Separator,
        MenuNode::Item(MenuItem::action(
            MenuItemId::Duplicate,
            "Duplicate",
            target_available,
            MenuIntent::Duplicate,
        )),
        MenuNode::Item(MenuItem::action(
            MenuItemId::Compress,
            compress_title,
            target_available,
            MenuIntent::Compress,
        )),
        MenuNode::Separator,
        MenuNode::Item(MenuItem::action(
            MenuItemId::CopyPath,
            "Copy Path",
            true,
            MenuIntent::CopyPath,
        )),
        MenuNode::Item(MenuItem::submenu(
            MenuItemId::Share,
            "Share",
            !share.is_empty(),
            share,
        )),
        MenuNode::Separator,
        MenuNode::Item(MenuItem::action(
            MenuItemId::Reveal,
            "Show in Finder",
            target_available,
            MenuIntent::Reveal,
        )),
        MenuNode::Separator,
        MenuNode::Item(MenuItem::action(
            MenuItemId::MoveToTrash,
            "Move to Trash",
            target_available,
            MenuIntent::MoveToTrash,
        )),
    ];

    MenuInvocation {
        id,
        bound_target: target,
        tree,
    }
}

fn selected_item<'a>(nodes: &'a [MenuNode], selected: &MenuItemId) -> Option<&'a MenuItem> {
    fn visit<'a>(
        nodes: &'a [MenuNode],
        selected: &MenuItemId,
        found: &mut Option<&'a MenuItem>,
        duplicate: &mut bool,
    ) {
        for node in nodes {
            let MenuNode::Item(item) = node else {
                continue;
            };
            if &item.id == selected && found.replace(item).is_some() {
                *duplicate = true;
            }
            visit(&item.children, selected, found, duplicate);
        }
    }

    let mut found = None;
    let mut duplicate = false;
    visit(nodes, selected, &mut found, &mut duplicate);
    (!duplicate).then_some(found).flatten()
}

fn target_binding_is_current(target: &ContextMenuTarget) -> bool {
    crate::path_identity::PathIdentity::observe(&target.path)
        .is_ok_and(|current| target.expected.same_binding(&current))
}

pub fn selection_result(
    invocation: &MenuInvocation,
    selection: Option<MenuSelection>,
) -> ContextMenuResult {
    let Some(selection) = selection else {
        return ContextMenuResult::Dismissed;
    };
    if selection.invocation_id != invocation.id
        || selection.bound_target != invocation.bound_target.path
    {
        return ContextMenuResult::Failed(ContextMenuFailure::StaleInvocation);
    }
    let Some(item) = selected_item(&invocation.tree, &selection.item_id) else {
        return ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection);
    };
    if !item.enabled {
        return ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection);
    }
    let Some(intent) = item.intent.clone() else {
        return ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection);
    };
    if !target_binding_is_current(&invocation.bound_target) {
        return ContextMenuResult::Failed(ContextMenuFailure::StaleInvocation);
    }
    let target = invocation.bound_target.clone();
    match intent {
        MenuIntent::Open => ContextMenuResult::OpenRequested,
        MenuIntent::OpenWith { application } => {
            ContextMenuResult::OpenWithRequested { application }
        }
        MenuIntent::QuickLook => ContextMenuResult::QuickLookRequested,
        MenuIntent::GetInfo => ContextMenuResult::GetInfoRequested,
        MenuIntent::ToggleTag { tag } => {
            ContextMenuResult::DeferredActionRequested(ContextMenuAction::ToggleTag { target, tag })
        }
        MenuIntent::Duplicate => {
            ContextMenuResult::DeferredActionRequested(ContextMenuAction::Duplicate(target))
        }
        MenuIntent::Compress => {
            ContextMenuResult::DeferredActionRequested(ContextMenuAction::Compress(target))
        }
        MenuIntent::CopyPath => ContextMenuResult::CopyPathRequested,
        MenuIntent::Share { service } => {
            ContextMenuResult::DeferredActionRequested(ContextMenuAction::Share { target, service })
        }
        MenuIntent::Reveal => ContextMenuResult::RevealRequested,
        MenuIntent::MoveToTrash => ContextMenuResult::MoveToTrashRequested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> MenuInvocation {
        let path = std::env::current_dir().expect("current directory");
        build_invocation(
            41,
            ContextMenuTarget {
                expected: crate::path_identity::PathIdentity::observe(&path)
                    .expect("fixture identity"),
                path,
            },
            "a very long but readable fixture filename.txt",
            DynamicMenuEntries {
                open_with: vec![
                    ("Zed".to_string(), PathBuf::from("/Applications/Zed.app")),
                    (
                        "Preview".to_string(),
                        PathBuf::from("/System/Applications/Preview.app"),
                    ),
                ],
                applied_tags: BTreeSet::from(["Blue".to_string()]),
                tags_available: true,
                share_services: vec![
                    ("Mail".to_string(), "com.apple.share.Mail".to_string()),
                    ("AirDrop".to_string(), "com.apple.share.AirDrop".to_string()),
                ],
            },
        )
    }

    #[test]
    fn model_has_stable_order_accessible_labels_and_submenus() {
        let invocation = fixture();
        let top_level = invocation
            .tree
            .iter()
            .map(|node| match node {
                MenuNode::Item(item) => item.title.as_str(),
                MenuNode::Separator => "-",
            })
            .collect::<Vec<_>>();
        assert_eq!(
            top_level,
            [
                "Open",
                "Open With",
                "Quick Look",
                "-",
                "Get Info",
                "Tags",
                "-",
                "Duplicate",
                "Compress \"a very long but readable fixture filena...\"",
                "-",
                "Copy Path",
                "Share",
                "-",
                "Show in Finder",
                "-",
                "Move to Trash",
            ]
        );
        for item in invocation.tree.iter().filter_map(|node| match node {
            MenuNode::Item(item) => Some(item),
            MenuNode::Separator => None,
        }) {
            assert!(!item.accessible_label.is_empty());
            assert_eq!(item.key_equivalent, None);
        }
    }

    #[test]
    fn dynamic_children_are_sorted_and_tag_state_is_explicit() {
        let invocation = fixture();
        let submenu = |id: MenuItemId| {
            invocation.tree.iter().find_map(|node| match node {
                MenuNode::Item(item) if item.id == id => Some(&item.children),
                _ => None,
            })
        };
        let open_with = submenu(MenuItemId::OpenWith).expect("Open With submenu");
        assert!(matches!(
            &open_with[0],
            MenuNode::Item(MenuItem {
                title,
                id: MenuItemId::OpenWithApplication(0),
                ..
            }) if title == "Preview"
        ));
        let tags = submenu(MenuItemId::Tags).expect("Tags submenu");
        assert!(matches!(
            &tags[4],
            MenuNode::Item(MenuItem {
                id: MenuItemId::Tag(TagColor::Blue),
                state: MenuItemState::On,
                ..
            })
        ));
        let share = submenu(MenuItemId::Share).expect("Share submenu");
        assert!(matches!(
            &share[0],
            MenuNode::Item(MenuItem { title, .. }) if title == "AirDrop"
        ));
    }

    #[test]
    fn selection_is_bound_to_one_invocation_and_target() {
        let invocation = fixture();
        let selection = MenuSelection {
            invocation_id: invocation.id + 1,
            bound_target: invocation.bound_target.path.clone(),
            item_id: MenuItemId::Duplicate,
        };
        assert_eq!(
            selection_result(&invocation, Some(selection)),
            ContextMenuResult::Failed(ContextMenuFailure::StaleInvocation)
        );

        let selected = MenuSelection {
            invocation_id: invocation.id,
            bound_target: invocation.bound_target.path.clone(),
            item_id: MenuItemId::Duplicate,
        };
        assert_eq!(
            selection_result(&invocation, Some(selected)),
            ContextMenuResult::DeferredActionRequested(ContextMenuAction::Duplicate(
                invocation.bound_target
            ))
        );
    }

    #[test]
    fn selection_rejects_forged_and_disabled_item_ids() {
        let invocation = fixture();
        let forged = MenuSelection {
            invocation_id: invocation.id,
            bound_target: invocation.bound_target.path.clone(),
            item_id: MenuItemId::OpenWithApplication(usize::MAX),
        };
        assert_eq!(
            selection_result(&invocation, Some(forged)),
            ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection)
        );

        let missing = PathBuf::from(format!(
            "/tmp/commander-missing-menu-target-{}",
            std::process::id()
        ));
        let disabled = build_invocation(
            42,
            ContextMenuTarget {
                expected: crate::path_identity::PathIdentity::missing(&missing),
                path: missing.clone(),
            },
            "missing",
            DynamicMenuEntries::default(),
        );
        assert_eq!(
            selection_result(
                &disabled,
                Some(MenuSelection {
                    invocation_id: disabled.id,
                    bound_target: missing,
                    item_id: MenuItemId::MoveToTrash,
                })
            ),
            ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection)
        );
    }

    #[test]
    fn selection_rejects_a_target_replaced_while_tracking() {
        let root = std::env::temp_dir().join(format!(
            "commander-menu-binding-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("item.txt");
        std::fs::write(&path, "before").unwrap();
        let invocation = build_invocation(
            43,
            ContextMenuTarget {
                expected: crate::path_identity::PathIdentity::observe(&path).unwrap(),
                path: path.clone(),
            },
            "item.txt",
            DynamicMenuEntries::default(),
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "replacement with different metadata").unwrap();

        assert_eq!(
            selection_result(
                &invocation,
                Some(MenuSelection {
                    invocation_id: invocation.id,
                    bound_target: path,
                    item_id: MenuItemId::Duplicate,
                })
            ),
            ContextMenuResult::Failed(ContextMenuFailure::StaleInvocation)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
