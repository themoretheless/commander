# Spreadsheet cell policy for TSV file details

Continuation of the Double Commander-inspired file-details export. The existing listing CSV path already apostrophe-prefixes cells beginning with =, +, -, @, tab or CR. TSV now shares that exact policy instead of treating quotation alone as sufficient to distinguish formula-like values. The policy is applied to every cell before TSV quoting, and added prefix bytes count toward the existing 1 MiB bound. Ordinary names, interior markers and existing apostrophes remain unchanged. No new filesystem access or output command is introduced.

The CSV helper is extracted without changing CSV output; sharing it prevents separate CSV/TSV marker rules. Header/row limits and atomic refusal remain. Prefix decisions and encoded-byte size are checked before constructing transformed cells. The palette description identifies spreadsheet TSV. This deliberately changes formula-like cell text by adding one apostrophe; consumers requiring original names/metadata can use the existing listing JSON export.

Tests compare parsed TSV and CSV cells for all six prefix markers, quoted multiline data and unchanged values, plus exact byte-limit acceptance/refusal including the added prefix. Existing control/Unicode/row-limit and workspace command tests remain. This is a defined text transformation, not a qualification of every spreadsheet importer or a universal formula-execution prevention claim. Native formula-specific palette/paste and OS spreadsheet behavior remain open; prior native ordinary/control-character exports are separate evidence.

Inspection confirmed that full metadata JSON (paths, exact file bytes, allocation and high-precision UTC dates) was already implemented in listing_report.rs. It was not duplicated.

Preliminary strict Clippy rejected redundant pub(crate) visibility inside the private module. Helpers now use the module’s existing public-function convention; no lint exception was added. Preliminary logs are retained.

Final validation: all seven gates exited 0; 3343 tests passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. Source hashes match the qualified code; focused/full logs retained.
