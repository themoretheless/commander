# Copy batch rename plans — 2026-10-30

Independent clipboard adaptation of [Double Commander old/new-name lists](https://doublecmd.github.io/doc/en/multirename.html), cohort repository #3. No upstream code copied. This implements export, not editor roundtrip or list import.

Three preview actions copy original names, proposed names, or a CSV mapping with per-row status and rule error. Lists preserve current target order and duplicate text, escaping newline, carriage return and tab for display. CSV preserves raw fields using RFC4180 quoting. Proposed names require every row to be valid or unchanged and no rule error; unchanged-only plans are permitted. Original names and diagnostic CSV remain available for invalid plans. Empty plans cannot be copied.

Exports consume the cached current preview after target reorder/exclusion. Pending context mutations suppress export and submission; copying suppresses same-frame submit. The existing trusted clipboard port supplies feedback. No filesystem operation is performed.

Three regressions cover Unicode, duplicate text, order, controls, empty lists, invalid/collision/expansion-limit gating, unchanged-only plans, raw CSV quoting and invalid-rule diagnostics. Native RenameOptions clicks all three buttons after excluding a target and renumbering: recording clipboard receives notes.md, then 01.md, then the exact mapping CSV. Readiness requires exactly three clipboard writes and untouched fixture paths; the user's real clipboard is isolated.

Native capture passed **16/16** after adding three settling frames between exclusion and copying, reaching phase 97. The first attempt timed out during the layout transition and is retained as native-first-attempt-failed.json. Fresh manifest postdates native-launch.json; exactly three recording clipboard calls, all other native effects zero. Frame inspected: all three buttons fit, notes.md→01.md is the sole row, and copy feedback is visible. Locked build, format and diff checks passed. No commit or push; broader transfers remain open.

Final locked visual-qa suite: **2622 passed, 0 failed, 3 ignored**. All-target Clippy with warnings denied passed after the QA settling change.
