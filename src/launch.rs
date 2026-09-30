//! Launch-argument parsing for terminal-to-GUI handoff:
//! `commander <left> [right]`.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchPaths {
    pub left: Option<PathBuf>,
    pub right: Option<PathBuf>,
}

pub fn parse_launch_args<I, S>(args: I) -> LaunchPaths
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut paths = LaunchPaths::default();
    let mut skip_program = true;
    for raw in args {
        let arg = raw.as_ref();
        if skip_program {
            skip_program = false;
            continue;
        }
        if arg.is_empty() || arg.starts_with('-') {
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
    LaunchPaths {
        left: paths.left.filter(|path| Path::new(path).is_dir()),
        right: paths.right.filter(|path| Path::new(path).is_dir()),
    }
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
}
