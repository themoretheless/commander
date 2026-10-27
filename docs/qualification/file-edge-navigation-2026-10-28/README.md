# Jump to first/last visible file — 2026-10-28

Independent adaptation of [Double Commander cm_GoToFirstFile / cm_GoToLastFile](https://doublecmd.github.io/doc/en/cmds.html). No upstream code/dependency copied.

Go to first visible file and Go to last visible file are available in the command palette when the current filtered view contains files. They move the cursor to the first/last non-directory in current display order and request cursor reveal. They honor filters, reverse sorting and folders-first/inline grouping; selections, folder paths and the other pane remain unchanged. Like ordinary cursor navigation, these moves do not replace the Repeat last command target. No new hotkey introduced. If no visible files remain, direct execution is a no-op and palette availability is disabled.

Verification:

- Full local suite: **2411 passed, 0 failed, 3 ignored**. New panel regression covers first/last targets across reverse sorting and folders-first/inline display, filtered single-file views, folder-only no-ops, cursor reveal and retained selection. Workspace regression executes both commands, verifies availability/no-file gating, selection/opposite-pane/focus retention and Repeat last target preservation. This run also includes the preceding same-stem workspace regression.
- All-target Clippy with visual-qa and warnings denied, formatting and scoped whitespace checks passed.
- Initial compilation rejected an iterator borrowing the panel across cursor mutation. Its lifetime is now confined to the row lookup block. Rejected compilation logs retained.

No native palette interaction, real file mutation, remote CI, commit or push performed.
