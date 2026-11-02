# Filtered deselection and stationary selection toggle — 2026-10-28

Independent adaptation of [fzf deselect-all versus clear-multi, and toggle actions](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). No upstream code copied.

Clear visible selection removes only selected rows of the current filtered view, retaining excluded picks and selections remembered elsewhere. It does not clear the separately saved selection-memory buffer. It is unavailable when the visible view has no selected rows. Existing Clear selection in all folders retains its wider scope. The new operation shares filtered-removal handling with the earlier per-kind deselection actions.

Toggle current selection in place toggles only the cursor entry, without advancing or replacing the last repeatable command. The parent row is unavailable/no-op; filtered-out picks and other-pane state remain intact. Existing Space/ToggleSelect still advances down after toggling. Neither action adds a default shortcut or filesystem operations.

Validation:

- Full local suite: **2447 passed, 0 failed, 3 ignored**. Panel regression covers mixed visible file/folder picks, excluded picks, remembered picks elsewhere, exact removed counts, repeated application, empty filtered views, unchanged cursor/query and restoration from an unaffected saved buffer.
- Both-pane command regression checks selected-visible availability, stationary toggle on/off without cursor movement or repeat-history replacement, filtered clearing that retains hidden picks, parent-row denial/no-op and other-pane/focus retention. Existing kind-deselection regressions also passed after sharing the filtered-removal helper.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native palette interaction/capture was run for these entries. No commit or push.
