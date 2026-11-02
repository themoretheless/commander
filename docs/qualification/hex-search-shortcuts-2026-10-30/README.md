# Hex repeat-search keyboard shortcuts — 2026-10-30

Independent adaptation of [Double Commander viewer Find next / Find previous](https://doublecmd.github.io/doc/en/viewer.html), cohort #3, following Commander's existing preview_find keyboard convention. No claim that the exact upstream keymap is identical.

With the hex search input focused, F3 or Enter requests forward search; Shift+F3 or Shift+Enter requests backward search. Directional shortcut events are consumed before TextEdit handling, with backward combinations checked first. Search retains query focus for repetition. A stable search-field ID replaces positional identity. Shortcuts are scoped to that field, require a loaded page and no current worker, and use the same deferred search action as buttons. Other fields/unfocused views do not launch searches. Busy input does not queue duplicate work. Buttons retain their existing behavior and now expose the shortcuts in hover hints.

All input modes, encoded query validation, overlap, wrap, progress/cancel and file/page identity checks remain with the existing search worker. Enter retains forward behavior and Shift+Enter gains backward behavior. No file/clipboard mutation or new persistent settings are added.

Two headless egui regressions drive real keyboard events through the rendered viewer and deterministic workers: forward/backward stepping via all four keys, focus retention, cyclic wrap, unfocused no-op and exactly one queued task when a shortcut arrives during an existing search. Existing pointer/menu and scanner tests remain included. Headless key coverage does not establish native OS function-key delivery.

Native keyboard/focus/menu layout validation remains pending because desktop CUA is unavailable. Historical HexSearch captures do not qualify these new keys/current binary; earlier native gates remain open. No native capture success is claimed.

Qualification notes:

- Initial test/Clippy compilation rejected a test-only RawInput.modifiers field absent in this checkout's egui API. The field was removed; key events carry their explicit modifiers, following the current event API. Initial outputs remain in `test-initial.log` and `clippy-initial.log`.
- Final keyboard regressions passed through actual egui input handling and deterministic worker execution. Full-suite and final check results follow their command logs.

Validation:

- Locked visual-qa full suite: **2779 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied passed (`clippy.log`).
- Formatting and whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked default check and visual-qa build passed (`check.log`, `build.log`). Source/binary hashes and pending native-validation record accompany this report.
