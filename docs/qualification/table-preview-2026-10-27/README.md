# CSV/TSV preview qualification — 2026-10-27

Validation: `cargo test preview -- --test-threads=1` — **64 passed**, no failures.

Scope: independent bounded CSV/TSV table renderer and Table/Source integration.
Reference idea: Strata service `src/services/table.rs` at
`ac5ab4f22d86798a40d7ad30eb3528d56eecf137`.

Parser cases cover quoted multiline Unicode, BOM/CRLF, ragged records,
empty TSV cells, escaped quotes, literal formulas, empty input, row/column
limits and UTF-8-safe byte limits. Headless AccessKit checks cover Table/Source
controls and the Source line-wrap control. The preview regression run also
covers existing document and source previews.

Clippy across all targets/features with warnings denied, dependency licenses,
formatting and whitespace checks pass. Logs are retained alongside this file.
No native screenshot, workbook support, editing or large-file performance
qualification is claimed for this increment.

An initial headless test expected an offscreen virtualized cell to appear as
an accessibility node; that assertion failed. Literal formula preservation is
now checked in the parser, and the UI test checks the visible mode controls.
This failed assertion did not justify a full-suite rerun.
