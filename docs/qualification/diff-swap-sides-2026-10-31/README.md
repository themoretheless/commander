# Swap retained comparison operands — 2026-10-31

Independent adaptation of [Midnight Commander Ctrl+U in its internal diff viewer](https://github.com/MidnightCommander/mc/blob/master/doc/man/mc.1.in), cohort #40. No upstream implementation was copied.

**Swap compared sides** reverses the captured minus/plus paths, labels and loaded source strings, then rebuilds the edit script. It retains case/space options, intraline highlighting, source-number visibility, compact-context settings and Find query/options. It resets stale selected rows, Go-to coordinates, expansions and scrolling, and activates comparison keyboard navigation. It never swaps or navigates file-manager panels. Reload rereads the reversed captured paths; failed-read diagnostics continue to name the unreadable original path.

**Ctrl+U** invokes the same operation only while comparison keyboard navigation owns input. Find/Go-to fields retain text editing; other inputs, modals and popups retain their existing priority. Exact Ctrl without Shift/Alt/Command is required (the platform's Ctrl command bit is accepted); held-key repeats do not reverse repeatedly. No new global shortcut or file editing is introduced.

## Evidence

- Six final checks passed: locked QA build, full QA tests, all-target QA Clippy with denied warnings, default check, formatting, whitespace. **3072 passed, 3 ignored**. Default check retains the existing QA-only `diff_find.rs` response warning.
- Three new regressions: swap cached sources after the files disappear, preserve policies/search and rebuild asymmetric changes/line numbers/CRLF/EOF markers, exact round trip and reversed Reload; failed-read swap/recovery and identical files; modifier/repeat admission using real held-key frames.
- Native `diff_swap`: **14/14**, 29 injected egui input phases. Actual controls enable comparison options, intraline highlighting and line numbers. Swap button, Ctrl+U reversal, focused Find Ctrl+U editing, another swap and Reload are exercised.
- Fixture files are deliberately changed before the first swap. Snapshots retain the old exact source strings until Reload and then contain the new strings in reversed order. Independent assertions check paths, labels, source numbering, asymmetric insertion/deletion counts, query ownership and unchanged panel paths. Find text `left` becomes empty under field-owned Ctrl+U without swapping sides.
- Existing native `diff_options`: **14/14**, original 18-phase option workflow and retained-source behavior. Both captures have zero recording native effects.
- Final screenshot inspected for the new button, reversed header, retained checked controls, source-number columns and ending markers. Native launch uses CUA; events inside the app are QA injection, not physical keyboard qualification. No file modifications are performed by the production swap action; only test fixtures change for the Reload check.

`checks.json`, `source-sha256.json`, `native-assertions.json`, `native/`, `native-options/`, launch records and logs retain the evidence. `verify.py` records the independent assertions. `before-held-key-fixture-fix/` preserves a failed first test attempt (egui derives repeat from prior held state); `before-qa-range-lint-fix/` preserves the successful native attempts before the QA-only range spelling correction. Final evidence is at the top level.

This completes operand reversal in the existing read-only comparison, not editable merge, patch generation/application, binary comparison or full cohort parity.
