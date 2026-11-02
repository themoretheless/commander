//! Pure line-level text diff via a longest-common-subsequence backtrack.
//! Deterministic and free of I/O, so the edit script is unit-tested directly.

/// Upper bound for the LCS matrix. At four bytes per cell this keeps the core
/// allocation near 32 MiB and, just as importantly, caps quadratic CPU work.
const MAX_DIFF_CELLS: usize = 8_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffKind {
    Equal,
    Insert,
    Delete,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DiffTooLarge;

/// Comparison keys only; display rows always retain the original operand text.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize)]
pub struct Options {
    pub trailing_spaces: bool,
    pub space_changes: bool,
    pub all_spaces: bool,
    pub expand_tabs: bool,
    pub ignore_crlf: bool,
    pub ignore_case: bool,
}
impl Options {
    fn ending(self, ending: Ending) -> Ending {
        if self.ignore_crlf && ending == Ending::CrLf {
            Ending::Lf
        } else {
            ending
        }
    }
    fn key(self, text: &str) -> std::borrow::Cow<'_, str> {
        let text = if self.trailing_spaces {
            text.trim_end_matches([' ', '\t'])
        } else {
            text
        };
        if !self.space_changes && !self.all_spaces && !self.ignore_case && !self.expand_tabs {
            return std::borrow::Cow::Borrowed(text);
        }
        // Tab stops count source Unicode scalar values, not display-cell widths.
        // Expansion precedes case folding and space policies.
        let expanded;
        let text = if self.expand_tabs && !self.all_spaces && !self.space_changes {
            expanded = expand_tabs(text);
            expanded.as_str()
        } else {
            text
        };
        let mut key = String::with_capacity(text.len());
        let mut blank = false;
        for ch in text.chars() {
            if self.all_spaces && matches!(ch, ' ' | '\t') {
                continue;
            }
            if self.space_changes && matches!(ch, ' ' | '\t') {
                if !blank {
                    key.push(' ');
                }
                blank = true;
            } else {
                blank = false;
                if self.ignore_case {
                    key.extend(ch.to_lowercase());
                } else {
                    key.push(ch);
                }
            }
        }
        std::borrow::Cow::Owned(key)
    }
}

/// Expand to fixed eight-character tab stops, independently for each source line.
fn expand_tabs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for ch in text.chars() {
        if ch == '\t' {
            let spaces = 8 - column;
            out.extend(std::iter::repeat_n(' ', spaces));
            column = 0;
        } else {
            out.push(ch);
            column = (column + 1) % 8;
        }
    }
    out
}

/// How a line ended in its file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ending {
    Lf,
    CrLf,
    /// The last line, with no newline after it.
    Eof,
}

/// Marker appended to a changed line that ends in CRLF while the other file
/// uses a different line-ending convention.
const CR_MARKER: &str = "\u{240d}";
/// Marker appended to a changed last line that lacks a final newline while
/// the other file's last line has one.
const NO_EOL_MARKER: &str = "  \\ no newline at end of file";

/// Split into (content, ending) pairs, keeping what `str::lines` drops.
fn split_lines(text: &str) -> Vec<(&str, Ending)> {
    text.split_inclusive('\n')
        .map(|line| {
            if let Some(content) = line.strip_suffix("\r\n") {
                (content, Ending::CrLf)
            } else if let Some(content) = line.strip_suffix('\n') {
                (content, Ending::Lf)
            } else {
                (line, Ending::Eof)
            }
        })
        .collect()
}

/// Diff `a` against `b` line by line. Equal lines are shared context; lines
/// only in `a` are `Delete`, lines only in `b` are `Insert`. A changed line
/// surfaces as a `Delete` of the old text followed by an `Insert` of the new.
///
/// Line endings are part of a line's identity, so files that differ only in
/// CRLF vs LF or in the final newline do not diff as identical; such lines
/// carry a visible marker since their text alone looks the same.
pub fn diff_lines(a: &str, b: &str) -> Result<Vec<DiffLine>, DiffTooLarge> {
    diff_lines_with_options(a, b, Options::default())
}

/// Space changes collapse runs of ASCII spaces/tabs to one space (not zero).
/// Tab expansion uses eight-character stops before case mapping.
/// All spaces removes ASCII spaces/tabs entirely and takes precedence over runs.
/// Case uses Unicode lowercase, without normalization or full case folding.
/// CRLF and LF remain distinct unless `ignore_crlf` is set; EOF stays exact. A matched context row displays the original `a` text.
pub fn diff_lines_with_options(
    a: &str,
    b: &str,
    options: Options,
) -> Result<Vec<DiffLine>, DiffTooLarge> {
    let a = split_lines(a);
    let b = split_lines(b);
    // Normalize each line once, never inside the quadratic LCS comparison.
    let keys = (options != Options::default()).then(|| {
        let ka: Vec<_> = a
            .iter()
            .map(|(s, e)| (options.key(s), options.ending(*e)))
            .collect();
        let kb: Vec<_> = b
            .iter()
            .map(|(s, e)| (options.key(s), options.ending(*e)))
            .collect();
        (ka, kb)
    });
    let equal = |i: usize, j: usize| match &keys {
        Some((ka, kb)) => ka[i] == kb[j],
        None => a[i] == b[j],
    };
    let (n, m) = (a.len(), b.len());
    let has = |lines: &[(&str, Ending)], ending| lines.iter().any(|l| l.1 == ending);
    // With normalized keys, text differences may be hidden. Always expose CRLF
    // on changed rows then, including mixed-ending files on both sides.
    let mark_cr = options != Options::default()
        || (has(&a, Ending::CrLf), has(&a, Ending::Lf))
            != (has(&b, Ending::CrLf), has(&b, Ending::Lf));
    let mark_eof = !a.is_empty() && !b.is_empty() && has(&a, Ending::Eof) != has(&b, Ending::Eof);

    // Shared leading and trailing lines are always part of the LCS, so only
    // the differing middle needs the quadratic matrix: two long files that
    // differ in one line stay well under the cap instead of being refused.
    let prefix = (0..n.min(m)).take_while(|&i| equal(i, i)).count();
    let suffix = (0..(n - prefix).min(m - prefix))
        .take_while(|&i| equal(n - 1 - i, m - 1 - i))
        .count();
    let (a_mid, b_mid) = (&a[prefix..n - suffix], &b[prefix..m - suffix]);
    let (n_mid, m_mid) = (a_mid.len(), b_mid.len());

    let rows = n_mid.checked_add(1).ok_or(DiffTooLarge)?;
    let cols = m_mid.checked_add(1).ok_or(DiffTooLarge)?;
    let cells = rows.checked_mul(cols).ok_or(DiffTooLarge)?;
    if cells > MAX_DIFF_CELLS {
        return Err(DiffTooLarge);
    }

    // dp[i][j] = LCS length of a_mid[i..] and b_mid[j..].
    let mut dp = vec![0u32; cells];
    let at = |i: usize, j: usize| i * cols + j;
    for i in (0..n_mid).rev() {
        for j in (0..m_mid).rev() {
            dp[at(i, j)] = if equal(prefix + i, prefix + j) {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }

    let mut ops: Vec<(DiffKind, (&str, Ending))> = Vec::with_capacity(n.max(m));
    ops.extend(a[..prefix].iter().map(|&line| (DiffKind::Equal, line)));
    let (mut i, mut j) = (0usize, 0usize);
    while i < n_mid && j < m_mid {
        if equal(prefix + i, prefix + j) {
            ops.push((DiffKind::Equal, a_mid[i]));
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            ops.push((DiffKind::Delete, a_mid[i]));
            i += 1;
        } else {
            ops.push((DiffKind::Insert, b_mid[j]));
            j += 1;
        }
    }
    ops.extend(a_mid[i..].iter().map(|&line| (DiffKind::Delete, line)));
    ops.extend(b_mid[j..].iter().map(|&line| (DiffKind::Insert, line)));
    ops.extend(a[n - suffix..].iter().map(|&line| (DiffKind::Equal, line)));
    Ok(render(&ops, mark_cr, mark_eof))
}

/// Turn the edit script into display lines, adding ending markers.
///
/// A changed CRLF line is marked when the files' conventions differ overall,
/// or when the same hunk holds the same text on the other side with another
/// ending: with mixed endings in both files, that is the only visible clue.
fn render(ops: &[(DiffKind, (&str, Ending))], mark_cr: bool, mark_eof: bool) -> Vec<DiffLine> {
    let mut out = Vec::with_capacity(ops.len());
    let mut start = 0;
    while start < ops.len() {
        if ops[start].0 == DiffKind::Equal {
            out.push(DiffLine {
                kind: DiffKind::Equal,
                text: ops[start].1.0.to_string(),
            });
            start += 1;
            continue;
        }
        let end = ops[start..]
            .iter()
            .position(|op| op.0 == DiffKind::Equal)
            .map_or(ops.len(), |k| start + k);
        let hunk = &ops[start..end];
        for &(kind, (content, ending)) in hunk {
            let crlf_vs_other = || {
                hunk.iter()
                    .any(|&(k, (c, e))| k != kind && c == content && e != Ending::CrLf)
            };
            let text = match ending {
                Ending::CrLf if mark_cr || crlf_vs_other() => format!("{content}{CR_MARKER}"),
                Ending::Eof if mark_eof => format!("{content}{NO_EOL_MARKER}"),
                _ => content.to_string(),
            };
            out.push(DiffLine { kind, text });
        }
        start = end;
    }
    out
}

/// Count of inserted and deleted lines, for a header summary.
pub fn change_counts(lines: &[DiffLine]) -> (usize, usize) {
    lines
        .iter()
        .fold((0, 0), |(ins, del), line| match line.kind {
            DiffKind::Insert => (ins + 1, del),
            DiffKind::Delete => (ins, del + 1),
            DiffKind::Equal => (ins, del),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lines: &[DiffLine]) -> Vec<DiffKind> {
        lines.iter().map(|l| l.kind).collect()
    }

    #[test]
    fn trailing_spaces_preserves_interior_leading_endings_and_original_rows() {
        let options = Options {
            trailing_spaces: true,
            ..Default::default()
        };
        let lines = diff_lines_with_options(" a  b \t\n", " a  b\n", options).unwrap();
        assert_eq!(change_counts(&lines), (0, 0));
        assert_eq!(lines[0].text, " a  b \t");
        for (a, b) in [
            (" a\n", "a\n"),
            ("a  b\n", "a b\n"),
            ("a\r\n", "a\n"),
            ("a ", "a\n"),
            ("a\u{a0}\n", "a\n"),
        ] {
            assert_ne!(
                change_counts(&diff_lines_with_options(a, b, options).unwrap()),
                (0, 0)
            );
        }
        assert_eq!(
            change_counts(
                &diff_lines_with_options(
                    "A \t\r\n",
                    "a\n",
                    Options {
                        ignore_case: true,
                        ignore_crlf: true,
                        ..options
                    }
                )
                .unwrap()
            ),
            (0, 0)
        );
        assert_eq!(
            change_counts(
                &diff_lines_with_options(
                    "a\tb \t\n",
                    "a       b\n",
                    Options {
                        expand_tabs: true,
                        ..options
                    }
                )
                .unwrap()
            ),
            (0, 0)
        );
        assert_eq!(
            change_counts(&diff_lines_with_options(" \t\n", "\n", options).unwrap()),
            (0, 0)
        );
    }

    #[test]
    fn expand_tabs_uses_positional_eight_character_stops_and_keeps_originals() {
        let options = Options {
            expand_tabs: true,
            ..Default::default()
        };
        for prefix in 0..24 {
            let a = format!("{}\tz\n", "x".repeat(prefix));
            let b = format!("{}{}z\n", "x".repeat(prefix), " ".repeat(8 - prefix % 8));
            let rows = diff_lines_with_options(&a, &b, options).unwrap();
            assert_eq!(change_counts(&rows), (0, 0));
            assert_eq!(rows[0].text, a.trim_end_matches('\n'));
            assert_eq!(change_counts(&diff_lines(&a, &b).unwrap()), (1, 1));
        }
        for (a, b) in [
            ("\t\tz\n", "                z\n"),
            ("é\tz\n", "é       z\n"),
            ("界\tz\n", "界       z\n"),
            ("a\tb\tc\n", "a       b       c\n"),
        ] {
            assert_eq!(
                change_counts(&diff_lines_with_options(a, b, options).unwrap()),
                (0, 0)
            );
        }
        for (a, b) in [
            ("a\tb\n", "a b\n"),
            ("a\tb\n", "ab\n"),
            ("a  b\n", "a b\n"),
            ("a\tb\r\n", "a       b\n"),
            ("a\tb", "a       b\n"),
        ] {
            assert_ne!(
                change_counts(&diff_lines_with_options(a, b, options).unwrap()),
                (0, 0)
            );
        }
    }

    #[test]
    fn expand_tabs_precedes_case_mapping_and_composes_with_other_policies() {
        let mut options = Options {
            expand_tabs: true,
            ignore_case: true,
            ..Default::default()
        };
        assert_eq!(
            change_counts(
                &diff_lines_with_options("İ\tX\n", "i\u{307}       x\n", options).unwrap()
            ),
            (0, 0)
        );
        options.ignore_crlf = true;
        assert_eq!(
            change_counts(&diff_lines_with_options("A\tB\r\n", "a       b\n", options).unwrap()),
            (0, 0)
        );
        options.space_changes = true;
        assert_eq!(
            change_counts(&diff_lines_with_options("A\tB\n", "a b\n", options).unwrap()),
            (0, 0)
        );
        options.space_changes = false;
        options.all_spaces = true;
        assert_eq!(
            change_counts(&diff_lines_with_options("A\tB\n", "ab\n", options).unwrap()),
            (0, 0)
        );
        let huge = "\t".repeat(10000);
        assert_eq!(expand_tabs(&huge).len(), 80000);
    }

    #[test]
    fn ignore_crlf_matches_mixed_endings_without_losing_content_or_eof_changes() {
        let options = Options {
            ignore_crlf: true,
            ..Default::default()
        };
        let a = "first\r\nsecond\nthird\r\n";
        let b = "first\nsecond\r\nthird\n";
        assert_eq!(change_counts(&diff_lines(a, b).unwrap()), (3, 3));
        let rows = diff_lines_with_options(a, b, options).unwrap();
        assert_eq!(change_counts(&rows), (0, 0));
        assert_eq!(
            rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
        for (a, b) in [
            ("a\r\n", "a"),
            ("a\rb\n", "ab\n"),
            ("a\r", "a"),
            ("\r\n", ""),
            ("a\r\r\n", "a\n"),
            ("a\r\n", "b\n"),
        ] {
            assert_ne!(
                change_counts(&diff_lines_with_options(a, b, options).unwrap()),
                (0, 0),
                "{a:?}, {b:?}"
            );
        }
        let rows = diff_lines_with_options("old\r\n", "new\n", options).unwrap();
        assert!(rows[0].text.ends_with(CR_MARKER));
        let rows = diff_lines_with_options("a\r\n", "a", options).unwrap();
        assert!(rows.iter().any(|r| r.text.ends_with(NO_EOL_MARKER)));
    }

    #[test]
    fn ignore_crlf_composes_independently_with_space_and_case_policies() {
        for mask in 0..8 {
            let options = Options {
                trailing_spaces: false,
                ignore_crlf: true,
                expand_tabs: false,
                space_changes: mask & 1 != 0,
                all_spaces: mask & 2 != 0,
                ignore_case: mask & 4 != 0,
            };
            assert_eq!(
                change_counts(&diff_lines_with_options("exact\r\n", "exact\n", options).unwrap()),
                (0, 0)
            );
            let rows = diff_lines_with_options("A  B\r\n", "ab\n", options).unwrap();
            assert_eq!(
                change_counts(&rows),
                if options.all_spaces && options.ignore_case {
                    (0, 0)
                } else {
                    (1, 1)
                }
            );
        }
        let options = Options {
            ignore_crlf: true,
            space_changes: true,
            ..Default::default()
        };
        assert_eq!(
            change_counts(&diff_lines_with_options("a  b\r\n", "a b\n", options).unwrap()),
            (0, 0)
        );
        assert_eq!(
            change_counts(&diff_lines_with_options("", "", options).unwrap()),
            (0, 0)
        );
    }

    #[test]
    fn all_spaces_matches_absent_horizontal_spacing_and_preserves_originals() {
        let a = " \ta + b \t\n A\tB \n";
        let b = "a+b\nab\n";
        let mut options = Options {
            all_spaces: true,
            ..Default::default()
        };
        assert_eq!(
            change_counts(&diff_lines_with_options(a, b, options).unwrap()),
            (1, 1)
        );
        options.ignore_case = true;
        let rows = diff_lines_with_options(a, b, options).unwrap();
        assert_eq!(change_counts(&rows), (0, 0));
        assert_eq!(
            rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            vec![" \ta + b \t", " A\tB "]
        );
        options.space_changes = true;
        assert_eq!(diff_lines_with_options(a, b, options).unwrap(), rows);
        options.all_spaces = false;
        assert_eq!(
            change_counts(&diff_lines_with_options(a, b, options).unwrap()),
            (2, 2)
        );
        assert_eq!(change_counts(&diff_lines(a, b).unwrap()), (2, 2));
    }

    #[test]
    fn all_spaces_keeps_line_endings_boundaries_non_ascii_spaces_and_content_changes() {
        let options = Options {
            all_spaces: true,
            ..Default::default()
        };
        for (a, b) in [
            ("a b\r\n", "ab\n"),
            ("a b", "ab\n"),
            ("a\nb\n", "ab\n"),
            ("a\u{a0}b\n", "ab\n"),
            ("a\u{2003}b\n", "ab\n"),
            ("a+b\n", "a-b\n"),
        ] {
            assert_ne!(
                change_counts(&diff_lines_with_options(a, b, options).unwrap()),
                (0, 0),
                "{a:?}, {b:?}"
            );
        }
        assert_eq!(
            change_counts(&diff_lines_with_options(" \t\n", "\n", options).unwrap()),
            (0, 0)
        );
        assert_ne!(
            change_counts(&diff_lines_with_options(" \t\n", "", options).unwrap()),
            (0, 0)
        );
    }

    #[test]
    fn options_compare_keys_but_keep_original_unicode_and_changed_text() {
        let a = "ПрИвЕт\t  мир\nkeep  a\nleft\n";
        let b = "привет мир\nkeep a\nright\n";
        assert_eq!(change_counts(&diff_lines(a, b).unwrap()), (3, 3));
        let options = Options {
            trailing_spaces: false,
            space_changes: true,
            all_spaces: false,
            expand_tabs: false,
            ignore_crlf: false,
            ignore_case: true,
        };
        let lines = diff_lines_with_options(a, b, options).unwrap();
        assert_eq!(change_counts(&lines), (1, 1));
        assert_eq!(lines[0].text, "ПрИвЕт\t  мир");
        assert_eq!(lines[1].text, "keep  a");
        assert_eq!(
            lines[2],
            DiffLine {
                kind: DiffKind::Delete,
                text: "left".into()
            }
        );
        assert_eq!(
            lines[3],
            DiffLine {
                kind: DiffKind::Insert,
                text: "right".into()
            }
        );
        assert_eq!(
            crate::diff_navigation::Navigation::new(&lines).source_numbers,
            [
                (Some(1), Some(1)),
                (Some(2), Some(2)),
                (Some(3), None),
                (None, Some(3))
            ]
        );
        assert_eq!(
            diff_lines_with_options(a, b, Options::default()),
            diff_lines(a, b)
        );
    }

    #[test]
    fn options_keep_missing_blanks_non_ascii_spaces_and_endings_distinct() {
        let options = Options {
            trailing_spaces: false,
            space_changes: true,
            all_spaces: false,
            expand_tabs: false,
            ignore_crlf: false,
            ignore_case: true,
        };
        for (a, b) in [
            ("a b\n", "ab\n"),
            (" a\n", "a\n"),
            ("a\u{a0}b\n", "a b\n"),
            ("A  B\r\n", "a b\n"),
            ("A  B", "a b\n"),
            ("straße\n", "STRASSE\n"),
            ("é\n", "e\u{301}\n"),
        ] {
            assert_ne!(
                change_counts(&diff_lines_with_options(a, b, options).unwrap()),
                (0, 0),
                "{a:?} / {b:?}"
            );
        }
        let lines = diff_lines_with_options("A\r\n", "a\n", options).unwrap();
        assert!(lines[0].text.contains(CR_MARKER));
        let mixed = diff_lines_with_options("A\r\nX\n", "a\nx\r\n", options).unwrap();
        assert!(
            mixed
                .iter()
                .any(|line| line.kind == DiffKind::Delete && line.text == format!("A{CR_MARKER}"))
        );
        assert!(
            mixed
                .iter()
                .any(|line| line.kind == DiffKind::Insert && line.text == format!("x{CR_MARKER}"))
        );
        let lines = diff_lines_with_options("A", "a\n", options).unwrap();
        assert!(lines[0].text.contains(NO_EOL_MARKER));
    }

    #[test]
    fn normalized_prefix_suffix_avoid_quadratic_matrix_and_real_middle_stays_visible() {
        let a: String = (0..3000).map(|i| format!("row {i}  A\n")).collect();
        let b: String = (0..3000).map(|i| format!("row {i} a\n")).collect();
        assert!(diff_lines(&a, &b).is_err());
        let options = Options {
            trailing_spaces: false,
            space_changes: true,
            all_spaces: false,
            expand_tabs: false,
            ignore_crlf: false,
            ignore_case: true,
        };
        let equal = diff_lines_with_options(&a, &b, options).unwrap();
        assert_eq!(equal.len(), 3000);
        assert_eq!(change_counts(&equal), (0, 0));
        let changed = b.replace("row 1500 a\n", "actual change\n");
        let lines = diff_lines_with_options(&a, &changed, options).unwrap();
        assert_eq!(change_counts(&lines), (1, 1));
        let nav = crate::diff_navigation::Navigation::new(&lines);
        assert_eq!(nav.hunks.len(), 1);
        assert_eq!(nav.hunks[0].left_start, 1501);
        assert_eq!(nav.hunks[0].right_start, 1501);
        assert_eq!(lines[0].text, "row 0  A");
        assert_eq!(lines.last().unwrap().text, "row 2999  A");
    }

    #[test]
    fn identical_inputs_are_all_equal() {
        let d = diff_lines("a\nb\nc", "a\nb\nc").unwrap();
        assert_eq!(
            kinds(&d),
            vec![DiffKind::Equal, DiffKind::Equal, DiffKind::Equal]
        );
        assert_eq!(change_counts(&d), (0, 0));
    }

    #[test]
    fn pure_insertion() {
        // "b" inserted between a and c.
        let d = diff_lines("a\nc", "a\nb\nc").unwrap();
        assert_eq!(
            kinds(&d),
            vec![DiffKind::Equal, DiffKind::Insert, DiffKind::Equal]
        );
        assert_eq!(d[1].text, "b");
        assert_eq!(change_counts(&d), (1, 0));
    }

    #[test]
    fn pure_deletion() {
        let d = diff_lines("a\nb\nc", "a\nc").unwrap();
        assert_eq!(
            kinds(&d),
            vec![DiffKind::Equal, DiffKind::Delete, DiffKind::Equal]
        );
        assert_eq!(d[1].text, "b");
        assert_eq!(change_counts(&d), (0, 1));
    }

    #[test]
    fn replacement_is_delete_then_insert() {
        let d = diff_lines("x", "y").unwrap();
        assert_eq!(kinds(&d), vec![DiffKind::Delete, DiffKind::Insert]);
        assert_eq!(d[0].text, "x");
        assert_eq!(d[1].text, "y");
    }

    #[test]
    fn empty_versus_nonempty() {
        assert_eq!(
            kinds(&diff_lines("", "a\nb").unwrap()),
            vec![DiffKind::Insert; 2]
        );
        assert_eq!(
            kinds(&diff_lines("a\nb", "").unwrap()),
            vec![DiffKind::Delete; 2]
        );
        assert!(diff_lines("", "").unwrap().is_empty());
    }

    #[test]
    fn line_ending_only_differences_are_visible() {
        let d = diff_lines("a\r\nb\r\n", "a\nb\n").unwrap();
        assert_eq!(change_counts(&d), (2, 2));
        assert_eq!(d[0].text, format!("a{CR_MARKER}"));
        assert!(
            d.iter()
                .any(|l| l.kind == DiffKind::Insert && l.text == "a")
        );

        let d = diff_lines("a\nb", "a\nb\n").unwrap();
        assert_eq!(change_counts(&d), (1, 1));
        assert_eq!(d[0].kind, DiffKind::Equal);
        assert_eq!(d[1].text, format!("b{NO_EOL_MARKER}"));
        assert_eq!(d[2].text, "b");

        // Same convention on both sides: no markers on real changes.
        let d = diff_lines("x\r\n", "y\r\n").unwrap();
        assert_eq!(d[0].text, "x");
        assert_eq!(d[1].text, "y");
    }

    #[test]
    fn mixed_endings_on_both_sides_are_marked_per_line() {
        // Both files mix CRLF and LF, so the per-file conventions match.
        let d = diff_lines("x\r\ny\n", "x\ny\r\n").unwrap();
        assert_eq!(change_counts(&d), (2, 2));
        let deleted: Vec<_> = d.iter().filter(|l| l.kind == DiffKind::Delete).collect();
        let inserted: Vec<_> = d.iter().filter(|l| l.kind == DiffKind::Insert).collect();
        assert_eq!(deleted[0].text, format!("x{CR_MARKER}"));
        assert_eq!(deleted[1].text, "y");
        assert_eq!(inserted[0].text, "x");
        assert_eq!(inserted[1].text, format!("y{CR_MARKER}"));

        // A real text change in a mixed file stays unmarked.
        let d = diff_lines("x\r\ny\n", "x\r\nz\n").unwrap();
        assert!(d.iter().all(|l| !l.text.contains(CR_MARKER)));
    }

    #[test]
    fn rejects_quadratic_work_before_allocating_the_matrix() {
        // Identical files now trim to an empty middle, so the cap is exercised
        // with two 3000-line files that share no line at all.
        let xs = std::iter::repeat_n("x", 3_000)
            .collect::<Vec<_>>()
            .join("\n");
        let ys = std::iter::repeat_n("y", 3_000)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(diff_lines(&xs, &ys), Err(DiffTooLarge));
        // Shared context around the differing middle does not rescue it.
        let wrapped = |mid: &str| format!("head\n{mid}\ntail");
        assert_eq!(diff_lines(&wrapped(&xs), &wrapped(&ys)), Err(DiffTooLarge));
    }

    #[test]
    fn long_files_with_a_small_change_are_not_refused() {
        let lines: Vec<String> = (0..3_000).map(|k| format!("line {k}")).collect();
        let a = lines.join("\n");
        let mut changed = lines.clone();
        changed[1_500] = "edited".to_string();
        let b = changed.join("\n");

        let d = diff_lines(&a, &b).unwrap();
        assert_eq!(change_counts(&d), (1, 1));
        assert_eq!(d.len(), 3_001);
        assert_eq!(d[1_499].kind, DiffKind::Equal);
        assert_eq!(d[1_500].kind, DiffKind::Delete);
        assert_eq!(d[1_500].text, "line 1500");
        assert_eq!(d[1_501].kind, DiffKind::Insert);
        assert_eq!(d[1_501].text, "edited");
        assert_eq!(d[3_000].text, "line 2999");
        // Identical long files diff as all-equal instead of too large.
        let same = diff_lines(&a, &a).unwrap();
        assert_eq!(same.len(), 3_000);
        assert_eq!(change_counts(&same), (0, 0));
    }
}
