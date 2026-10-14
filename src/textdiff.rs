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
    let text = |(content, ending): (&str, Ending)| match ending {
        Ending::CrLf if mark_cr => format!("{content}{CR_MARKER}"),
        Ending::Eof if mark_eof => format!("{content}{NO_EOL_MARKER}"),
        _ => content.to_string(),
    };

    let rows = n.checked_add(1).ok_or(DiffTooLarge)?;
    let cols = m.checked_add(1).ok_or(DiffTooLarge)?;
    let cells = rows.checked_mul(cols).ok_or(DiffTooLarge)?;
    if cells > MAX_DIFF_CELLS {
        return Err(DiffTooLarge);
    }

    // dp[i][j] = LCS length of a[i..] and b[j..].
    let mut dp = vec![0u32; cells];
    let at = |i: usize, j: usize| i * cols + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if a[i] == b[j] {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine {
                kind: DiffKind::Equal,
                text: a[i].0.to_string(),
            });
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            out.push(DiffLine {
                kind: DiffKind::Delete,
                text: text(a[i]),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                kind: DiffKind::Insert,
                text: text(b[j]),
            });
            j += 1;
        }
    }
    while i < n {
        out.push(DiffLine {
            kind: DiffKind::Delete,
            text: text(a[i]),
        });
        i += 1;
    }
    while j < m {
        out.push(DiffLine {
            kind: DiffKind::Insert,
            text: text(b[j]),
        });
        j += 1;
    }
    Ok(out)
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
    fn rejects_quadratic_work_before_allocating_the_matrix() {
        let many_lines = std::iter::repeat_n("x", 3_000)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(diff_lines(&many_lines, &many_lines), Err(DiffTooLarge));
    }
}
