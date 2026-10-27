//! Literal basename lists for selection: bounded, exact and without filesystem I/O.
use std::collections::HashSet;

pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_NAMES: usize = 4096;

pub fn parse(text: &str) -> Result<HashSet<&str>, String> {
    if text.len() > MAX_BYTES {
        return Err("Name lists are limited to 64 KiB".into());
    }
    let mut names = HashSet::new();
    let mut count = 0;
    for (index, line) in text.split('\n').enumerate() {
        let name = line.strip_suffix('\r').unwrap_or(line);
        if name.is_empty() {
            continue;
        }
        count += 1;
        if count > MAX_NAMES {
            return Err("Name lists are limited to 4096 nonempty lines".into());
        }
        if matches!(name, "." | "..") || name.contains(['/', '\0', '\r']) {
            return Err(format!(
                "Line {}: enter a filename, without a path or control characters",
                index + 1
            ));
        }
        names.insert(name);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_names_preserve_spaces_globs_unicode_and_deduplicate() {
        let names = parse("a.txt\r\n\n a.txt \n*.rs\n!keep\né\ne\u{301}\na.txt\n").unwrap();
        assert_eq!(names.len(), 6);
        for name in ["a.txt", " a.txt ", "*.rs", "!keep", "é", "e\u{301}"] {
            assert!(names.contains(name));
        }
        assert!(!names.contains("A.txt"));
        assert!(parse("").unwrap().is_empty());
    }
    #[test]
    fn invalid_or_oversized_lists_fail_instead_of_truncating() {
        for bad in [
            "ok\n../bad",
            "ok\n/path",
            "ok\n.",
            "ok\n..",
            "ok\nbad\0name",
            "bad\rname",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse(&"a".repeat(MAX_BYTES)).is_ok());
        assert!(parse(&"é".repeat(MAX_BYTES / 2 + 1)).is_err());
        assert!(parse(&"x\n".repeat(MAX_NAMES)).is_ok());
        assert!(parse(&"x\n".repeat(MAX_NAMES + 1)).is_err());
    }
}
