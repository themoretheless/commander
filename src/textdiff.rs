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
    let a = split_lines(a);
    let b = split_lines(b);
    let (n, m) = (a.len(), b.len());
    let has = |lines: &[(&str, Ending)], ending| lines.iter().any(|l| l.1 == ending);
    let mark_cr = (has(&a, Ending::CrLf), has(&a, Ending::Lf))
        != (has(&b, Ending::CrLf), has(&b, Ending::Lf));
    let mark_eof = !a.is_empty() && !b.is_empty() && has(&a, Ending::Eof) != has(&b, Ending::Eof);

    // Shared leading and trailing lines are always part of the LCS, so only
    // the differing middle needs the quadratic matrix: two long files that
    // differ in one line stay well under the cap instead of being refused.
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
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
            dp[at(i, j)] = if a_mid[i] == b_mid[j] {
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
        if a_mid[i] == b_mid[j] {
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
