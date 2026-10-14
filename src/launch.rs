//! Launch-argument parsing for terminal-to-GUI handoff:
//! `commander <left> [right]`.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchPaths {
    pub left: Option<PathBuf>,
    pub right: Option<PathBuf>,
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

pub fn sanitize_launch_paths(paths: LaunchPaths) -> LaunchPaths {
    let accept = |path: PathBuf| absolutize(&path).filter(|path| path.is_dir());
    LaunchPaths {
        left: paths.left.and_then(accept),
        right: paths.right.and_then(accept),
    }
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
            }
        );
        assert_eq!(
            parse_launch_args(["commander", "/tmp/only"]),
            LaunchPaths {
                left: Some(PathBuf::from("/tmp/only")),
                right: None,
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
    fn relative_launch_paths_become_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let sanitized = sanitize_launch_paths(LaunchPaths {
            left: Some(PathBuf::from(".")),
            right: Some(PathBuf::from("..")),
        });
        assert_eq!(sanitized.left.as_deref(), Some(cwd.as_path()));
        assert_eq!(sanitized.right.as_deref(), cwd.parent());
    }
}
