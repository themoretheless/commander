# Explicit table delimiter choice

Independent adaptation of [Strata's delimiter-parameterized table parser](https://github.com/lgse/strata/blob/ac5ab4f22d86798a40d7ad30eb3528d56eecf137/src/services/table.rs), inspected through GitHub. Commander exposes File default, Comma, Tab, Semicolon and Pipe in the preview. This is a manual UI extension; no automatic detection or upstream UI parity is claimed. Existing .csv/.tsv eligibility and comma/tab filename defaults remain. No source code was copied.

Choice is held per pane and source allocation, alongside the bounded loaded text. Changing the effective delimiter reparses the cached text in the same frame, resets heading/filter/sort/numeric mode and vertical row scroll, and updates the copy format. File/source refresh or a different filename default resets the choice to File default. Switching labels with the same effective byte does not needlessly invalidate the view. There is no filesystem rewrite or extra read. Existing 500-row/64-column/256-KiB parse and 1-MiB export bounds remain.

Visible export uses the active delimiter and the csv crate's quoting for cells/line breaks; labels distinguish semicolon/pipe from comma CSV and TSV. Source text/cells are preserved and no locale decimal inference is added.

A new actual egui popup/pointer test selects Semicolon, validates quoted separators and embedded newline, composes numeric sorting/filtering, checks the actual returned copy bytes, switches to Pipe and checks reset, refreshes source and checks defaults and pane isolation. The existing roundtrip export regression now covers all four delimiters, retaining headings/ragged rows/formula literals. Unsupported export-byte expectation changed from pipe to colon. Native OS-window delimiter popup/copy remains open. Seven gates are recorded in checks.json.

Final result: all seven gates passed; 3330 tests passed, 0 failed, 3 ignored. Strict Clippy passed. No dependency, format autodetection or persistence schema change.
