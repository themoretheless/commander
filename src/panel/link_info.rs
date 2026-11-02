//! Link observations captured with directory enumeration, never by card rendering.
use std::{
    fs, io,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTargetState {
    Available,
    Missing,
    Unavailable(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkInfo {
    pub target: Result<PathBuf, String>,
    pub state: LinkTargetState,
}
impl LinkInfo {
    pub(super) fn observe(path: &Path, followed: &io::Result<fs::Metadata>) -> Self {
        Self {
            target: fs::read_link(path).map_err(|error| error.to_string()),
            state: match followed {
                Ok(_) => LinkTargetState::Available,
                Err(error) if crate::fs_util::missing_link_target_error(error.kind()) => {
                    LinkTargetState::Missing
                }
                Err(error) => LinkTargetState::Unavailable(error.to_string()),
            },
        }
    }
    pub fn status(&self) -> String {
        let mut text = match &self.state {
            LinkTargetState::Available => "Target exists".into(),
            LinkTargetState::Missing => "Target missing".into(),
            LinkTargetState::Unavailable(error) => format!("Target unavailable: {error}"),
        };
        if let Err(error) = &self.target {
            text.push_str(&format!(" · Link text unavailable: {error}"));
        }
        text
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn missing_and_unavailable_errors_remain_distinct_in_the_cached_card() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.path().join("link");
        std::os::unix::fs::symlink("absent", &path).unwrap();
        let missing = LinkInfo::observe(&path, &Err(io::Error::from(io::ErrorKind::NotFound)));
        assert_eq!(missing.state, LinkTargetState::Missing);
        assert_eq!(missing.target, Ok(PathBuf::from("absent")));
        let denied = LinkInfo::observe(
            &path,
            &Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        );
        assert!(matches!(denied.state, LinkTargetState::Unavailable(_)));
        assert!(denied.status().starts_with("Target unavailable:"));
    }
}

#[cfg(all(test, unix))]
mod card_tests {
    use super::*;
    use crate::panel::{FileEntry, make_info};
    #[test]
    fn cards_keep_captured_link_text_and_status_until_a_new_observation() {
        let dir = crate::testutil::TempDir::new();
        dir.file("first", "a");
        dir.file("second", "bb");
        let path = dir.path().join("link");
        std::os::unix::fs::symlink("first", &path).unwrap();
        let entry = FileEntry::observe_lexical(path.clone()).unwrap();
        let before = make_info(&entry, None, None);
        assert!(before.path.ends_with("-> first"));
        assert_eq!(before.link_status.as_deref(), Some("Target exists"));
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("second", &path).unwrap();
        assert_eq!(make_info(&entry, None, None), before);
        let fresh = FileEntry::observe_lexical(path.clone()).unwrap();
        assert!(make_info(&fresh, None, None).path.ends_with("-> second"));
        std::fs::remove_file(dir.path().join("second")).unwrap();
        let broken = FileEntry::observe_lexical(path.clone()).unwrap();
        let card = make_info(&broken, None, None);
        assert_eq!(card.kind, "Broken symlink");
        assert_eq!(card.size, "Unavailable");
        assert_eq!(card.link_status.as_deref(), Some("Target missing"));
        std::fs::remove_file(path).unwrap();
        assert_eq!(make_info(&broken, None, None), card);
    }
    #[test]
    fn inaccessible_and_loop_targets_are_not_misreported_as_missing() {
        let dir = crate::testutil::TempDir::new();
        dir.file("target", "a");
        let path = dir.path().join("link");
        std::os::unix::fs::symlink("target", &path).unwrap();
        let mut entry = FileEntry::observe_lexical(path.clone()).unwrap();
        entry.permissions = None;
        entry.symlink = Some(std::sync::Arc::new(LinkInfo::observe(
            &path,
            &Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        )));
        let card = make_info(&entry, None, None);
        assert_eq!(card.kind, "Symlink (target unavailable)");
        assert!(card.link_status.unwrap().starts_with("Target unavailable:"));
        let path = dir.path().join("loop");
        std::os::unix::fs::symlink("loop", &path).unwrap();
        let entry = FileEntry::observe_lexical(path).unwrap();
        let card = make_info(&entry, None, None);
        assert_eq!(card.kind, "Symlink (target unavailable)");
        assert!(card.link_status.unwrap().starts_with("Target unavailable:"));
    }
}
