//! Path input and filename validation shared by the UI and workspace core.
//!
//! These helpers deliberately preserve lexical paths. Filesystem naming
//! policy differs by volume, so case folding and Unicode normalization belong
//! to a future capability-aware layer rather than this validator.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirInputError {
    Empty,
    Missing,
    NotDirectory,
    Unavailable,
    UnsupportedTilde,
}

impl fmt::Display for DirInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "Path is empty",
            Self::Missing => "Path does not exist",
            Self::NotDirectory => "Not a folder",
            Self::Unavailable => "Path cannot be accessed",
            Self::UnsupportedTilde => "~user expansion is not supported",
        })
    }
}

impl std::error::Error for DirInputError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewNameError {
    Empty,
    ContainsSlash,
    ContainsNul,
    DotEntry,
    Collision,
}

impl fmt::Display for NewNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "Name cannot be empty",
            Self::ContainsSlash => "Name cannot contain '/'",
            Self::ContainsNul => "Name cannot contain NUL",
            Self::DotEntry => "Invalid name",
            Self::Collision => "Name already in use",
        })
    }
}

impl std::error::Error for NewNameError {}

/// Parse go-to-path input without touching the filesystem or canonicalizing it.
pub(crate) fn parse_dir_input(input: &str, home: &Path) -> Result<PathBuf, DirInputError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DirInputError::Empty);
    }
    if trimmed.contains('\0') {
        return Err(DirInputError::Unavailable);
    }
    if trimmed.starts_with('~') && trimmed != "~" && !trimmed.starts_with("~/") {
        return Err(DirInputError::UnsupportedTilde);
    }
    let expanded = if trimmed == "~" {
        home.to_path_buf()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    Ok(expanded)
}

pub(crate) trait DirectoryProbePort: Send + Sync {
    fn probe(&self, path: &Path) -> Result<(), DirInputError>;
}

pub(crate) struct FsDirectoryProbe;

impl DirectoryProbePort for FsDirectoryProbe {
    fn probe(&self, path: &Path) -> Result<(), DirInputError> {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(DirInputError::NotDirectory),
            Err(error) => {
                return Err(match error.kind() {
                    std::io::ErrorKind::NotFound => DirInputError::Missing,
                    std::io::ErrorKind::NotADirectory => DirInputError::NotDirectory,
                    _ => DirInputError::Unavailable,
                });
            }
        }
        Ok(())
    }
}

/// Validate a proposed basename against an exact sibling-name snapshot.
///
/// `siblings` must exclude the entry being renamed. The commit path performs
/// the same validation against a fresh directory listing before its atomic
/// no-replace rename.
pub(crate) fn validate_new_name(name: &str, siblings: &[String]) -> Result<(), NewNameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NewNameError::Empty);
    }
    if name.contains('/') {
        return Err(NewNameError::ContainsSlash);
    }
    if name.contains('\0') {
        return Err(NewNameError::ContainsNul);
    }
    if name == "." || name == ".." {
        return Err(NewNameError::DotEntry);
    }
    if siblings.iter().any(|sibling| sibling == name) {
        return Err(NewNameError::Collision);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn dir_input_expands_only_exact_tilde_forms() {
        let home = TempDir::new();
        let documents = home.dir("Documents");

        assert_eq!(
            parse_dir_input("~", home.path()),
            Ok(home.path().to_path_buf())
        );
        assert_eq!(parse_dir_input("~/Documents", home.path()), Ok(documents));
        assert_eq!(
            parse_dir_input("  ~/Documents  ", home.path()),
            Ok(home.path().join("Documents"))
        );
    }

    #[test]
    fn dir_input_reports_unsupported_tilde_user_forms() {
        let home = TempDir::new();

        assert_eq!(
            parse_dir_input("~alice", home.path()),
            Err(DirInputError::UnsupportedTilde)
        );
        assert_eq!(
            parse_dir_input("~alice/docs", home.path()),
            Err(DirInputError::UnsupportedTilde)
        );
    }

    #[test]
    fn dir_input_preserves_lexical_relative_paths() {
        assert_eq!(parse_dir_input(" . ", Path::new("/unused")), Ok(".".into()));
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_probe_accepts_directory_symlink_without_canonicalizing() {
        let root = TempDir::new();
        let target = root.dir("target");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(
            parse_dir_input(link.to_str().unwrap(), root.path()),
            Ok(link.clone())
        );
        assert_eq!(FsDirectoryProbe.probe(&link), Ok(()));
    }

    #[test]
    fn parser_and_probe_distinguish_empty_missing_and_non_directory() {
        let root = TempDir::new();
        let file = root.file("note.txt", "x");
        let missing = root.path().join("missing");

        assert_eq!(
            parse_dir_input("   ", root.path()),
            Err(DirInputError::Empty)
        );
        assert_eq!(
            FsDirectoryProbe.probe(&missing),
            Err(DirInputError::Missing)
        );
        assert_eq!(
            FsDirectoryProbe.probe(&file),
            Err(DirInputError::NotDirectory)
        );
        assert_eq!(FsDirectoryProbe.probe(root.path()), Ok(()));
    }

    #[test]
    fn directory_error_copy_remains_stable() {
        assert_eq!(DirInputError::Empty.to_string(), "Path is empty");
        assert_eq!(DirInputError::Missing.to_string(), "Path does not exist");
        assert_eq!(DirInputError::NotDirectory.to_string(), "Not a folder");
        assert_eq!(
            DirInputError::Unavailable.to_string(),
            "Path cannot be accessed"
        );
        assert_eq!(
            DirInputError::UnsupportedTilde.to_string(),
            "~user expansion is not supported"
        );
    }

    #[test]
    fn new_name_rejects_whitespace_separators_nul_dot_entries_and_exact_collision() {
        let siblings = vec!["taken.txt".to_string()];

        assert_eq!(validate_new_name("  ", &siblings), Err(NewNameError::Empty));
        assert_eq!(
            validate_new_name("a/b", &siblings),
            Err(NewNameError::ContainsSlash)
        );
        assert_eq!(
            validate_new_name("a\0b", &siblings),
            Err(NewNameError::ContainsNul)
        );
        assert_eq!(
            validate_new_name(".", &siblings),
            Err(NewNameError::DotEntry)
        );
        assert_eq!(
            validate_new_name("..", &siblings),
            Err(NewNameError::DotEntry)
        );
        assert_eq!(
            validate_new_name("taken.txt", &siblings),
            Err(NewNameError::Collision)
        );
        assert_eq!(validate_new_name(" fresh.txt ", &siblings), Ok(()));
        assert_eq!(validate_new_name("line\nbreak", &siblings), Ok(()));
    }

    #[test]
    fn new_name_keeps_case_and_unicode_normalization_distinct() {
        let siblings = vec!["Readme.md".to_string(), "\u{e9}.txt".to_string()];

        assert_eq!(validate_new_name("README.md", &siblings), Ok(()));
        assert_eq!(validate_new_name("e\u{301}.txt", &siblings), Ok(()));
    }

    #[test]
    fn new_name_error_copy_remains_stable() {
        assert_eq!(NewNameError::Empty.to_string(), "Name cannot be empty");
        assert_eq!(
            NewNameError::ContainsSlash.to_string(),
            "Name cannot contain '/'"
        );
        assert_eq!(
            NewNameError::ContainsNul.to_string(),
            "Name cannot contain NUL"
        );
        assert_eq!(NewNameError::DotEntry.to_string(), "Invalid name");
        assert_eq!(NewNameError::Collision.to_string(), "Name already in use");
    }

    #[test]
    fn invalid_path_encoding_is_not_reported_as_missing() {
        assert_eq!(
            parse_dir_input("\0", Path::new("/unused")),
            Err(DirInputError::Unavailable)
        );
        assert_eq!(
            FsDirectoryProbe.probe(Path::new("\0")),
            Err(DirInputError::Unavailable)
        );
    }
}
