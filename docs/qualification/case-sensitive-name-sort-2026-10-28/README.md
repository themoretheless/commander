# Case-sensitive name sorting — 2026-10-28

Independent adaptation of [Double Commander configurable name-sort case sensitivity](https://doublecmd.github.io/doc/en/configuration.html). No upstream code copied. Commander adds Sort: case-sensitive names (toggle) to the palette, default off. This compares original Unicode names in code-point order (ASCII uppercase before lowercase), retaining accents. It is not locale collation or full parity with Double Commander's locale/uppercase-first policies.

The option applies to full-name and stem primary sorting and ascending name ties in other sort columns. Natural numeric comparison remains independently configurable; folders-first and sort direction retain their existing policies. Comparisons borrow original names without adding key allocations. Toggling reuses cursor/selection reconciliation and sends the existing notice with on/off state.

ViewConfig carries the option in per-folder memory and tab snapshots/copies. Both persisted pane-view representations serialize it, with missing legacy fields defaulting off. Existing enum/config fields remain compatible.

Validation:

- Full local suite: **2433 passed, 0 failed, 3 ignored**. New sort regressions cover case-sensitive full-name ascending/descending order, non-name descending ties remaining ascending, toggle-off defaults, natural/plain numeric stems and Unicode code-point/accent behavior.
- Both-pane command regression checks actual row order, cursor/selection retention, independent other-pane settings, duplicated-tab view inheritance and subsequent independent toggling, and tab JSON roundtrip.
- Pane persistence regression covers left/right serialization and legacy missing-field defaults; empty legacy ViewConfig defaults off too. This validates serialization, not an actual process restart.
- All-target Clippy with visual-qa and warnings denied passed. Final formatting and scoped whitespace checks passed. Only documentation/comments changed after full suite and Clippy. No rejected run for this feature.

 No native interaction/capture or actual desktop restart was run for this option. No commit or push.
