# Archive extension sorting — 2026-10-30

Independent adaptation of [Double Commander SortByExt](https://doublecmd.github.io/doc/en/cmds.html), cohort #3.

The archive Sort menu now offers Extension for ZIP and all supported TAR codecs. The key is the final filename suffix from cached Unicode-lowercased internal paths, not the parent directory's suffix. `.profile`, extensionless names, trailing dots and directories share an empty extension; `.config.json` uses json, and archive.tar.gz uses gz. Comparison uses ordinary string order. Natural names still control the ascending full-path tie breaker; Reverse reverses the extension key, retaining existing ascending tie semantics. Folders first retains priority; synthetic navigation folders retain existing name ordering. This does not claim Double Commander's full sort configuration parity.

Changing to Extension starts ascending in both the menu and shared column-choice logic. No member array or original index changes: selection, extraction identity and duplicate paths retain the established row-ID contract; visible and selected exports reuse the same Order implementation. Keys borrow cached strings and introduce no per-comparison allocation.

Regression checks case-insensitive suffix ordering, reverse, folders-first, duplicate IDs, dotfiles, compound suffixes, directory dots, parent-only suffixes, Unicode suffixes, initial/toggled direction and codec availability.

Native menu selection/order/layout verification remains pending because desktop CUA is unavailable. Earlier archive native gates remain open. No native capture is claimed.

Validation:

- Full locked visual-qa suite: **2754 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied: passed (`clippy.log`).
- Default locked check, formatting and diff whitespace checks: passed (`check.log`, `fmt.log`, `diff-check.log`).
- Locked visual-qa build: passed (`build.log`). Source and binary hashes accompany this report.
