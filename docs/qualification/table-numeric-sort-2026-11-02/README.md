# Exact numeric table sorting

Commander extension of the [Strata-inspired delimited preview](../../research/feature-transfers-2026-10-27.md#next-transfer-csv-and-tsv-tables). This is independently implemented exact decimal sorting; no claim that upstream Strata has this precise numeric policy.

**Numeric sort** changes active column sorting from natural text ordering to exact signed decimal/scientific ordering. Supported syntax includes an optional sign, decimal point and signed e/E exponent. Surrounding whitespace is ignored for comparison. There is no binary-float rounding or expansion of exponent powers. Each cell is limited to 4096 input bytes and exponent magnitude 1000000. NaN, infinity, localized/grouped numbers, empty/missing cells and unsupported values stay last in both directions, retaining original relative order. Equal numeric values also retain source order.

Numbers are parsed once per sorted column rebuild under the existing 500-row/64-column/256-KiB preview limits. Heading/filter/copy ownership stays unchanged; all exports preserve original cell spelling (including 2.100). Changing the source allocation resets the mode with the rest of view state. No editing, type inference or locale collation is added.

Three new regressions cover precision above f64 integer range, signed zero, negatives, fractions, extreme exponents, equal-value stability, invalid/empty/ragged cells, bounds and legacy original order. A separate comparison oracle uses small exact rational cross-products across 80656 pairs. A real egui pointer test toggles numeric mode and sort direction, applies a filter, checks exported CSV bytes and source-refresh reset. Native OS-window numeric control capture remains open. Seven gates are recorded in checks.json.

Final result: all seven gates passed; 3329 tests passed, 0 failed, 3 ignored. Strict Clippy passed. No dependency or advisory-policy change.
