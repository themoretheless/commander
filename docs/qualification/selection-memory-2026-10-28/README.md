# Save/restore selection memory — 2026-10-28

Independent adaptation of [Double Commander SaveSelection/RestoreSelection](https://doublecmd.github.io/doc/en/cmds.html). No upstream code copied. The palette offers Save selection in memory and Restore saved selection. Each runtime tab has one memory buffer bound to the folder where it was saved. It is not session-persisted or copied into duplicated tabs; owned-panel transfers and closed-tab recovery retain it.

Save records explicit current-folder picks, including picks excluded by the text filter, without cursor fallback. Saving an empty set intentionally overwrites the buffer. Restore replaces this folder's selection with saved paths present in its complete listing; it preserves picks in other folders and does not consume the buffer. Different-folder and incomplete-listing attempts are rejected without changing the selection/buffer. Returning to the saved folder enables restore. No filesystem probes are added to capability evaluation; presence follows the loaded listing, not a new filesystem identity check.

Pending operation checks apply to palette capability; workspace dispatch separately blocks pending modal ownership for both commands. Selection replacement bumps the existing selection revision for dependent views.

Validation:

- Full local suite: **2426 passed, 0 failed, 3 ignored**. Panel regressions cover filtered-out picks, replacement rather than additive restore, deletion followed by refresh, empty-buffer overwrite, folder binding/return, selections remembered elsewhere, rejected partial/denied/gone reads and retained buffer after refresh.
- Workspace regression covers both panes, command availability, independent new-tab memory, switching back, closed-tab recovery, modal-time save/restore denial and later recovery of the original buffer. Final **1 targeted test passed** after strengthening the other-pane/focus assertions; the full suite predates only that test refinement and documentation.
- Final all-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native palette interaction, restart persistence or real filesystem transfer is claimed. No commit or push.
