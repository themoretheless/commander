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
}

impl fmt::Display for DirInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "Path is empty",
            Self::Missing => "Path does not exist",
            Self::NotDirectory => "Not a folder",
        })
    }
}

impl std::error::Error for DirInputError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewNameError {
    Empty,
    ContainsSlash,
    DotEntry,
    Collision,
}

impl fmt::Display for NewNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "Name cannot be empty",
            Self::ContainsSlash => "Name cannot contain '/'",
            Self::DotEntry => "Invalid name",
            Self::Collision => "Name already in use",
        })
    }
}

impl std::error::Error for NewNameError {}

/// Resolve go-to-path input without canonicalizing it.
pub(crate) fn resolve_dir_input(input: &str, home: &Path) -> Result<PathBuf, DirInputError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DirInputError::Empty);
    }
    let expanded = if trimmed == "~" {
        home.to_path_buf()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if !expanded.exists() {
        return Err(DirInputError::Missing);
    }
    if !expanded.is_dir() {
        return Err(DirInputError::NotDirectory);
    }
    Ok(expanded)
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
            resolve_dir_input("~", home.path()),
            Ok(home.path().to_path_buf())
        );
        assert_eq!(resolve_dir_input("~/Documents", home.path()), Ok(documents));
        assert_eq!(
            resolve_dir_input("  ~/Documents  ", home.path()),
            Ok(home.path().join("Documents"))
        );
    }

    #[test]
    fn dir_input_preserves_lexical_relative_paths() {
        assert_eq!(
            resolve_dir_input(" . ", Path::new("/unused")),
            Ok(".".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn dir_input_accepts_directory_symlink_without_canonicalizing() {
        let root = TempDir::new();
        let target = root.dir("target");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(
            resolve_dir_input(link.to_str().unwrap(), root.path()),
            Ok(link)
        );
    }

    #[test]
    fn dir_input_distinguishes_empty_missing_and_non_directory() {
        let root = TempDir::new();
        let file = root.file("note.txt", "x");

        assert_eq!(
            resolve_dir_input("   ", root.path()),
            Err(DirInputError::Empty)
        );
        assert_eq!(
            resolve_dir_input(root.path().join("missing").to_str().unwrap(), root.path()),
            Err(DirInputError::Missing)
        );
        assert_eq!(
            resolve_dir_input(file.to_str().unwrap(), root.path()),
            Err(DirInputError::NotDirectory)
        );
    }

    #[test]
    fn directory_error_copy_remains_stable() {
        assert_eq!(DirInputError::Empty.to_string(), "Path is empty");
        assert_eq!(DirInputError::Missing.to_string(), "Path does not exist");
        assert_eq!(DirInputError::NotDirectory.to_string(), "Not a folder");
    }

    #[test]
    fn new_name_rejects_whitespace_slash_dot_entries_and_exact_collision() {
        let siblings = vec!["taken.txt".to_string()];

        assert_eq!(validate_new_name("  ", &siblings), Err(NewNameError::Empty));
        assert_eq!(
            validate_new_name("a/b", &siblings),
            Err(NewNameError::ContainsSlash)
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
        assert_eq!(NewNameError::DotEntry.to_string(), "Invalid name");
        assert_eq!(NewNameError::Collision.to_string(), "Name already in use");
    }
}
