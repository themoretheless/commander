# Favorites letter-case matching — 2026-11-03

Independent adaptation of cohort #75 [fzf case controls](https://raw.githubusercontent.com/junegunn/fzf/master/man/man1/fzf.1). Favorites picker now offers Ignore case, Smart case and Match case. Smart case uses Unicode uppercase detection. The effective policy applies equally to favorite labels and path fallback through Commander's existing fuzzy scorer. Existing label-first ranking, stable ties, 100-result cap, 128-character bound and immutable opening snapshot remain in force.

Default remains Ignore case; choice lasts for the current opening only. Changing policy invalidates the ranking cache even with unchanged input, clears old rows and selection, and cancels pending Enter submission until the next frame. It never probes paths or mutates the saved Favorites store. Other omnibar modes retain their existing semantics. This is not fzf query-language or Unicode normalization compatibility.

New Rust regression covers ASCII and Cyrillic modes, name/path fallback, cache transitions with the same query, empty and oversized queries. Full seven gates passed: audit/build/tests/Clippy/default-feature check/format/diff; 3513 passed, 0 failed, 3 ignored. See `checks.json` and logs.

Extended the existing native FavoritePicker scenario from 38 to 57 steps, retaining background/new-tab/other-panel actions and reopen/default checks. Four new checkpoints independently capture Match/lowercase → zero, Smart/lowercase → one, Smart/uppercase → zero, Ignore/uppercase → one, against “Проект GTK”. All 16 native checks passed on the recorded final binary, with no external native effects. Final frame inspected: controls and selected result are legible. CUA reported app absent after the successful short capture; receipts establish completion without a relaunch. Wider platforms and persistent case preferences remain outside this transfer.

`run_checks.py`, `prepare_capture.py` and `verify.py` reproduce the recorded checks and validate hashes/receipts.
