# Archive saved-filter menu search — 2026-10-30

Independent adaptation of [Far Manager filtering of menus and dropdown lists](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Saved filters now has an explicit Search saved filters field. It matches a trimmed Unicode-lowercase substring against the complete displayed description, including the saved query, scope, mode, type and metadata constraints. The menu search is limited to 128 characters and belongs to the loaded listing, separately from active/saved query settings. It does not enter saved identity, query history caps or Reset filters.

A match count reports how many saved entries are visible. Clear search reveals all entries. Clear all saved filters explicitly removes all snapshots, including hidden ones, preserving the active filter. Recall and Remove continue to identify the original full snapshot rather than a filtered row number. Existing bounds remain 20 entries / 16 KiB source text. The description formatter is shared by display and matching; no member I/O or query recompilation is introduced by menu search.

The popup uses CloseOnClickOutside so clicking/editing its search field does not dismiss it; preset actions retain explicit close behavior. The initial pointer failure exposed the previous default CloseOnClick behavior and is preserved. The match-count probe uses an explicit usize type.

A headless pointer regression types a Unicode search into the actual field, verifies that active query/cache/saved entries remain unchanged, and independently recalls/removes the matching original-index-1 snapshot. Existing recall and removal coverage is rerun. Native entry/focus/menu positioning/scroll and active-filter preservation remain pending because CUA is not exposed. No native launch was attempted, older captures do not qualify changed controls, and no commit or push was performed.

Final validation: 44 focused tests and the locked visual-QA suite passed (2841 tests, 3 ignored). Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source and binary hashes accompany the logs. Native validation remains pending.
