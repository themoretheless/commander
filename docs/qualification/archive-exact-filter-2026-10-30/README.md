# Exact literal archive filtering — 2026-10-30

Independent adaptation of [fd exact filename/full-path matching](https://github.com/sharkdp/fd/blob/master/doc/fd.1) (`--exact`), cohort #77 in the 99-project research. The archive browser now exposes Exact beside Regex.

Exact compares the entire candidate (full internal path by default, final component with Filename only) as a literal string. Regex metacharacters have no syntax and query edge spaces are retained. Existing Match case / Smart case rules and cached lowercase candidates apply. Empty input retains the established no-path-filter behavior. Default literal-substring trimming remains unchanged.

Regex and Exact are mutually exclusive in the UI; enabling either switches off the other. The compiler also rejects conflicting flags, including an empty query, instead of silently choosing a mode. Bounds and error handling remain shared. Exclusion complements the exact predicate; size/type/folder/narrowing continue to intersect separately. Both real and synthetic navigation rows use the same compiled owner.

Exact is part of Options identity, so changing it rebuilds the existing cache and saved-filter full keys distinguish exact and substring snapshots. Saved labels show Exact, recall restores it, and Reset returns to substring/full-path defaults. Hidden selections and duplicate member IDs remain intact. No archive payload or worker I/O is added.

Two parser/pipeline regressions cover literal metacharacters, non-substring behavior, preserved edge spaces, case, both scopes, blank/conflicting modes, cache rebuild, duplicate IDs, navigation/exclusion, hidden selections and saved/Reset state. Native checkbox exclusivity, literal entry, saved recall, focus and layout remain pending: CUA is not exposed in this continuation and no native launch was attempted. Older archive captures do not qualify the changed controls/labels.

Validation: locked visual-QA suite passed 2817 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
