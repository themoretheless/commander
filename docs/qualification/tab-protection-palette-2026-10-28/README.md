# Folder-tab protection commands — 2026-10-28

[Double Commander tab-option commands](https://doublecmd.github.io/doc/en/cmds.html) inspired keyboard-accessible protection management through the command palette. Independent adaptation of command-based tab-state control: Commander protection prevents closing while allowing navigation; it does not lock a directory. No upstream code copied.

The palette now offers Protect folder tab from closing and Unprotect folder tab. They target the active tab in the focused pane. Availability reflects current protection and pending operations; workspace tab guards also reject modal-time changes. Both operations use existing persisted metadata and presentation invalidation. Existing context-menu protection is unchanged.

Validation:

- Full local suite: **2420 passed, 0 failed, 3 ignored**. Both-pane regression verifies capability changes, close-command denial, preservation of active tab/filter and untouched other pane. Existing modal regression now dispatches ProtectTab and verifies no mutation.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed.
- Initial test build used a nonexistent helper; corrected to the owning availability API. Rejected log retained.

 No native framebuffer scenario or real desktop restart is claimed for these palette entries; existing runtime protection UI was qualified separately. No commit or push.
