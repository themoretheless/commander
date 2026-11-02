# Exact numeric bounds for size queries — 2026-10-28

Qualification follow-up to the fd-inspired [file-size filtering](https://github.com/sharkdp/fd/blob/master/src/cli.rs) already transferred to Commander. This fixes Commander's parser rather than adding another spelling. No upstream code copied.

Two pre-fix runs are retained. The first proved `size:>18446744073709551615B` was accepted because the strict-to-inclusive conversion used saturating_add. The second also proved a decimal byte value of 9007199254740993.0 became 9007199254740992 through f64. Existing ordinary strict-bound tests still passed, so they did not cover these limits.

Strict > now checks the one-byte increment and rejects an impossible upper-bound query, symmetric with the existing size:<0 rejection. >=MAX, >MAX-1 and <MAX keep their valid inclusive/exclusive boundaries. Existing integer JSON bounds remain unchanged.

Size parsing now uses integer unit factors and decimal digit arithmetic throughout. Whole-byte multiplication/addition/rounding are checked. Fractional multiplication runs right-to-left with carry bounded by the unit factor, so even very long fractions do not need an overflowing denominator or floating-point approximation. The existing policy remains: round to nearest byte, ties upward, then apply the size operator. This does not redefine comparison against unquantized rational thresholds. Binary units and aliases remain unchanged; .5KB and 1.B retain their accepted syntax.

Regressions cover all text modes, expression/JSON round trips, max-u64 edges, values beyond 2^53, upper-limit rounding, just-below-half fractions at byte/KB scale, tiny long TB fractions and overflow. Maximum-size tests use synthetic metadata, not enormous filesystem files. A real native/index/ZIP matrix uses small files at long-decimal thresholds and confirms identical result sets without content reads. No UI widget changed or new native capture performed. No commit or push.

Validation: **2542 passed, 0 failed, 3 ignored** in the full locked visual-qa suite; all four focused size regressions passed. All-target visual-qa Clippy with warnings denied, formatting and scoped whitespace checks passed. Both pre-fix failures are retained as reproduced.log and decimal-reproduced.log.
