//! Omnibar input grammar: one field for navigation, commands, search, and
//! shell. The leading character selects the mode; the rest is the payload.
//! UI-independent so the routing contract is unit-tested.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OmnibarMode<'a> {
    /// Plain text: recent folders, pinned folders, and commands together.
    Jump(&'a str),
    /// `>`: commands only.
    Command(&'a str),
    /// `/` or `~`: a literal folder path.
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
        } else if trimmed.starts_with('/') || trimmed.starts_with('~') {
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

/// Hint row shown under an empty omnibar.
pub(crate) const GRAMMAR_HINT: &str = "folder name to jump  \u{00b7}  / or ~ path  \u{00b7}  > command  \u{00b7}  ? search  \u{00b7}  ! shell";

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
    fn path_keeps_inner_spaces() {
        assert_eq!(
            OmnibarMode::parse("~/My Files "),
            OmnibarMode::Path("~/My Files")
        );
    }
}
