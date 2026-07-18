//! Compact file-name presentation that keeps the identifying suffix visible.

use std::borrow::Cow;

const COMPOUND_SUFFIXES: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".d.ts", ".user.js", ".min.js", ".min.css",
];

fn suffix(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    if let Some(compound) = COMPOUND_SUFFIXES
        .iter()
        .find(|suffix| lower.ends_with(**suffix))
    {
        return name.get(name.len().saturating_sub(compound.len())..);
    }

    let extension = std::path::Path::new(name).extension()?.to_str()?;
    let suffix_len = extension.len().saturating_add(1);
    name.get(name.len().checked_sub(suffix_len)?..)
}

fn take_start(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn take_end(value: &str, count: usize) -> String {
    value
        .chars()
        .rev()
        .take(count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Truncate to at most `max_chars` Unicode scalar values. For regular files,
/// reserve space for the extension before shortening the stem. Dotfiles and
/// extensionless names retain both their beginning and ending.
pub fn truncate_preserving_extension(name: &str, max_chars: usize) -> Cow<'_, str> {
    let name_chars = name.chars().count();
    if name_chars <= max_chars {
        return Cow::Borrowed(name);
    }
    if max_chars == 0 {
        return Cow::Borrowed("");
    }
    if max_chars == 1 {
        return Cow::Borrowed("\u{2026}");
    }

    if let Some(suffix) = suffix(name) {
        let suffix_chars = suffix.chars().count();
        if suffix_chars.saturating_add(2) <= max_chars {
            let stem = &name[..name.len() - suffix.len()];
            let stem_budget = max_chars - suffix_chars - 1;
            return Cow::Owned(format!("{}\u{2026}{suffix}", take_start(stem, stem_budget)));
        }
    }

    let content = max_chars - 1;
    let start = content.div_ceil(2);
    let end = content / 2;
    Cow::Owned(format!(
        "{}\u{2026}{}",
        take_start(name, start),
        take_end(name, end)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_short_names_borrowed() {
        assert!(matches!(
            truncate_preserving_extension("notes.txt", 20),
            Cow::Borrowed("notes.txt")
        ));
    }

    #[test]
    fn preserves_regular_and_compound_extensions() {
        assert_eq!(
            truncate_preserving_extension("annual-report-final.pdf", 14),
            "annual-re\u{2026}.pdf"
        );
        assert_eq!(
            truncate_preserving_extension("backup-production-2026.tar.gz", 16),
            "backup-p\u{2026}.tar.gz"
        );
        assert_eq!(
            truncate_preserving_extension("component.generated.d.ts", 12),
            "compon\u{2026}.d.ts"
        );
    }

    #[test]
    fn dotfiles_and_unicode_keep_both_ends() {
        assert_eq!(
            truncate_preserving_extension(".gitignore", 7),
            ".gi\u{2026}ore"
        );
        let shortened = truncate_preserving_extension("отчёт-подробный.txt", 12);
        assert_eq!(shortened.chars().count(), 12);
        assert!(shortened.ends_with(".txt"));
    }

    #[test]
    fn tiny_budgets_are_stable() {
        assert_eq!(truncate_preserving_extension("long.txt", 0), "");
        assert_eq!(truncate_preserving_extension("long.txt", 1), "\u{2026}");
        assert_eq!(truncate_preserving_extension("long.txt", 2), "l\u{2026}");
    }
}
