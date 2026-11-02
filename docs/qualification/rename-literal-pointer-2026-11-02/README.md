# Regex literal replacement pointer regression

The production checkbox is extracted without changing its behavior. A real egui pointer sequence (move, press, release) enables literal replacement, attempts a click while regex is disabled, then disables literal replacement after regex is restored. On the release frame, the test checks the actual cached preview and submit rule: old.txt → $1.txt → old.txt. The disabled control retains its choice and rejects the attempted click. No direct checkbox-value assignments simulate interaction.

The earlier insertion of a test accidentally moved an existing visual-qa attribute onto that test. The attribute now belongs to the original QA-specific preset-order test again; both literal replacement tests run without visual-qa as well.

This exercises egui input and production control/validation code, not a native window, full dialog layout or the OS pointer. Native checkbox layout qualification remains open.

Default-feature test compilation exposed an existing archive test calling `MemberFilter::row_position`, which was gated only on visual-qa. The helper is now compiled for tests as well, retaining its production feature gate. The original failure and the successful default-feature focused rerun (4/4) are retained. Final full default-feature tests and all seven regular gates follow this change; earlier gate logs are kept under preliminary-checks.

Final results: seven gates exited 0; visual-qa tests 3336 passed / 3 ignored; default-feature full tests 3319 passed / 3 ignored; no failures. Source hashes match the final code. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning.
