//! Shared natural ordering for directory and archive display names.
use std::cmp::Ordering;

/// Natural ("human") ordering: runs of digits compare by numeric value, so
/// "file2" sorts before "file10". Names compare as supplied; callers use
/// lowercase keys for case-insensitive ordering or original names otherwise.
///
/// Works on UTF-8 bytes so sorting a listing does not allocate per
/// comparison: digits are ASCII and never occur inside a multi-byte
/// character, and byte order within UTF-8 matches code point order.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    // Leading-zero count only breaks a tie once the rest of the names compare
    // equal, so "a01b" still sorts before "a1c".
    let mut zero_tiebreak = Ordering::Equal;
    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let va = strip_leading_zeros(&a[si..i]);
            let vb = strip_leading_zeros(&b[sj..j]);
            let ordering = va.len().cmp(&vb.len()).then_with(|| va.cmp(vb));
            if ordering != Ordering::Equal {
                return ordering;
            }
            if zero_tiebreak == Ordering::Equal {
                zero_tiebreak = (i - si).cmp(&(j - sj));
            }
        } else {
            match ca.cmp(&cb) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                ordering => return ordering,
            }
        }
    }
    // One side is exhausted: the longer remainder sorts last.
    (a.len() - i).cmp(&(b.len() - j)).then(zero_tiebreak)
}

fn strip_leading_zeros(run: &[u8]) -> &[u8] {
    let mut first = 0;
    while first + 1 < run.len() && run[first] == b'0' {
        first += 1;
    }
    &run[first..]
}
