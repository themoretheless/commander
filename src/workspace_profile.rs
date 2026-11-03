//! Named workspace profiles (research **J008**).

mod exchange;
pub use exchange::{
    MAX_BYTES as MAX_EXCHANGE_BYTES, decode as decode_exchange, encode as encode_exchange,
};

use crate::filesystem_policy::{NamePolicy, SymlinkPolicy};
use crate::operation::DurabilityProfile;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.workspace_profiles", 1, 4 * 1024 * 1024);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProfile {
    pub name: String,
    pub left_root: PathBuf,
    pub right_root: PathBuf,
    #[serde(default)]
    pub left_filter: String,
    #[serde(default)]
    pub right_filter: String,
    #[serde(default)]
    pub durability: DurabilityProfile,
    #[serde(default)]
    pub name_policy: NamePolicy,
    #[serde(default)]
    pub symlink_policy: SymlinkPolicy,
    #[serde(default)]
    pub trusted_command_templates: Vec<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub tab_layout: serde_json::Value,
}

impl WorkspaceProfile {
    pub fn new(name: impl Into<String>, left_root: PathBuf, right_root: PathBuf) -> Self {
        Self {
            name: name.into(),
            left_root,
            right_root,
            left_filter: String::new(),
            right_filter: String::new(),
            durability: DurabilityProfile::default(),
            name_policy: NamePolicy::default(),
            symlink_policy: SymlinkPolicy::default(),
            trusted_command_templates: Vec::new(),
            tab_layout: serde_json::Value::Null,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBook {
    pub profiles: Vec<WorkspaceProfile>,
}

impl ProfileBook {
    pub fn upsert(&mut self, profile: WorkspaceProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == profile.name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    pub fn get(&self, name: &str) -> Option<&WorkspaceProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

pub struct TabLayout {
    pub left: crate::workspace::tabs::SavedTabs,
    pub right: crate::workspace::tabs::SavedTabs,
    pub active: usize,
}
impl WorkspaceProfile {
    pub(crate) fn layout(&self) -> Result<Option<TabLayout>, String> {
        let value = &self.tab_layout;
        if value.is_null() {
            return Ok(None);
        }
        if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > 512 * 1024 {
            return Err("Profile tab layout exceeds 512 KiB".into());
        }
        let object = value.as_object().ok_or("Invalid profile tab layout")?;
        if object.len() != 4
            || value["version"] != 1
            || !object
                .keys()
                .all(|k| ["version", "left", "right", "active"].contains(&k.as_str()))
        {
            return Err("Expected profile tab layout version 1".into());
        }
        let active = value["active"]
            .as_u64()
            .filter(|n| *n < 2)
            .ok_or("Invalid active profile panel")? as usize;
        let parse_side = |key: &str| -> Result<crate::workspace::tabs::SavedTabs, String> {
            let side = value[key].as_object().ok_or("Invalid profile tab panel")?;
            if side.len() != 2 || !side.contains_key("entries") || !side.contains_key("active") {
                return Err("Invalid profile panel fields".into());
            }
            let rows = side["entries"].as_array().ok_or("Missing profile tabs")?;
            if rows.is_empty() || rows.len() > crate::workspace::tabs::MAX_TABS {
                return Err("Each profile panel needs 1–8 tabs".into());
            }
            let index = side["active"]
                .as_u64()
                .filter(|n| *n < (rows.len() as u64))
                .ok_or("Invalid active profile tab")? as usize;
            let mut entries = Vec::new();
            for row in rows {
                if !row.as_object().is_some_and(|o| {
                    o.keys().all(|k| {
                        [
                            "path",
                            "title",
                            "close_protected",
                            "full_path_label",
                            "pinned_path",
                            "view",
                            "miller_widths",
                        ]
                        .contains(&k.as_str())
                    })
                }) {
                    return Err("Invalid profile tab fields".into());
                }
                let tab: crate::workspace::tabs::SavedTab =
                    serde_json::from_value(row.clone()).map_err(|e| e.to_string())?;
                if !tab.path.is_absolute() {
                    return Err("Profile tab paths must be absolute".into());
                }
                entries.push(tab);
            }
            Ok(crate::workspace::tabs::SavedTabs {
                entries,
                active: index,
                closed: Vec::new(),
            })
        };
        let left = parse_side("left")?;
        let right = parse_side("right")?;
        if left.entries[left.active].path != self.left_root
            || right.entries[right.active].path != self.right_root
        {
            return Err("Profile active tabs disagree with saved roots".into());
        }
        Ok(Some(TabLayout {
            left,
            right,
            active,
        }))
    }
    pub fn with_layout(mut self, layout: serde_json::Value) -> Result<Self, String> {
        self.tab_layout = layout;
        validate_profile(&self)?;
        Ok(self)
    }
}

fn validate_profile(profile: &WorkspaceProfile) -> Result<(), String> {
    if profile.name.trim().is_empty()
        || profile.name.chars().count() > 80
        || profile.name.chars().any(char::is_control)
    {
        return Err("Use a profile name of 1–80 characters without controls".into());
    }
    if !profile.left_root.is_absolute() || !profile.right_root.is_absolute() {
        return Err("Profile folders must be absolute paths".into());
    }
    if profile.left_filter.len() > 4096 || profile.right_filter.len() > 4096 {
        return Err("Profile filters are limited to 4 KiB each".into());
    }
    profile.layout()?;
    Ok(())
}
impl ProfileBook {
    pub fn add_named(&mut self, profile: WorkspaceProfile) -> Result<(), String> {
        validate_profile(&profile)?;
        if self.get(&profile.name).is_some() {
            return Err("That profile already exists; use Replace selected".into());
        }
        if self.profiles.len() >= 128 {
            return Err("At most 128 named workspace profiles".into());
        }
        self.profiles.push(profile);
        Ok(())
    }
    pub fn rename_named(&mut self, old: &str, new: String) -> Result<bool, String> {
        let index = self
            .profiles
            .iter()
            .position(|p| p.name == old)
            .ok_or("Select an existing profile")?;
        let mut renamed = self.profiles[index].clone();
        renamed.name = new;
        validate_profile(&renamed)?;
        if old == renamed.name {
            return Ok(false);
        }
        if self.get(&renamed.name).is_some() {
            return Err("That profile name already exists".into());
        }
        self.profiles[index] = renamed;
        Ok(true)
    }
    pub fn replace_named(&mut self, profile: WorkspaceProfile) -> Result<(), String> {
        validate_profile(&profile)?;
        let old = self
            .profiles
            .iter_mut()
            .find(|p| p.name == profile.name)
            .ok_or("Select an existing profile")?;
        *old = profile;
        Ok(())
    }
}

pub struct Editor {
    pub book: ProfileBook,
    pub name: String,
    pub selected: Option<String>,
    pub error: Option<String>,
    path: PathBuf,
    pub exchange_open: bool,
    pub exchange_text: String,
    pub import_preview: Option<WorkspaceProfile>,
}
impl Editor {
    pub fn open() -> Self {
        Self::open_at(store_path())
    }
    pub(crate) fn open_at(path: PathBuf) -> Self {
        Self {
            book: load_store_at(persist(), &path),
            name: String::new(),
            selected: None,
            error: None,
            path,
            exchange_open: false,
            exchange_text: String::new(),
            import_preview: None,
        }
    }
    pub fn commit(&mut self, next: ProfileBook) -> Result<(), String> {
        commit_if_unchanged_at(persist(), &self.path, &self.book, &next)?;
        self.book = next;
        self.error = None;
        if self.path == store_path() {
            *crate::lock_util::recover(cache()) = self.book.clone();
        }
        Ok(())
    }
    pub fn export_selected(&self) -> Result<String, String> {
        let profile = self
            .selected
            .as_deref()
            .and_then(|n| self.book.get(n))
            .ok_or("Select a saved profile to export")?;
        encode_exchange(profile)
    }
    pub fn review_import(&mut self) -> Result<(), String> {
        self.import_preview = None;
        self.import_preview = Some(decode_exchange(&self.exchange_text)?);
        self.error = None;
        Ok(())
    }
    pub fn add_reviewed_import(&mut self) -> Result<(), String> {
        let current = decode_exchange(&self.exchange_text)?;
        if self.import_preview.as_ref() != Some(&current) {
            return Err("Review the changed JSON before adding it".into());
        }
        let name = current.name.clone();
        let mut next = self.book.clone();
        next.add_named(current)?;
        self.commit(next)?;
        self.selected = Some(name.clone());
        self.name = name;
        self.import_preview = None;
        Ok(())
    }

    pub fn rename_selected(&mut self, new: String) -> Result<(), String> {
        let old = self
            .selected
            .as_deref()
            .ok_or("Select an existing profile")?;
        let mut next = self.book.clone();
        if next.rename_named(old, new.clone())? {
            self.commit(next)?;
        }
        self.selected = Some(new);
        self.error = None;
        Ok(())
    }
    pub fn duplicate_selected(&mut self, new: String) -> Result<(), String> {
        let old = self
            .selected
            .as_deref()
            .and_then(|name| self.book.get(name))
            .ok_or("Select an existing profile")?;
        let mut copy = old.clone();
        copy.name = new.clone();
        let mut next = self.book.clone();
        next.add_named(copy)?;
        self.commit(next)?;
        self.selected = Some(new);
        Ok(())
    }

    pub fn reload(&mut self) {
        self.book = load_store_at(persist(), &self.path);
        self.selected = None;
        self.error = None;
    }
}
fn commit_if_unchanged_at(
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    expected: &ProfileBook,
    next: &ProfileBook,
) -> Result<(), String> {
    let mut loaded = crate::persistence::load_enveloped::<ProfileBook>(persist, path, STORE);
    if loaded.gate.status().fails_closed() {
        return Err("Profile store is unreadable; see persistence health".into());
    }
    if loaded.value.unwrap_or_default() != *expected {
        return Err("Profiles changed outside this window; Reload before saving".into());
    }
    crate::persistence::save_enveloped(
        persist,
        path,
        STORE,
        next,
        &mut loaded.gate,
        crate::persistence::SaveIntent::Explicit,
    )
    .map_err(|e| format!("Could not save workspace profiles: {e:?}"))?;
    Ok(())
}

fn store_path() -> PathBuf {
    crate::fs_util::config_dir().join("workspace_profiles.json")
}

fn cache() -> &'static Mutex<ProfileBook> {
    static CACHE: OnceLock<Mutex<ProfileBook>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(load_store()))
}

fn persist() -> &'static crate::persistence::FsPersist {
    static PERSIST: OnceLock<crate::persistence::FsPersist> = OnceLock::new();
    PERSIST.get_or_init(crate::persistence::FsPersist::default)
}

fn load_store() -> ProfileBook {
    load_store_at(persist(), &store_path())
}

/// A store that exists but cannot be read loads empty and blocks saves, so
/// it is reported through persistence health instead of vanishing silently.
fn load_store_at(persist: &dyn crate::persistence::Persist, path: &Path) -> ProfileBook {
    let loaded = crate::persistence::load_enveloped::<ProfileBook>(persist, path, STORE);
    if loaded.value.is_none() && loaded.gate.status().fails_closed() {
        crate::persistence::record_unreadable("Workspace profiles");
    }
    loaded.value.unwrap_or_default()
}

fn save_store(store: &ProfileBook) -> bool {
    save_store_at(persist(), &store_path(), store)
}

/// Failed or blocked saves reach persistence health (and its error toast);
/// `false` when nothing was committed.
fn save_store_at(
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    store: &ProfileBook,
) -> bool {
    let mut loaded = crate::persistence::load_enveloped::<ProfileBook>(persist, path, STORE);
    crate::persistence::report_save(
        "Workspace profiles",
        crate::persistence::save_enveloped(
            persist,
            path,
            STORE,
            store,
            &mut loaded.gate,
            crate::persistence::SaveIntent::Explicit,
        ),
    )
    .is_some()
}

pub fn load() -> ProfileBook {
    crate::lock_util::recover(cache()).clone()
}

/// Updates the in-memory book and persists it; `false` when the write
/// failed (already reported through persistence health).
pub fn save(book: &ProfileBook) -> bool {
    if !save_store(book) {
        return false;
    }
    *crate::lock_util::recover(cache()) = book.clone();
    true
}

/// Adds or replaces `profile` and persists the book; `false` when the
/// write failed (already reported through persistence health).
pub fn upsert(profile: WorkspaceProfile) -> bool {
    let mut book = load();
    book.upsert(profile);
    save(&book)
}

#[derive(Clone, Debug)]
pub struct CaptureParams<'a> {
    pub name: String,
    pub left_root: &'a Path,
    pub right_root: &'a Path,
    pub left_filter: String,
    pub right_filter: String,
    pub durability: DurabilityProfile,
    pub name_policy: NamePolicy,
    pub symlink_policy: SymlinkPolicy,
    pub trusted_command_templates: Vec<String>,
}

pub fn capture(params: CaptureParams<'_>) -> WorkspaceProfile {
    let mut profile = WorkspaceProfile::new(
        params.name,
        params.left_root.to_path_buf(),
        params.right_root.to_path_buf(),
    );
    profile.left_filter = params.left_filter;
    profile.right_filter = params.right_filter;
    profile.durability = params.durability;
    profile.name_policy = params.name_policy;
    profile.symlink_policy = params.symlink_policy;
    profile.trusted_command_templates = params.trusted_command_templates;
    profile
}

/// Captures a profile and persists it into the book. The notice says
/// whether the write was committed, so a failed save is never reported as
/// saved; `.1` is true for a failure.
pub fn capture_and_save(params: CaptureParams<'_>) -> (String, bool) {
    save_profile(capture(params))
}

pub fn save_profile(profile: WorkspaceProfile) -> (String, bool) {
    let name = profile.name.clone();
    save_notice(&name, upsert(profile))
}

/// User-facing outcome of saving the profile `name`, and whether it is an error.
fn save_notice(name: &str, saved: bool) -> (String, bool) {
    if saved {
        (format!("Saved workspace profile \"{name}\""), false)
    } else {
        (
            format!("Could not save workspace profile \"{name}\"; see persistence health"),
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_tabs_round_trip_profile_exchange_and_reset_on_apply() {
        use crate::workspace::{ActivePanel, Workspace};
        let tmp = crate::testutil::TempDir::new();
        let home = tmp.dir("home");
        let away = tmp.dir("away");
        let mut ws = Workspace::new(home.clone(), away.clone());
        assert!(ws.set_tab_pinned(ActivePanel::Left, 0, true));
        ws.left.navigate_to(away.clone());
        let profile = ws.capture_named_profile("Pinned".into()).unwrap();
        let encoded = encode_exchange(&profile).unwrap();
        let decoded = decode_exchange(&encoded).unwrap();
        let layout = decoded.layout().unwrap().unwrap();
        assert_eq!(layout.left.entries[0].pinned_path, Some(home.clone()));
        let mut fresh = Workspace::new(away.clone(), away);
        fresh.apply_profile_tab_layout(&layout).unwrap();
        assert_eq!(fresh.left.current_path, home);
        assert!(fresh.left.entries().is_empty());
        assert!(!fresh.left.has_notify());
    }

    #[test]
    fn import_review_is_non_mutating_and_add_rechecks_draft_names_and_store() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("profiles.json");
        let mut editor = Editor::open_at(path.clone());
        let source = WorkspaceProfile::new(
            "Imported",
            temp.path().join("missing-left"),
            temp.path().join("missing-right"),
        );
        editor.exchange_text = encode_exchange(&source).unwrap();
        assert!(editor.add_reviewed_import().is_err());
        editor.review_import().unwrap();
        assert!(editor.book.profiles.is_empty());
        assert!(!path.exists());
        let reviewed = editor.exchange_text.clone();
        editor.exchange_text = editor.exchange_text.replace("Imported", "Changed");
        assert!(editor.add_reviewed_import().is_err());
        assert!(!path.exists());
        editor.exchange_text = reviewed;
        editor.add_reviewed_import().unwrap();
        assert_eq!(editor.book.get("Imported"), Some(&source));
        assert_eq!(editor.selected.as_deref(), Some("Imported"));
        let before = std::fs::read(&path).unwrap();
        editor.review_import().unwrap();
        assert!(editor.add_reviewed_import().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        editor.exchange_text = "broken".into();
        assert!(editor.review_import().is_err());
        assert!(editor.import_preview.is_none());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn rename_and_duplicate_preserve_saved_layout_and_all_policies() {
        let temp = crate::testutil::TempDir::new();
        let a = temp.dir("a");
        let b = temp.dir("b");
        let mut ws = crate::workspace::Workspace::new(a, b.clone());
        assert!(ws.new_background_tab(crate::workspace::ActivePanel::Left, b));
        let mut original = ws.capture_named_profile("Work".into()).unwrap();
        original.left_filter = "*.rs".into();
        original.trusted_command_templates = vec!["retained".into()];
        let path = temp.path().join("profiles.json");
        let mut editor = Editor::open_at(path.clone());
        let mut book = editor.book.clone();
        book.add_named(original.clone()).unwrap();
        editor.commit(book).unwrap();
        editor.selected = Some("Work".into());
        editor.rename_selected("Project".into()).unwrap();
        assert_eq!(editor.book.profiles.len(), 1);
        assert_eq!(editor.book.profiles[0].name, "Project");
        editor.duplicate_selected("Copy".into()).unwrap();
        assert_eq!(editor.selected.as_deref(), Some("Copy"));
        assert_eq!(editor.book.profiles.len(), 2);
        for row in &editor.book.profiles {
            let mut actual = row.clone();
            actual.name = "Work".into();
            assert_eq!(actual, original);
        }
        let bytes = std::fs::read(&path).unwrap();
        editor.rename_selected("Copy".into()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let before = editor.book.clone();
        assert!(editor.rename_selected("Project".into()).is_err());
        assert!(editor.duplicate_selected("Copy".into()).is_err());
        assert!(editor.rename_selected("bad\nname".into()).is_err());
        assert_eq!(editor.book, before);
        assert_eq!(editor.selected.as_deref(), Some("Copy"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(Editor::open_at(path).book, before);
    }
    #[test]
    fn stale_rename_keeps_selection_and_book_until_reload() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("profiles.json");
        let mut first = Editor::open_at(path.clone());
        let mut next = first.book.clone();
        next.add_named(WorkspaceProfile::new(
            "Work",
            temp.path().into(),
            temp.path().into(),
        ))
        .unwrap();
        first.commit(next).unwrap();
        first.selected = Some("Work".into());
        let mut stale = Editor::open_at(path.clone());
        stale.selected = Some("Work".into());
        let before = stale.book.clone();
        first.rename_selected("Project".into()).unwrap();
        assert!(stale.rename_selected("Other".into()).is_err());
        assert_eq!(stale.book, before);
        assert_eq!(stale.selected.as_deref(), Some("Work"));
        assert_eq!(Editor::open_at(path).book, first.book);
    }

    #[test]
    fn full_tab_layout_round_trips_through_atomic_store_and_rejects_partial_rows() {
        let temp = crate::testutil::TempDir::new();
        let a = temp.dir("a");
        let b = temp.dir("b");
        let mut ws = crate::workspace::Workspace::new(a, b.clone());
        assert!(ws.new_background_tab(crate::workspace::ActivePanel::Left, b));
        let profile = ws.capture_named_profile("Full".into()).unwrap();
        let path = temp.path().join("profiles.json");
        let mut editor = Editor::open_at(path.clone());
        let mut next = editor.book.clone();
        next.add_named(profile.clone()).unwrap();
        editor.commit(next).unwrap();
        let restored = Editor::open_at(path);
        assert_eq!(restored.book.get("Full"), Some(&profile));
        for field in ["active", "entries"] {
            let mut bad = profile.clone();
            bad.tab_layout["left"][field] = serde_json::Value::Null;
            assert!(bad.layout().is_err());
        }
        let mut bad = profile.clone();
        bad.tab_layout["left"]["entries"][0]["path"] = "relative".into();
        assert!(bad.layout().is_err());
        let mut bad = profile.clone();
        bad.tab_layout["left"]["entries"][0]["extra"] = "x".into();
        assert!(bad.layout().is_err());
        let mut bad = profile;
        bad.tab_layout["left"]["entries"][0]["miller_widths"] =
            serde_json::json!({"padding":"x".repeat(512*1024)});
        assert!(bad.layout().is_err());
    }

    #[test]
    fn named_add_and_replace_are_explicit_and_bounded() {
        let mut book = ProfileBook::default();
        let profile = WorkspaceProfile::new("Work", "/left".into(), "/right".into());
        book.add_named(profile.clone()).unwrap();
        assert!(book.add_named(profile.clone()).is_err());
        let mut replacement = profile.clone();
        replacement.left_filter = "*.rs".into();
        book.replace_named(replacement.clone()).unwrap();
        assert_eq!(book.get("Work"), Some(&replacement));
        for name in ["", "bad\nname"] {
            let mut bad = profile.clone();
            bad.name = name.into();
            assert!(book.add_named(bad).is_err());
        }
        let mut bad = profile.clone();
        bad.name = "x".repeat(81);
        assert!(book.add_named(bad).is_err());
        let mut bad = profile.clone();
        bad.name = "New".into();
        bad.left_root = "relative".into();
        assert!(book.add_named(bad).is_err());
        assert_eq!(book.profiles.len(), 1);
    }
    #[test]
    fn editor_commits_atomically_reopens_and_refuses_stale_replacement() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("profiles.json");
        let mut first = Editor::open_at(path.clone());
        let mut stale = Editor::open_at(path.clone());
        let mut next = first.book.clone();
        next.add_named(WorkspaceProfile::new(
            "Work",
            temp.path().into(),
            temp.path().into(),
        ))
        .unwrap();
        first.commit(next.clone()).unwrap();
        assert_eq!(Editor::open_at(path.clone()).book, next);
        let mut stale_next = stale.book.clone();
        stale_next
            .add_named(WorkspaceProfile::new(
                "Home",
                temp.path().into(),
                temp.path().into(),
            ))
            .unwrap();
        assert!(
            stale
                .commit(stale_next)
                .unwrap_err()
                .contains("changed outside")
        );
        assert!(stale.book.profiles.is_empty());
        assert_eq!(Editor::open_at(path.clone()).book, next);
        stale.reload();
        assert_eq!(stale.book, next);
        std::fs::write(&path, "broken").unwrap();
        let before = first.book.clone();
        assert!(first.commit(ProfileBook::default()).is_err());
        assert_eq!(first.book, before);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
    }

    #[test]
    fn profiles_upsert_by_name_and_persist() {
        let mut book = ProfileBook::default();
        book.upsert(WorkspaceProfile::new(
            "photos",
            PathBuf::from("/l"),
            PathBuf::from("/r"),
        ));
        book.upsert(WorkspaceProfile::new(
            "photos",
            PathBuf::from("/l2"),
            PathBuf::from("/r2"),
        ));
        assert_eq!(book.profiles.len(), 1);
        assert_eq!(book.get("photos").unwrap().left_root, PathBuf::from("/l2"));
        // Round-trip through a temp store: `save()` would overwrite the
        // developer's real workspace_profiles.json.
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("workspace_profiles.json");
        let persist = crate::persistence::FsPersist::default();
        assert!(save_store_at(&persist, &path, &book));
        assert_eq!(load_store_at(&persist, &path), book);
    }

    #[test]
    fn unreadable_store_is_reported_and_blocks_saves_visibly() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.file("store.json", "{ not json");
        let persist = crate::persistence::FsPersist::default();
        let before = crate::persistence::health_snapshot();
        assert_eq!(load_store_at(&persist, &path), ProfileBook::default());
        let after_load = crate::persistence::health_snapshot();
        assert!(after_load.unreadable_stores > before.unreadable_stores);
        assert!(!save_store_at(&persist, &path, &ProfileBook::default()));
        assert!(crate::persistence::health_snapshot().save_failures > after_load.save_failures);
    }

    #[test]
    fn failed_profile_save_is_reported_as_an_error() {
        let (message, error) = save_notice("current", false);
        assert!(error);
        assert!(message.starts_with("Could not save"), "{message}");
        let (message, error) = save_notice("current", true);
        assert!(!error);
        assert_eq!(message, "Saved workspace profile \"current\"");
    }

    #[test]
    fn missing_store_is_not_reported_as_unreadable() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("store.json");
        let persist = crate::persistence::FsPersist::default();
        assert_eq!(load_store_at(&persist, &path), ProfileBook::default());
        assert!(save_store_at(&persist, &path, &ProfileBook::default()));
    }
}
