//! Launch-argument parsing for terminal-to-GUI handoff:
//! `commander <left> [right]`.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchPaths {
    pub left: Option<PathBuf>,
    pub right: Option<PathBuf>,
    /// The file to put the cursor on in `left` (`commander ~/notes.txt`).
    pub left_focus: Option<String>,
    /// The file to put the cursor on in `right`.
    pub right_focus: Option<String>,
}

/// Takes raw `OsStr` arguments so a non-UTF-8 path cannot panic at startup.
/// Tokens starting with `-` are flags until a literal `--`; after it every
/// token is a path, so folders named like `-dir` can still be opened.
pub fn parse_launch_args<I, S>(args: I) -> LaunchPaths
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut paths = LaunchPaths::default();
    let mut skip_program = true;
    let mut flags_done = false;
    for raw in args {
        let arg = raw.as_ref();
        if skip_program {
            skip_program = false;
            continue;
        }
        if arg.is_empty() {
            continue;
        }
        if !flags_done && arg.as_encoded_bytes().first() == Some(&b'-') {
            flags_done = arg == OsStr::new("--");
            continue;
        }
        let path = PathBuf::from(arg);
        if paths.left.is_none() {
            paths.left = Some(path);
        } else if paths.right.is_none() {
            paths.right = Some(path);
            break;
        }
    }
    paths
}

/// Resolve launch arguments to folders: a folder opens as given, a file
/// opens its containing folder with the cursor on it (`commander
/// ~/notes.txt`), and a missing path is dropped with a log warning instead of
/// silently.
pub fn sanitize_launch_paths(paths: LaunchPaths) -> LaunchPaths {
    let accept = |path: Option<PathBuf>| {
        let path = path?;
        let resolved = launch_folder(&path, |p| p.is_dir(), |p| p.exists());
        if resolved.is_none() {
            log::warn!(
                target: "commander::launch",
                "Ignoring launch path that is missing or cannot be resolved: {}",
                path.display()
            );
        }
        resolved
    };
    let (left, left_focus) = accept(paths.left).unzip();
    let (right, right_focus) = accept(paths.right).unzip();
    LaunchPaths {
        left,
        right,
        left_focus: left_focus.flatten(),
        right_focus: right_focus.flatten(),
    }
}

/// The folder a launch argument opens, plus the entry to focus in it: the
/// folder itself (no focus) when `is_dir`, its parent focused on the file
/// when it `exists` as something else, `None` when it is missing.
fn launch_folder(
    path: &Path,
    is_dir: impl Fn(&Path) -> bool,
    exists: impl Fn(&Path) -> bool,
) -> Option<(PathBuf, Option<String>)> {
    let absolute = absolutize(path)?;
    if is_dir(&absolute) {
        return Some((absolute, None));
    }
    if !exists(&absolute) {
        return None;
    }
    let name = absolute.file_name()?.to_string_lossy().into_owned();
    absolute
        .parent()
        .filter(|parent| is_dir(parent))
        .map(|parent| (parent.to_path_buf(), Some(name)))
}

/// Resolves a relative launch path (`.`, `..`, `sub/dir`) against the working
/// directory like a shell `cd` would, so panels never hold a relative path
/// whose parent is empty (breaking go-up, breadcrumbs and session restore).
fn absolutize(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    resolved.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    resolved.pop();
                }
            }
            other => resolved.push(other),
        }
    }
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_left_and_optional_right() {
        assert_eq!(
            parse_launch_args(["commander", "/tmp/a", "/tmp/b"]),
            LaunchPaths {
                left: Some(PathBuf::from("/tmp/a")),
                right: Some(PathBuf::from("/tmp/b")),
                ..LaunchPaths::default()
            }
        );
        assert_eq!(
            parse_launch_args(["commander", "/tmp/only"]),
            LaunchPaths {
                left: Some(PathBuf::from("/tmp/only")),
                right: None,
                ..LaunchPaths::default()
            }
        );
    }

    #[test]
    fn skips_flags_and_empty_tokens() {
        assert_eq!(
            parse_launch_args(["commander", "--foo", "", "/left", "-v", "/right"]),
            LaunchPaths {
                left: Some(PathBuf::from("/left")),
                right: Some(PathBuf::from("/right")),
                ..LaunchPaths::default()
            }
        );
    }

    #[test]
    fn double_dash_ends_flag_parsing() {
        assert_eq!(
            parse_launch_args(["commander", "-v", "--", "-left", "--right"]),
            LaunchPaths {
                left: Some(PathBuf::from("-left")),
                right: Some(PathBuf::from("--right")),
                ..LaunchPaths::default()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepts_non_utf8_arguments() {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"/tmp/\xff");
        assert_eq!(
            parse_launch_args([OsStr::new("commander"), raw]).left,
            Some(PathBuf::from(raw))
        );
    }

    #[test]
    fn file_arguments_open_their_folder_and_missing_ones_are_dropped() {
        let root = crate::testutil::TempDir::new();
        let file = root.file("notes.txt", "n");
        let sanitized = sanitize_launch_paths(LaunchPaths {
            left: Some(file),
            right: Some(root.path().join("missing.txt")),
            ..LaunchPaths::default()
        });
        assert_eq!(sanitized.left.as_deref(), Some(root.path()));
        assert_eq!(sanitized.left_focus.as_deref(), Some("notes.txt"));
        assert_eq!(sanitized.right, None);
        assert_eq!(sanitized.right_focus, None);
        let folder = sanitize_launch_paths(LaunchPaths {
            left: Some(root.path().to_path_buf()),
            ..LaunchPaths::default()
        });
        assert_eq!(folder.left.as_deref(), Some(root.path()));
        assert_eq!(folder.left_focus, None, "a folder argument focuses nothing");
    }

    #[test]
    fn relative_launch_paths_become_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let sanitized = sanitize_launch_paths(LaunchPaths {
            left: Some(PathBuf::from(".")),
            right: Some(PathBuf::from("..")),
            ..LaunchPaths::default()
        });
        assert_eq!(sanitized.left.as_deref(), Some(cwd.as_path()));
        assert_eq!(sanitized.right.as_deref(), cwd.parent());
    }
}
