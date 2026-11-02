# Bounded delimiter detection

Commander extension of the Strata-inspired delimited preview and explicit delimiter controls. **Detect delimiter** samples at most the first 32 logical CSV records in the bounded loaded text, using comma/tab/semicolon/pipe readers. It requires at least two records with the same 2–64 columns and exactly one valid candidate. Quoting/BOM/multiline handling comes from the same csv parser, rather than raw separator counts. Input scanning remains bounded to 256 KiB at UTF-8 boundaries; there is no additional file read.

Success selects the detected manual dialect and feeds the normal same-frame parse/export path. Only an effective separator change resets heading/filter/sort/numeric state. Ambiguous, one-record, one-column, inconsistent or unsupported samples show a persistent message without changing the selected delimiter, actual table, cached row order or copied data. Manual selection clears the message even if selecting the same byte; source replacement clears the detection state.

Detection is explicit and sample-based, not a guarantee about records after 32 or a replacement for manual review. More than one consistent dialect is refused without prioritizing the filename default or guessing. This is independently specified Commander behavior, not a claim that Strata implements this detector.

Two new regressions cover all four dialects with quoted separators/multiline cells/BOM, ambiguity, row/column/byte boundaries and later records beyond the sample. A real egui popup/pointer failure test proves retained parsed-table and ordered-row allocations, filters/sort/numeric policy and actual copy bytes; selecting the same manual choice clears the error and source refresh resets state. The previous positive popup test now invokes Detect on a semicolon file and checks parse/export plus manual Pipe override and pane isolation. Native OS-window Detect/ambiguity paths remain open. Seven gates are recorded in checks.json.

Final result: all seven gates passed; 3332 tests passed, 0 failed, 3 ignored. Strict Clippy passed. No dependency or session schema change.
