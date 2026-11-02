# Copy captured file details as TSV

Independent adaptation of Double Commander’s Copy all shown columns / cm_CopyFileDetailsToClip, cohort entry 4: https://doublecmd.github.io/doc/en/help.html, consulted 2026-11-02. No upstream implementation copied.

The command palette adds Copy file details (TSV), exporting a header and the captured Name, Size and Modified display strings of the active pane’s visible selection, or cursor entry when nothing is selected. It uses existing selected-or-cursor semantics and PICKED availability. It does not query filesystem metadata or traverse folders. The data uses existing display strings (including any display placeholders or lossy filename representation); it is not a machine-readable exact-size/timestamp or lossless-path report. Column visibility customization and columns beyond these three are not claimed.

All cells are quoted with doubled quote escaping. Tabs, CR/LF and quotes in filenames round-trip through a TSV parser; physical newlines can occur inside quoted cells. Output is bounded to 10000 entries and 1 MiB of encoded UTF-8, computed before each record is written. Failure returns only a notice and publishes no partial clipboard text. Parent/no visible entry produces no clipboard request. Existing CopyText delivery/feedback owns the native effect. No new shortcut is assigned.

Regression coverage checks quoted controls/Unicode, exact encoded-byte boundary, quote expansion, row cap and empty input; workspace command routing, availability, captured selection after source removal without a refresh, cursor fallback and unchanged selection/cursor. Native command-palette/OS pasteboard qualification remains open. This is a fixed three-column adaptation, not full upstream view-column parity.

The preliminary test compile used a private selection field. It was corrected to use the existing public cursor API; the failed log is retained.

Final validation: all seven gates exited 0; tests 3342 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. Final source hashes and focused/full logs are retained.
