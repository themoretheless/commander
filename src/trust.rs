//! Per-root trust labels that gate automatic archive inspection and related
//! privileged actions (research **J003**).
//!
//! Full label assignment is not shipped yet: every root currently resolves to
//! [`TrustLabel::Trusted`] so existing browse/search behaviour is unchanged.
//! Call sites already consult these hooks so J003 can land without rewriting
//! discovery/open paths.

use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrustLabel {
    #[default]
    Trusted,
    Restricted,
    Untrusted,
}

impl TrustLabel {
    pub const fn allows_auto_archive_inspect(self) -> bool {
        matches!(self, Self::Trusted)
    }

    pub const fn allows_run_command(self) -> bool {
        !matches!(self, Self::Untrusted)
    }
}

pub fn label_for(path: &Path) -> TrustLabel {
    let _ = path;
    TrustLabel::Trusted
}

pub fn allows_auto_archive_inspect(path: &Path) -> bool {
    label_for(path).allows_auto_archive_inspect()
}

pub fn allows_run_command(path: &Path) -> bool {
    label_for(path).allows_run_command()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_roots_are_trusted_until_j003_lands() {
        let path = PathBuf::from("/tmp/example.zip");
        assert_eq!(label_for(&path), TrustLabel::Trusted);
        assert!(allows_auto_archive_inspect(&path));
        assert!(allows_run_command(&path));
    }

    #[test]
    fn restricted_blocks_auto_inspect_but_not_run_command() {
        assert!(!TrustLabel::Restricted.allows_auto_archive_inspect());
        assert!(TrustLabel::Restricted.allows_run_command());
        assert!(!TrustLabel::Untrusted.allows_run_command());
    }
}
