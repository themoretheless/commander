# Native file details TSV command

New native copy_details scenario uses End, Cmd+K, exact command query and Enter to copy the cursor file; then Cmd+A, another palette invocation/query and Enter to copy all six visible entries. Each input phase advances only after inner UI handling. The fixture includes a tab, newline, quotes and Cyrillic in one filename. Selection/navigation/command delivery use keyboard input; no direct export-handler calls or selection-model assignments simulate those actions.

Final receipt 1793649441231298000/copy_details captured 14/14 with exactly two recording clipboard writes and zero other native-port effects. The final selected pane was visually inspected after normal copy toasts expired. The control-character filename uses existing visible fallback glyphs; the export, rather than the screenshot, proves complete literal characters. Intermediate palette layout is not visually qualified by the final pane image.

Expected-one/expected-many JSON observes cached name/size_str/modified_str fields immediately before palette invocations. verify.py independently parses actual copied TSV and regenerates byte-identical quoted output with Python csv. It also checks the fixed fixture name set, cursor README.txt and source/binary hashes. Sizes/dates are captured display strings; measured folder-size overlays and exact numeric/timestamp reporting are not claimed.

The scenario includes the default six-second toast lifetime in addition to the normal ten-second capture budget, as existing checksum/delimiter scenes do. Production toast timing is unchanged. Snapshot export is observational; fixture setup creates only the owned QA data. Native process completion is checked before replacing/staging the wrapper. The initial many-row count was corrected to six from inspecting fixture construction before native launch.

OS pasteboard, filtered/hidden selections, limit-error UI, non-UTF8 filenames and alternate viewport sizes remain separate native scope. Core tests cover several limits/cursor behaviors; no full upstream dynamic-column parity claim.

Final validation: seven gates exited 0; 3342 tests passed, 0 failed, 3 ignored. Independent verify.py passed against matching final source/captured binary hashes and compared raw UTF-8 clipboard bytes to Python csv output. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning.
