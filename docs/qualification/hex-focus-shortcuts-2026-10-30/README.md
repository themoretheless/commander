# Hex Go/Search focus commands — 2026-10-30

Independent adaptation of [Midnight Commander viewer F5 Goto / F7 Search](https://github.com/MidnightCommander/mc/blob/master/doc/man/mc.1.in), cohort #40. Upstream opens dialogs; Commander focuses existing inline fields rather than claiming identical dialog or keymap parity.

While the hex viewer is shown and no read/search is active, unmodified F5 focuses Go to offset and F7 focuses Search. The target response requests scroll reveal within the outer sheet. Both actions preserve typed values, current page, bookmarks and history and do not queue read/search work. A stable offset-field ID complements the existing stable query ID. The read-only header now advertises both keys and Find next's hint explains F7 plus F3/Enter repetition.

A modifier-state guard leaves Shift/Control/Alt/Command function-key combinations untouched. During active work no focus command is consumed, queued or replayed. Existing scoped F3/Shift+F3 and Enter search behavior remains. These keys are handled only by the active HexViewer show path; they do not install a global binding or change normal file-panel F5/F7 actions. OS/media interception still requires native validation.

Two headless egui regressions send real modifier/key events, verify field focus and retained query/page with no new work, and exercise Shift suppression plus exactly one existing queued read while busy. Scroll reveal wiring is inspected, not native visually qualified. Existing keyboard, pointer, page and search regressions remain in the full suite.

Native F5/F7 delivery, focus/scroll reveal and layout validation remain pending because desktop CUA is unavailable. Historical hex captures do not qualify these keys/current binary; earlier native gates remain open. No native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2790 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied passed (`clippy.log`).
- Formatting and whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked default check and visual-qa build passed (`check.log`, `build.log`). Source/binary hashes and pending native-validation record accompany this report.
