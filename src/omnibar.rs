//! Omnibar input grammar: one field for navigation, commands, search, and
//! shell. The leading character selects the mode; the rest is the payload.
//! UI-independent so the routing contract is unit-tested.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OmnibarMode<'a> {
    /// Plain text: recent folders, pinned folders, and commands together.
    Jump(&'a str),
    /// `>`: commands only.
    Command(&'a str),
    /// `/` or `~`: a literal folder path; `./` or `../`: a path relative to
    /// the active folder (see [`resolve_relative`]).
    Path(&'a str),
    /// `?`: recursive search from the active folder.
    Search(&'a str),
    /// `!`: shell command in the active folder.
    Shell(&'a str),
}

impl<'a> OmnibarMode<'a> {
    pub(crate) fn parse(input: &'a str) -> Self {
        let trimmed = input.trim_start();
        if let Some(rest) = trimmed.strip_prefix('>') {
            Self::Command(rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix('?') {
            Self::Search(rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix('!') {
            Self::Shell(rest.trim())
        } else if trimmed.starts_with('/')
            || trimmed.starts_with('~')
            || is_relative_path(trimmed.trim_end())
        {
            Self::Path(trimmed.trim_end())
        } else {
            Self::Jump(trimmed.trim())
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Jump(_) => "Go",
            Self::Command(_) => "Command",
            Self::Path(_) => "Path",
            Self::Search(_) => "Search",
            Self::Shell(_) => "Shell",
        }
    }
}

/// `.`, `..`, or anything starting `./` or `../`: a path from the active
/// folder rather than a folder name to jump to.
fn is_relative_path(input: &str) -> bool {
    matches!(input, "." | "..") || input.starts_with("./") || input.starts_with("../")
}

/// For relative Path-mode input, the absolute path it names from `base` (the
/// active folder), with `.` and `..` resolved lexically like a shell `cd`;
/// `None` for absolute or `~` input, which the path probe handles as typed.
pub(crate) fn resolve_relative(input: &str, base: &std::path::Path) -> Option<String> {
    use std::path::Component;
    if !is_relative_path(input) {
        return None;
    }
    let mut resolved = std::path::PathBuf::new();
    for component in base.join(input).components() {
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
    Some(resolved.to_string_lossy().into_owned())
}

/// Hint row shown under an empty omnibar.
pub const GRAMMAR_HINT: &str = "folder name to jump  \u{00b7}  /, ~ or ./ path  \u{00b7}  > command  \u{00b7}  ? search  \u{00b7}  ! shell";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_select_modes() {
        assert_eq!(OmnibarMode::parse(""), OmnibarMode::Jump(""));
        assert_eq!(OmnibarMode::parse("  docs "), OmnibarMode::Jump("docs"));
        assert_eq!(OmnibarMode::parse("> copy"), OmnibarMode::Command("copy"));
        assert_eq!(OmnibarMode::parse("?*.rs"), OmnibarMode::Search("*.rs"));
        assert_eq!(OmnibarMode::parse("! ls -la"), OmnibarMode::Shell("ls -la"));
        assert_eq!(OmnibarMode::parse("~/Doc"), OmnibarMode::Path("~/Doc"));
        assert_eq!(OmnibarMode::parse("/tmp/"), OmnibarMode::Path("/tmp/"));
    }

    #[test]
    fn relative_paths_use_path_mode_from_the_active_folder() {
        assert_eq!(OmnibarMode::parse("./sub"), OmnibarMode::Path("./sub"));
        assert_eq!(OmnibarMode::parse(" ../x "), OmnibarMode::Path("../x"));
        assert_eq!(OmnibarMode::parse(".."), OmnibarMode::Path(".."));
        // Dot-names are still folder names to jump to.
        assert_eq!(OmnibarMode::parse(".config"), OmnibarMode::Jump(".config"));

        let base = std::path::Path::new("/home/me/project");
        assert_eq!(
            resolve_relative("./src/app", base).as_deref(),
            Some("/home/me/project/src/app")
        );
        assert_eq!(
            resolve_relative("../other/./x", base).as_deref(),
            Some("/home/me/other/x")
        );
        assert_eq!(resolve_relative("..", base).as_deref(), Some("/home/me"));
        assert_eq!(resolve_relative("~/Docs", base), None);
        assert_eq!(resolve_relative("/tmp", base), None);
    }

    #[test]
    fn path_keeps_inner_spaces() {
        assert_eq!(
            OmnibarMode::parse("~/My Files "),
            OmnibarMode::Path("~/My Files")
        );
    }
}
