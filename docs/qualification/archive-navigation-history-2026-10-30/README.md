# Archive navigation history — 2026-10-30

Implemented: Back / Forward buttons inside the archive pane, independently adapting [Yazi directory history](https://yazi-rs.github.io/docs/configuration/keymap/#back) (cohort #27).

History distinguishes All members from folder root and nested folders. Direct navigation records the previous view; a new branch clears Forward. Revisiting the current location adds no duplicate. Cursor IDs are restored and revealed only when visible under the current filter. Query, sorting and archive-wide selection remain current rather than being rolled back. Navigation uses the existing immutable listing and does not read/extract members or change host filesystem directories.

Past and future together retain at most 64 saved locations and 256 KiB of encoded folder paths, excluding the current view. Oldest past entries are pruned first, then oldest future entries. History resets with the listing and is not persisted. No pixel-scroll restoration, keyboard shortcut parity or full Yazi VFS parity is claimed.

Validation:

- Four new regressions: traversal across modes/folders and cursor restoration; branching/no duplicate entries; current filter/sort/selection retention and hidden-cursor rejection; entry/path bounds and reset.
- `cargo test --locked --features visual-qa`: 2747 passed, 3 ignored, zero failed.
- Locked default check, all-target visual-qa Clippy with warnings denied, visual-qa build, formatting and diff whitespace checks passed; logs accompany this report.
- Native Back/Forward clicks, cursor reveal and layout remain **pending**: desktop CUA is unavailable. No native capture or UI success is claimed. Previous archive export/filter/check native gates also remain open.

`source-sha256.json` records the source/binary snapshot; `native-validation.json` records the pending gate.
