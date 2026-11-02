//! File-pane matching: fuzzy by default, with literal and anchored name forms.

#[derive(Clone, Copy)]
pub struct LiteralPattern<'a> {
    pub needle: &'a str,
    pub start: bool,
    pub end: bool,
    pub negated: bool,
}

pub fn literal_pattern(query: &str) -> Option<LiteralPattern<'_>> {
    if let Some(needle) = query.trim_start().strip_prefix('\'') {
        return Some(LiteralPattern {
            needle,
            start: false,
            end: false,
            negated: false,
        });
    }
    let query = query.trim();
    if let Some(needle) = quoted_needle(query) {
        return Some(LiteralPattern {
            needle,
            start: false,
            end: false,
            negated: false,
        });
    }
    let negated = query.starts_with('!');
    let query = query.strip_prefix('!').unwrap_or(query);
    let start = query.starts_with('^');
    let end = query.ends_with('$');
    if !start && !end && !negated {
        return None;
    }
    let needle = if start { &query[1..] } else { query };
    let needle = if end {
        needle.strip_suffix('$').unwrap_or(needle)
    } else {
        needle
    };
    Some(LiteralPattern {
        needle,
        start,
        end,
        negated,
    })
}

pub fn normalized_query(query: &str) -> &str {
    if query.trim_start().starts_with('\'') {
        query.trim_start()
    } else {
        query.trim()
    }
}

fn literal_range(
    literal: LiteralPattern<'_>,
    candidate: &str,
    case_sensitive: bool,
) -> Option<(usize, usize)> {
    let pattern = literal
        .needle
        .chars()
        .map(|c| crate::fuzzy::fold_case(c, case_sensitive));
    let length = literal.needle.chars().count();
    let mut remaining = candidate.chars();
    let mut start = 0;
    loop {
        let mut chars = remaining.clone();
        if pattern.clone().all(|expected| {
            chars
                .next()
                .is_some_and(|actual| crate::fuzzy::fold_case(actual, case_sensitive) == expected)
        }) && (!literal.end || chars.next().is_none())
        {
            return Some((start, start + length));
        }
        if literal.start {
            return None;
        }
        remaining.next()?;
        start += 1;
    }
}

/// A fully quoted branch is literal. Quotes within it are intentionally not
/// an escape language; the existing leading-apostrophe form searches those.
fn quoted_needle(query: &str) -> Option<&str> {
    let inner = query.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('"')).then_some(inner)
}

fn valid_branch(query: &str) -> bool {
    !query.starts_with('"') || quoted_needle(query).is_some()
}

/// Standalone operators outside quoted terms. Both operators reset the term
/// boundary even when scanning for the other one, so later quotes stay literal.
fn separator(query: &str, wanted: &str) -> Option<usize> {
    let mut quoted = false;
    let mut term_start = true;
    let mut skip_until = 0;
    for (index, ch) in query.char_indices() {
        if index < skip_until {
            continue;
        }
        if quoted {
            if ch == '"' {
                quoted = false;
            }
            continue;
        }
        if term_start && ch == '"' {
            quoted = true;
            term_start = false;
            continue;
        }
        let operator = if ch == '|' {
            Some("|")
        } else if ch == '&' && query[index..].starts_with("&&") {
            Some("&&")
        } else {
            None
        };
        if let Some(operator) = operator {
            let before = query[..index].chars().next_back();
            let after = query[index + operator.len()..].chars().next();
            if before.is_none_or(char::is_whitespace) && after.is_none_or(char::is_whitespace) {
                if operator == wanted {
                    return Some(index);
                }
                skip_until = index + operator.len();
                term_start = true;
                continue;
            }
        }
        if !ch.is_whitespace() {
            term_start = false;
        }
    }
    None
}

#[derive(Clone)]
struct Parts<'a> {
    remaining: Option<&'a str>,
    operator: &'static str,
}

impl<'a> Iterator for Parts<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.remaining.take()?;
        match separator(remaining, self.operator) {
            Some(index) => {
                self.remaining = Some(&remaining[index + self.operator.len()..]);
                Some(remaining[..index].trim())
            }
            None => Some(remaining.trim()),
        }
    }
}

fn parts<'a>(query: &'a str, operator: &'static str) -> Option<Parts<'a>> {
    if query.trim_start().starts_with('\'') || separator(query, operator).is_none() {
        return None;
    }
    Some(Parts {
        remaining: Some(query),
        operator,
    })
}

fn alternatives(query: &str) -> Option<Parts<'_>> {
    parts(query, "|")
}
fn conjunctions(query: &str) -> Option<Parts<'_>> {
    parts(query, "&&")
}

fn valid_expression_branch(query: &str) -> bool {
    match conjunctions(query) {
        Some(mut terms) => terms.all(|term| !term.is_empty() && valid_branch(term)),
        None => valid_branch(query),
    }
}

fn branch_is_match(query: &str, candidate: &str, case_sensitive: bool) -> bool {
    if !valid_expression_branch(query) {
        return false;
    }
    match conjunctions(query) {
        Some(mut terms) => terms.all(|term| single_is_match(term, candidate, case_sensitive)),
        None => single_is_match(query, candidate, case_sensitive),
    }
}

fn branch_ranges(query: &str, candidate: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    if !branch_is_match(query, candidate, case_sensitive) {
        return Vec::new();
    }
    let Some(terms) = conjunctions(query) else {
        return single_ranges(query, candidate, case_sensitive);
    };
    let mut ranges: Vec<_> = terms
        .flat_map(|term| single_ranges(term, candidate, case_sensitive))
        .collect();
    ranges.sort_unstable();
    let mut write = 0;
    for read in 0..ranges.len() {
        let range = ranges[read];
        if write > 0 && range.0 <= ranges[write - 1].1 {
            ranges[write - 1].1 = ranges[write - 1].1.max(range.1);
        } else {
            ranges[write] = range;
            write += 1;
        }
    }
    ranges.truncate(write);
    ranges
}

pub fn is_fuzzy_query(query: &str) -> bool {
    !query.trim_start().starts_with('"')
        && alternatives(query).is_none()
        && conjunctions(query).is_none()
        && literal_pattern(query).is_none()
}

fn single_is_match(query: &str, candidate: &str, case_sensitive: bool) -> bool {
    if !query.trim_start().starts_with('\'') && !valid_branch(query.trim()) {
        return false;
    }
    match literal_pattern(query) {
        Some(pattern) => {
            literal_range(pattern, candidate, case_sensitive).is_some() != pattern.negated
        }
        None => {
            if case_sensitive {
                crate::fuzzy::is_match_case(query, candidate, true)
            } else {
                crate::fuzzy::is_match(query, candidate)
            }
        }
    }
}

pub fn is_match(query: &str, candidate: &str) -> bool {
    is_match_case(query, candidate, false)
}

pub fn is_match_case(query: &str, candidate: &str, case_sensitive: bool) -> bool {
    if let Some(mut branches) = alternatives(query) {
        return !branches
            .clone()
            .any(|branch| branch.is_empty() || !valid_expression_branch(branch))
            && branches.any(|branch| branch_is_match(branch, candidate, case_sensitive));
    }
    branch_is_match(query, candidate, case_sensitive)
}

pub fn matched_ranges(query: &str, candidate: &str) -> Vec<(usize, usize)> {
    matched_ranges_case(query, candidate, false)
}

pub fn matched_ranges_case(
    query: &str,
    candidate: &str,
    case_sensitive: bool,
) -> Vec<(usize, usize)> {
    if let Some(mut branches) = alternatives(query) {
        if branches
            .clone()
            .any(|branch| branch.is_empty() || !valid_expression_branch(branch))
        {
            return Vec::new();
        }
        return branches
            .find(|branch| branch_is_match(branch, candidate, case_sensitive))
            .map_or_else(Vec::new, |branch| {
                branch_ranges(branch, candidate, case_sensitive)
            });
    }
    branch_ranges(query, candidate, case_sensitive)
}

fn single_ranges(query: &str, candidate: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    if !query.trim_start().starts_with('\'') && !valid_branch(query.trim()) {
        return Vec::new();
    }
    match literal_pattern(query) {
        Some(pattern) if pattern.negated => Vec::new(),
        Some(pattern) => literal_range(pattern, candidate, case_sensitive)
            .filter(|(start, end)| end > start)
            .into_iter()
            .collect(),
        None => crate::fuzzy::score_case(query, candidate, case_sensitive)
            .map(|score| score.matched_ranges)
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_sensitive_filter_keeps_branch_highlights_in_original_coordinates() {
        assert_eq!(matched_ranges_case("^A | 'X", "a X", true), [(2, 3)]);
        assert_eq!(matched_ranges_case("^A | 'X", "a X", false), [(0, 1)]);
        assert!(!is_match_case("^A && .TXT$", "Aé.txt", true));
        assert!(is_match_case("^A && .TXT$", "Aé.txt", false));
        assert_eq!(
            matched_ranges_case("^A && .TXT$", "Aé.TXT", true),
            [(0, 1), (2, 6)]
        );
    }

    #[test]
    fn conjunctions_require_every_term_and_bind_more_tightly_than_alternatives() {
        let query = ".rs$ && !^test | .toml$";
        assert!(is_match(query, "main.rs"));
        assert!(!is_match(query, "test_main.rs"));
        assert!(is_match(query, "test.toml"));
        assert!(!is_match(query, "notes.txt"));
        assert_eq!(matched_ranges(r#""ab" && "bc""#, "abc"), [(0, 3)]);
        assert!(is_match(r#"^x && "a | b""#, "x a | b"));
        assert!(!is_match(r#"^x && "a | b""#, "a | b"));
        assert!(is_match(r#""a && b" | ^keep"#, "a && b"));
        assert!(is_match("'a && b | c", "literal a && b | c"));
        assert!(!is_match("'a && b | c", "a"));
        assert!(is_match("ab cd", "a-b c-d"));
        assert!(is_match("!x && !y", "z"));
        assert!(matched_ranges("!x && !y", "z").is_empty());
        for invalid in [
            "&&",
            ".rs$ &&",
            "&& .rs$",
            ".rs$ | .toml$ &&",
            r#".rs$ && "unfinished"#,
        ] {
            assert!(!is_match(invalid, "main.rs"));
            assert!(matched_ranges(invalid, "main.rs").is_empty());
        }
    }

    #[test]
    fn quoted_alternatives_keep_internal_pipes_spaces_and_operator_text_literal() {
        let query = r#""a | b" | .rs$"#;
        assert!(is_match(query, "prefix a | b suffix"));
        assert!(is_match(query, "main.rs"));
        assert!(!is_match(query, "a"));
        assert_eq!(matched_ranges(query, "a | b.txt"), [(0, 5)]);
        assert!(is_match(r#".rs$ | "^a$""#, "literal ^a$ text"));
        assert!(!is_match(r#".rs$ | "^a$""#, "a"));
        assert!(is_match(r#"" a ""#, "x a y"));
        assert!(!is_match(r#"" a ""#, "a"));
        assert!(is_match(r#""a | b" | "c | d""#, "c | d"));
        assert!(is_match(r#"'"unclosed"#, "literal \"unclosed"));
        assert!(is_match(r#"file" | .rs$"#, "file\".txt"));
        for invalid in [
            r#""a"#,
            r#".rs$ | "a"#,
            r#""a"b | .rs$"#,
            r#".rs$ | "a" "b""#,
        ] {
            assert!(!is_match(invalid, "main.rs"));
            assert!(matched_ranges(invalid, "main.rs").is_empty());
        }
    }

    #[test]
    fn alternatives_combine_patterns_and_highlight_the_first_matching_branch() {
        assert!(is_match(".rs$ | .toml$", "Cargo.TOML"));
        assert!(is_match(".rs$ | .toml$", "main.rs"));
        assert!(!is_match(".rs$ | .toml$", "notes.txt"));
        assert_eq!(matched_ranges(".rs$ | .toml$", "main.rs"), [(4, 7)]);
        assert!(is_match("ab | cd", "a-b.txt"));
        assert!(is_match("^é | ^ФАЙЛ", "файл.txt"));
        assert!(is_match("!backup | ^backup-keep", "backup-keep.txt"));
        assert!(!is_match("!backup | ^backup-keep", "backup-drop.txt"));
        assert!(matched_ranges("!backup | ^notes", "notes.txt").is_empty());
        assert!(is_match("'a | b", "literal a | b text"));
        assert!(!is_match("'a | b", "a"));
        assert!(is_match("x|y", "x|y"));
        assert!(!is_match("x|y", "x"));
        for query in ["|", "ab |", "| ab", "ab | | cd"] {
            assert!(!is_match(query, "ab"));
            assert!(matched_ranges(query, "ab").is_empty());
        }
    }

    #[test]
    fn negated_literal_and_anchor_rules_complement_their_positive_matches() {
        for (negative, positive) in [
            ("!backup", "'backup"),
            ("!^temp", "^temp"),
            ("!.tmp$", ".tmp$"),
            ("!^İ.txt$", "^İ.txt$"),
            ("!", "'"),
            ("!^$", "^$"),
        ] {
            for name in [
                "",
                "backup.txt",
                "BACKUP",
                "temp-a",
                "attempt",
                "a.tmp",
                "a.tmp.bak",
                "İ.txt",
                "i.txt",
            ] {
                assert_eq!(
                    is_match(negative, name),
                    !is_match(positive, name),
                    "{negative:?}, {name:?}"
                );
                assert!(matched_ranges(negative, name).is_empty());
            }
        }
        assert!(is_match("'!backup", "literal!backup.txt"));
        assert!(!is_match("'!backup", "backup.txt"));
        assert_eq!(matched_ranges("'!backup", "literal!backup.txt"), [(7, 14)]);
    }

    #[test]
    fn anchored_names_require_boundaries_and_quote_escapes_operators() {
        assert!(is_match("^ab", "AB.txt"));
        assert!(!is_match("^ab", "xAB.txt"));
        assert!(is_match(".rs$", "main.RS"));
        assert!(!is_match(".rs$", "main.rs.bak"));
        assert!(is_match("^a.b$", "A.B"));
        assert!(!is_match("^a.b$", "axb"));
        assert!(!is_match("^a$", "a\n"));
        assert!(is_match("'^ab$", "literal ^ab$ token"));
        assert!(!is_match("'^ab$", "ab"));
        assert!(is_match("^", "anything"));
        assert!(is_match("$", "anything"));
        assert!(is_match("^$", ""));
        assert!(!is_match("^$", "anything"));
        assert_eq!(matched_ranges("файл$", "файл-файл"), [(5, 9)]);
        assert_eq!(matched_ranges("^é", "Été"), [(0, 1)]);
        assert_eq!(matched_ranges("^é.txt$", "É.txt"), [(0, 5)]);
    }

    #[test]
    fn literal_prefix_preserves_spaces_punctuation_and_apostrophes() {
        assert!(is_match("ab", "a--b"));
        assert!(!is_match("'ab", "a--b"));
        assert!(is_match("'ab", "xxAByy"));
        assert!(is_match("'a b ", "a b .txt"));
        assert!(!is_match("'a b ", "a b.txt"));
        assert!(is_match("'*.rs", "literal*.rs"));
        assert!(!is_match("'*.rs", "main.rs"));
        assert!(is_match("''quoted", "'quoted.txt"));
        assert!(is_match("'", "anything"));
        assert!(matched_ranges("'", "anything").is_empty());
    }

    #[test]
    fn unicode_ranges_are_character_indices_and_match_the_predicate() {
        assert_eq!(matched_ranges("'ФАЙЛ", "é-файл.txt"), [(2, 6)]);
        assert_eq!(matched_ranges("'é", "xÉ.txt"), [(1, 2)]);
        for query in ["ab", "'ab", "'é", "'", "' ", "''", "no"] {
            for name in ["a--b", "AB.txt", "Été", "' a b ", ""] {
                let ranges = matched_ranges(query, name);
                if !ranges.is_empty() {
                    assert!(is_match(query, name));
                }
                for (start, end) in ranges {
                    assert!(start < end && end <= name.chars().count());
                }
            }
        }
    }
}
