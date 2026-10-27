# First/last folder-tab navigation — 2026-10-28

Independent adaptation of [Double Commander ActivateTabByIndex first/last destinations](https://doublecmd.github.io/doc/en/cmds.html). The command palette offers First folder tab and Last folder tab for the focused pane. This transfers the edge destinations; arbitrary indexed activation and cross-pane parameters are not claimed. No upstream code copied.

Commands reuse existing tab activation, including suspended runtime-state restoration and presentation invalidation. Availability disables the already-active destination and pending operations. Workspace tab guards block pending modals too. Protection does not prevent switching tabs. Navigation preserves the last repeatable command, the other pane and closed history; repeated arrival is a no-op.

Validation:

- Initial full suite: **2421 passed, 0 failed, 3 ignored**, before the final repeat-history exclusion and strengthened modal regression.
- Full suite with the final production changes: **2420 passed, 1 failed, 3 ignored**. The new regression incorrectly expected modal ownership to disable palette capabilities; the capability context tracks pending operations, while tab execution separately guards modal ownership. Removed that incorrect expectation, retaining both-direction modal no-movement/revision/filter checks.
- Final targeted regression: **1 passed** across both panes, three tabs, saved filters/protection, destination availability, repeated-arrival no-op, other-pane and history preservation, repeat-command retention, and modal execution denial. The full suite was not rerun after that test-only correction; no final all-suite-green claim.
- Final all-target Clippy with visual-qa and warnings denied passed; formatting and scoped whitespace checks passed.

 No native palette interaction or real process restart was run for these new actions. No commit or push.
