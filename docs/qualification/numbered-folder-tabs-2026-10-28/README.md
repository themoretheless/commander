# Numbered folder-tab navigation — 2026-10-28

Independent adaptation of [Double Commander ActivateTabByIndex](https://doublecmd.github.io/doc/en/cmds.html). No upstream code copied. The palette offers Go to folder tab 1 through 8, using the visible strip order in the focused pane. This implements direct indexed navigation in addition to the earlier first/last commands. Cross-pane parameter syntax is not claimed.

Missing/current destinations are disabled with an explanation. Existing pending-operation capability checks apply; runtime activation separately blocks modal ownership. Invalid indices safely do nothing. Activation restores the suspended runtime panel and retains other-pane state and closed history. These navigation commands preserve the last repeatable action. Existing Cmd+number saved-folder shortcuts remain unchanged.

Validation:

- Full local suite: **2422 passed, 0 failed, 3 ignored**. Includes the preceding first/last navigation regression with its corrected modal expectation. New regression covers every numbered destination in both panes, saved per-tab filters, repeated-arrival no-op, unavailable/missing and extreme invalid indices, untouched other pane, closed-history and repeat-action retention, and modal execution denial.
- Final all-target Clippy with visual-qa and warnings denied passed. Initial Clippy found an unnecessary borrow in the test's format! call; corrected, rejected log retained. The full suite predates only that test-style correction.
- Formatting and scoped whitespace checks passed.

 No native palette interaction or real process restart was run for these entries. No commit or push.
