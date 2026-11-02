# Explicit left/right pane focus — 2026-10-28

Independent adaptation of [Double Commander FocusSwap side=left/right](https://doublecmd.github.io/doc/en/cmds.html). No upstream code copied. Swap panels and toggle-focus navigation already existed; the palette now also provides Focus left pane and Focus right pane for deterministic navigation.

The already-focused destination is unavailable. Dispatch changes only the focused side, retaining tabs, filters and selection, and does not replace the last repeatable command. Pending operations and workspace modal ownership block execution, preventing a queued focus command from retargeting a modal's working side. Existing toggle-focus behavior is unchanged.

Validation:

- Final full local suite: **2441 passed, 0 failed, 3 ignored**. The new command regression exercises both explicit destinations, selected-destination capability/idempotence, unchanged tab snapshots/revision, selection/filter retention, last-repeatable preservation and modal denial from either starting side.
- Initial full suite had **2440 passed, 1 failed, 3 ignored** because the early regression itself changed the sort after taking its snapshot. That test fixture was corrected to seed repeat-history directly; the compiler had already read the early version, so a final full rerun was required. Rejected evidence retained. No production fix was needed for that failure.
- Final all-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed.

 No native palette interaction/capture was run for these entries. No commit or push.
