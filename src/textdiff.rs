//! Pure line-level text diff via a longest-common-subsequence backtrack.
//! Deterministic and free of I/O, so the edit script is unit-tested directly.

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

/// Diff `a` against `b` line by line. Equal lines are shared context; lines
/// only in `a` are `Delete`, lines only in `b` are `Insert`. A changed line
/// surfaces as a `Delete` of the old text followed by an `Insert` of the new.
pub fn diff_lines(a: &str, b: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let (n, m) = (a.len(), b.len());

    // dp[i][j] = LCS length of a[i..] and b[j..].
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine {
                kind: DiffKind::Equal,
                text: a[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(DiffLine {
                kind: DiffKind::Delete,
                text: a[i].to_string(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                kind: DiffKind::Insert,
                text: b[j].to_string(),
            });
            j += 1;
        }
    }
    while i < n {
        out.push(DiffLine {
            kind: DiffKind::Delete,
            text: a[i].to_string(),
        });
        i += 1;
    }
    while j < m {
        out.push(DiffLine {
            kind: DiffKind::Insert,
            text: b[j].to_string(),
        });
        j += 1;
    }
    out
}

/// Count of inserted and deleted lines, for a header summary.
pub fn change_counts(lines: &[DiffLine]) -> (usize, usize) {
    let ins = lines.iter().filter(|l| l.kind == DiffKind::Insert).count();
    let del = lines.iter().filter(|l| l.kind == DiffKind::Delete).count();
    (ins, del)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lines: &[DiffLine]) -> Vec<DiffKind> {
        lines.iter().map(|l| l.kind).collect()
    }

    #[test]
    fn identical_inputs_are_all_equal() {
        let d = diff_lines("a\nb\nc", "a\nb\nc");
        assert_eq!(
            kinds(&d),
            vec![DiffKind::Equal, DiffKind::Equal, DiffKind::Equal]
        );
        assert_eq!(change_counts(&d), (0, 0));
    }

    #[test]
    fn pure_insertion() {
        // "b" inserted between a and c.
        let d = diff_lines("a\nc", "a\nb\nc");
        assert_eq!(
            kinds(&d),
            vec![DiffKind::Equal, DiffKind::Insert, DiffKind::Equal]
        );
        assert_eq!(d[1].text, "b");
        assert_eq!(change_counts(&d), (1, 0));
    }

    #[test]
    fn pure_deletion() {
        let d = diff_lines("a\nb\nc", "a\nc");
        assert_eq!(
            kinds(&d),
            vec![DiffKind::Equal, DiffKind::Delete, DiffKind::Equal]
        );
        assert_eq!(d[1].text, "b");
        assert_eq!(change_counts(&d), (0, 1));
    }

    #[test]
    fn replacement_is_delete_then_insert() {
        let d = diff_lines("x", "y");
        assert_eq!(kinds(&d), vec![DiffKind::Delete, DiffKind::Insert]);
        assert_eq!(d[0].text, "x");
        assert_eq!(d[1].text, "y");
    }

    #[test]
    fn empty_versus_nonempty() {
        assert_eq!(kinds(&diff_lines("", "a\nb")), vec![DiffKind::Insert; 2]);
        assert_eq!(kinds(&diff_lines("a\nb", "")), vec![DiffKind::Delete; 2]);
        assert!(diff_lines("", "").is_empty());
    }
}
