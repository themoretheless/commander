# Native archive filter save / Reset / recall round trip — 2026-10-30

Continues qualification of the transferred archive filters and saved presets. ArchiveBrowser retains its dotfile/exclusion steps, then types +1b into size, 1..3 into depth, 1970-01-01 into Modified UTC and rs into Extensions using each actual field. It clicks Save filter, Reset filters, opens Saved filters and recalls the original snapshot at index 0.

Reset must leave all active query/predicate settings clear while retaining exactly one saved entry before the scenario can open its menu. Final phase 35 requires all four exact metadata strings, the original exclusion and dotfile settings, and complete equality between the restored active settings and the saved snapshot. Existing exact folder, sort, first-member, hidden-real/synthetic-row omission and hidden-selection invariants remain required. Pointer targets must settle before each click.

The snapshot constructor is factored into one owner shared by saving and native validation, retaining the existing complete fields and saved identity. Test/visual-QA probes expose actual input and Reset rectangles; read-only getters inspect clear/restored state. No settings are set directly by the scenario after initialization; field values and actions flow through UI input. Five focused saved-filter regressions pass.

Native results and final checks follow below. Physical keyboard/VoiceOver, invalid-input recovery, other query modes/scopes and all archive formats remain separate gates. No commit or push performed.


Fresh native capture passed 14/14 checks with all recorded effects zero. The inspected framebuffer shows all four restored metadata values, exclusion *2.rs, Hide dotfiles, enabled Saved filters and Copy selected (2), inside the bounded sheet. The final scene gate proves a clear intermediate Reset state and complete snapshot restoration; the image alone is not used as proof of the intermediate actions. CUA returned application-not-found after the own process exited; fresh successful artifacts and unique launch metadata establish capture completion.

This closes the concrete native entry/Save/Reset/recall case for the four metadata filters plus dotfile/exclusion constraints. It does not qualify every input grammar, other scopes/modes, menu search/removal, multiple presets, VoiceOver, physical OS input or other archive formats. Historical pending reports and captures remain unchanged.

Final verification: five focused saved-filter tests and the locked visual-QA suite passed (2847 tests, 3 ignored). Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes accompany the logs and the current binary matches native launch metadata.
