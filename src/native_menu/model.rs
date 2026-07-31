use crate::ports::{ContextMenuAction, ContextMenuFailure, ContextMenuResult, ContextMenuTarget};
use serde::Serialize;
use std::collections::BTreeSet;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use unicode_segmentation::UnicodeSegmentation;

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
    OpenWithApplication(String),
    QuickLook,
    GetInfo,
    Tags,
    Tag(TagColor),
    Duplicate,
    Compress,
    CopyPath,
    Share,
    ShareService(String),
    Reveal,
    MoveToTrash,
}

impl MenuItemId {
    pub fn accessibility_id(&self) -> String {
        let suffix = match self {
            Self::Open => "open".to_string(),
            Self::OpenWith => "open_with".to_string(),
            Self::OpenWithApplication(identity) => {
                format!("open_with.application.{identity}")
            }
            Self::QuickLook => "quick_look".to_string(),
            Self::GetInfo => "get_info".to_string(),
            Self::Tags => "tags".to_string(),
            Self::Tag(color) => format!("tag.{}", color.label().to_ascii_lowercase()),
            Self::Duplicate => "duplicate".to_string(),
            Self::Compress => "compress".to_string(),
            Self::CopyPath => "copy_path".to_string(),
            Self::Share => "share".to_string(),
            Self::ShareService(identity) => format!("share.service.{identity}"),
            Self::Reveal => "reveal".to_string(),
            Self::MoveToTrash => "move_to_trash".to_string(),
        };
        format!("commander.context_menu.{suffix}")
    }
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
    pub accessibility_id: String,
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
            accessibility_id: id.accessibility_id(),
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
            accessibility_id: id.accessibility_id(),
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

    fn with_key_equivalent(mut self, key: &str, command: bool) -> Self {
        self.key_equivalent = Some(KeyEquivalent {
            key: key.to_string(),
            command,
            option: false,
            control: false,
            shift: false,
        });
        self
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
    let graphemes = UnicodeSegmentation::graphemes(name, true).collect::<Vec<_>>();
    if graphemes.len() <= max_chars {
        return name.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    format!("{}...", graphemes[..keep].concat())
}

fn stable_value_id(domain: &str, value: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    hasher.update(value);
    hasher.finalize().to_hex().to_string()
}

fn stable_path_id(path: &Path) -> String {
    #[cfg(unix)]
    {
        stable_value_id("commander.open-with.path.v1", path.as_os_str().as_bytes())
    }
    #[cfg(windows)]
    {
        let mut bytes = Vec::new();
        for unit in path.as_os_str().encode_wide() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        stable_value_id("commander.open-with.path.v1", &bytes)
    }
    #[cfg(not(any(unix, windows)))]
    {
        stable_value_id(
            "commander.open-with.path.v1",
            path.to_string_lossy().as_bytes(),
        )
    }
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
    let mut application_paths = BTreeSet::new();
    dynamic
        .open_with
        .retain(|(_, path)| application_paths.insert(path.clone()));
    dynamic.share_services.sort();
    let mut service_names = BTreeSet::new();
    dynamic
        .share_services
        .retain(|(_, service)| service_names.insert(service.clone()));

    let open_with = dynamic
        .open_with
        .into_iter()
        .map(|(title, application)| {
            let identity = stable_path_id(&application);
            MenuNode::Item(MenuItem::action(
                MenuItemId::OpenWithApplication(identity),
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
        .map(|(title, service)| {
            let identity = stable_value_id("commander.share.service.v1", service.as_bytes());
            MenuNode::Item(MenuItem::action(
                MenuItemId::ShareService(identity),
                title,
                true,
                MenuIntent::Share { service },
            ))
        })
        .collect::<Vec<_>>();
    let target_available = target.expected.exists;
    let compress_title = format!("Compress \"{}\"", clipped_name(display_name, 42));
    let compress_accessible_label = format!("Compress \"{display_name}\"");

    let tree = vec![
        MenuNode::Item(
            MenuItem::action(MenuItemId::Open, "Open", target_available, MenuIntent::Open)
                .with_key_equivalent("o", true),
        ),
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
        MenuNode::Item({
            let mut item = MenuItem::action(
                MenuItemId::Compress,
                compress_title,
                target_available,
                MenuIntent::Compress,
            );
            item.accessible_label = compress_accessible_label;
            item
        }),
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

pub fn tracking_result(
    invocation: &MenuInvocation,
    appkit_reported_selection: bool,
    selection: Option<MenuSelection>,
) -> ContextMenuResult {
    match (appkit_reported_selection, selection) {
        (false, None) => ContextMenuResult::Dismissed,
        (true, Some(selection)) => selection_result(invocation, Some(selection)),
        (true, None) | (false, Some(_)) => {
            ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection)
        }
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
            assert!(item.accessibility_id.starts_with("commander.context_menu."));
        }
        let open = invocation
            .tree
            .iter()
            .find_map(|node| match node {
                MenuNode::Item(item) if item.id == MenuItemId::Open => Some(item),
                _ => None,
            })
            .expect("Open");
        assert_eq!(
            open.key_equivalent,
            Some(KeyEquivalent {
                key: "o".to_string(),
                command: true,
                option: false,
                control: false,
                shift: false,
            })
        );
        let compress = invocation
            .tree
            .iter()
            .find_map(|node| match node {
                MenuNode::Item(item) if item.id == MenuItemId::Compress => Some(item),
                _ => None,
            })
            .expect("Compress");
        assert_eq!(
            compress.accessible_label,
            "Compress \"a very long but readable fixture filename.txt\""
        );
        assert_ne!(compress.title, compress.accessible_label);
    }

    #[test]
    fn visual_clipping_preserves_graphemes_while_accessibility_keeps_full_name() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let name = format!("{}{family}-a-long-suffix.txt", "a".repeat(38));
        let path = std::env::current_dir().unwrap();
        let invocation = build_invocation(
            44,
            ContextMenuTarget {
                expected: crate::path_identity::PathIdentity::observe(&path).unwrap(),
                path,
            },
            &name,
            DynamicMenuEntries::default(),
        );
        let compress = invocation
            .tree
            .iter()
            .find_map(|node| match node {
                MenuNode::Item(item) if item.id == MenuItemId::Compress => Some(item),
                _ => None,
            })
            .unwrap();
        assert_eq!(compress.accessible_label, format!("Compress \"{name}\""));
        assert!(compress.title.contains(family));
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
                id: MenuItemId::OpenWithApplication(_),
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
    fn dynamic_accessibility_ids_survive_unrelated_provider_insertions() {
        let original = fixture();
        let mut entries = DynamicMenuEntries {
            open_with: vec![
                (
                    "Earlier".to_string(),
                    PathBuf::from("/Applications/Earlier.app"),
                ),
                (
                    "Preview".to_string(),
                    PathBuf::from("/System/Applications/Preview.app"),
                ),
                ("Zed".to_string(), PathBuf::from("/Applications/Zed.app")),
            ],
            share_services: vec![
                ("AirDrop".to_string(), "com.apple.share.AirDrop".to_string()),
                ("Earlier".to_string(), "com.apple.share.AAA".to_string()),
                ("Mail".to_string(), "com.apple.share.Mail".to_string()),
            ],
            ..DynamicMenuEntries::default()
        };
        entries.applied_tags.insert("Blue".to_string());
        entries.tags_available = true;
        let updated = build_invocation(
            original.id,
            original.bound_target.clone(),
            "a very long but readable fixture filename.txt",
            entries,
        );
        let dynamic_id = |invocation: &MenuInvocation, parent: MenuItemId, title: &str| {
            invocation
                .tree
                .iter()
                .find_map(|node| match node {
                    MenuNode::Item(item) if item.id == parent => Some(&item.children),
                    _ => None,
                })
                .and_then(|children| {
                    children.iter().find_map(|node| match node {
                        MenuNode::Item(item) if item.title == title => {
                            Some(item.accessibility_id.clone())
                        }
                        _ => None,
                    })
                })
                .unwrap()
        };
        assert_eq!(
            dynamic_id(&original, MenuItemId::OpenWith, "Preview"),
            dynamic_id(&updated, MenuItemId::OpenWith, "Preview")
        );
        assert_eq!(
            dynamic_id(&original, MenuItemId::Share, "AirDrop"),
            dynamic_id(&updated, MenuItemId::Share, "AirDrop")
        );
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
            item_id: MenuItemId::OpenWithApplication("forged".to_string()),
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

    #[test]
    fn tracking_result_distinguishes_escape_from_inconsistent_appkit_state() {
        let invocation = fixture();
        assert_eq!(
            tracking_result(&invocation, false, None),
            ContextMenuResult::Dismissed
        );
        assert_eq!(
            tracking_result(&invocation, true, None),
            ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection)
        );
        assert_eq!(
            tracking_result(
                &invocation,
                false,
                Some(MenuSelection {
                    invocation_id: invocation.id,
                    bound_target: invocation.bound_target.path.clone(),
                    item_id: MenuItemId::CopyPath,
                })
            ),
            ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection)
        );
    }
}
