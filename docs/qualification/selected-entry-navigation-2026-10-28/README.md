# Navigate among selected visible entries — 2026-10-28

Independent adaptation of [fzf down-selected/up-selected actions](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). No upstream code copied. The palette adds Go to next selected entry and Go to previous selected entry for the focused pane.

Commands move strictly below/above the current cursor among visible selected files or folders, respecting display order and filtering. They do not wrap, change picks, or navigate into a folder. No selected entry in a direction disables that action with a reason. Moving reveals the cursor through the existing scroll request; no destination leaves position/scroll unchanged. Selection iteration borrows the current filtered view, without filesystem access or allocating a separate selected-row list.

Directional capability flags are derived in the existing full command-context scan using visible row numbers rather than underlying entry indices. The commands preserve the last repeatable action and other-pane state. Workspace dispatch rejects pending operations and modal ownership.

Validation:

- Full local suite: **2445 passed, 0 failed, 3 ignored**. Panel regression exercises both directions from every row including the parent, explicit boundary expectations, mixed file/folder picks, filtered-out picks, no-wrap behavior, scroll request, descending order and an empty filtered view.
- Both-pane command regression checks per-direction capabilities before/after movement, selection and repeat-history retention, other-pane cursor/tab stability, focused-side retention, modal denial and unavailable directions when the only visible row is unselected.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native palette interaction/capture was run for these new commands. No commit or push.
