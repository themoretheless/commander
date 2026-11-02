# Archive Regex and Smart case filter — 2026-10-30

Independent Rust adaptation of [Yazi's regex/case filter](https://github.com/sxyazi/yazi/blob/main/yazi-fs/src/filter.rs) and [documented smart-case filtering](https://yazi-rs.github.io/docs/configuration/keymap/#filter), cohort #27. No upstream implementation copied. Commander applies the interaction to already-listed archive paths without member content reads or extraction.

The archive filter now exposes Regex, Match case and Smart case. Defaults remain literal substring and case-insensitive; existing Unicode-lowercased path matching and literal whitespace trimming are retained. Explicit Match case wins over Smart case as in Commander's preview search. Smart case inspects Unicode uppercase query characters, including regex syntax; inline regex flags retain their meaning. Regex runs against the full displayed internal path and preserves entered whitespace. No fuzzy matching, wildcard dialect, Yazi byte-strand normalization/highlighting or complete flag-combination parity is claimed.

Query admission is 256 Unicode characters. The shared preview compiler applies 1 MiB NFA/DFA limits and nesting 32. A compiled pattern or its error is cached by query/options; folder navigation reuses it. Query/options/folder changes rebuild matches, while sort changes keep row identities. All-members, direct-child folder rows and synthetic child-folder navigation use the same matcher. Invalid patterns show their error and no real/navigation rows rather than silently restoring all rows. No selected member IDs are removed; selected clipboard/extraction scope remains independent of visibility.

If filtering hides the focused row, its cursor is cleared and the list resets its scroll. This also fixes stale Enter selection in literal filtering: the old hidden row cannot be toggled through a cursor that no longer appears. A visible cursor survives filtering/sorting. Arrow movement with no rows returns no cursor. The options are session state and reset with the archive listing owner; controls are disabled while its initial listing loads.

Six new regressions cover literal compatibility/Unicode folding, regex/Unicode/manual/smart/inline case policy, invalid/query/depth limits and meaningful regex whitespace, real/navigation row matching with hidden selection, invalid-pattern recovery/cursor clearing, and literal cursor/limit behavior. Existing archive sort/copy/navigation regressions remain part of the full suite. Initial test compilation missed a fully qualified Path in a test; corrected and retained in `test-initial.log`.

Native window validation remains pending because the desktop CUA tool is unavailable. No native filter click, fresh frame/layout result or completion of the previous ArchiveExport native gate is claimed. Required follow-up: toggle the three options, enter valid and invalid regexes, navigate filtered folders, verify the empty invalid state and hidden selected export, then inspect a fresh native frame.

Verified core gates: full locked visual-qa suite **2736 passed, 0 failed, 3 ignored**; all-target visual-qa Clippy with warnings denied, locked default-feature check, locked visual-qa build and fmt/diff checks passed. Final binary/build hash is recorded with the source snapshot; native window validation remains pending as described above.

Broader transfers remain open; no commit or push.
