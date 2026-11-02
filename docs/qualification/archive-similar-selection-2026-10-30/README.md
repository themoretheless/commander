# Archive same-name and same-folder selection — 2026-10-30

Independent adaptation of [Double Commander MarkCurrentName / MarkCurrentPath and inverse commands](https://doublecmd.github.io/doc/en/cmds.html), cohort #3. Archive selection actions now include Same name and Same folder, alongside the existing Same extension menu. Each exposes Select visible matches and Deselect visible matches.

Same name compares regular-file final-component stems after removing only the last extension, ignoring case through the existing cached lowercase paths. Thus Archive.tar.gz matches archive.tar.xz, .env matches .env.txt, and duplicate paths retain independent row IDs. Directories, links and other records are excluded, including overlapping file/link flags. Missing lower-path cache, hidden focus or synthetic focus produces no rows.

Same folder compares exact lexical internal parent paths, with case preserved. It can use any focused visible real member and includes all visible real record types in that same parent. Root-level names share the empty internal parent; no host folder lookup is performed. Src and src remain distinct. Synthetic navigation rows are never selected and directory subtrees are never expanded implicitly.

Both actions preserve hidden choices and current view/filter/sort/folder state. Successful application resets the Shift range anchor through the existing owner; it does not overwrite saved selection memory. A shared popup action owner gates disabled/stale focus and uses the same matched IDs as the core. No archive payload read, worker or filesystem mutation is added.

Three regressions cover final-suffix/case/dotfile semantics, duplicate identities, source order, invalid/hidden/synthetic focus, case-sensitive parents and root/record types, plus actual egui pointer Select then Deselect for both menus with a hidden selected member retained throughout. Native menu/action/focus/layout remains pending: CUA is not exposed in this continuation and no native launch was attempted. Earlier captures do not qualify the added controls.

Validation: locked visual-QA suite passed 2828 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
