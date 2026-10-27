# Semicolon-separated selection masks — 2026-10-27

Independent adaptation of [Double Commander’s semicolon-separated group masks](https://doublecmd.github.io/doc/en/help.html). No upstream code/dependency copied.

Select by mask now accepts both comma and semicolon delimiters, including mixed input such as *.jpg;*.png,!*raw*. Trimming, case-insensitive matching, extension shorthand, empty-term rejection and subtraction priority remain. The same parser drives the change-count preview and application. Files/folders targeting remains intact. The dialog’s label/hint document the new syntax. Names containing literal semicolons or commas should use Exact names, whose parser remains unchanged.

Verification:

- Full local suite: **2403 passed, 0 failed, 3 ignored**. New coverage compares mixed delimiters against legacy comma parsing; checks empty/subtraction terms; verifies preview-versus-applied selection changes, subtraction precedence and idempotence across Both/Files/Folders targets; verifies Exact names selects literal semicolon/comma filenames.
- After a test-only Clippy style repair, the affected semicolon regression passed again. Final all-target Clippy with visual-qa and warnings denied, formatting and scoped whitespace checks passed. No full rerun after that iterator-only test change.
- Initial test compilation rejected retained borrowed paths across selection mutation. The test now owns its before/after snapshots. Clippy then requested iter() instead of into_iter() on the selected-path slice. Rejected logs retained; neither repair changed production behavior.

No native UI capture, real file operation, remote CI, commit or push performed for this localized grammar change.
