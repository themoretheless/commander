# Theme JSON exchange UI

Continuation of the [Kiorg-inspired custom-theme milestone](../custom-theme-2026-11-02/README.md). This qualifies Commander's own JSON workflow, not Kiorg/Strata file compatibility.

The theme_exchange native scenario starts with an empty unapplied editor. It focuses the actual JSON TextEdit, sends Cmd+A and an egui Paste event containing a valid theme, clicks Apply, then replaces the draft with invalid text and clicks Apply again. The applied snapshot and live panel color must stay unchanged on rejection. Copy applied JSON must record the valid applied document despite the invalid draft. Reset must clear applied data, restore Commander dark colors, retain the invalid draft and leave the editor open. Checkpoints record saved data, draft, error and actual panel RGB after each action. No physical OS pasteboard or keyboard claim: the clipboard is the existing recording port.

Validation: all six gates passed; 3,261 tests passed / 3 ignored, strict Clippy and both builds green. Theme exchange and the prior custom_theme Apply control each passed native 16/16. Both final screenshots were visually inspected. The recording clipboard contains exactly the valid imported document; saved state, error, draft and RGB checkpoints independently agree through rejection, copy and reset. Theme exchange has one recorded clipboard call and all other native effects zero; the control has no native effects.

The prior custom_theme native Apply workflow is the control. Both scenes are included in CI. Source, binary and validator hashes are retained with the final run records. Earlier feature tests cover strict validation and actual atomic session reopen.

Dedicated native color-picker, settings-menu entry, OS Increase Contrast transitions and process-level restart coverage remain open. The dependency graph is unchanged; previous audit failures and the overall 99-repository transfer goal remain open.
