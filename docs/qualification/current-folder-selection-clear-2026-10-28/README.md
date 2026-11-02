# Clear only the current folder selection — 2026-10-28

Completes the clearing scopes around the [fzf-inspired selection actions](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). Commander has selection memory across folders, so Clear selection in this folder is a separate middle scope between Clear visible selection and Clear selection in all folders. No upstream code copied.

The palette command clears current-folder picks, including filtered-out entries, by reusing the existing folder-local selection operation. Picks remembered in other folders and the separate save/restore buffer remain intact. An O(1) current-folder selection count supplies capability evaluation; the action is unavailable if only other folders contain picks. No listing scan or filesystem operation is added by this count. Cursor, filter and focused pane remain unchanged.

Validation:

- Full local suite: **2472 passed, 0 failed, 3 ignored**. New both-pane regression covers filtered-out local picks, unavailable visible-only clearing, available folder-local clearing, preservation of remembered other-folder picks, disabled state after clearing, unchanged cursor/filter/focus and other-pane selection, and restoration from the untouched saved buffer.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native palette interaction/capture was run. No commit or push.
