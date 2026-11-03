# Favorites matching field — 2026-11-03

Independent adaptation of cohort #77 [fd full-path matching](https://raw.githubusercontent.com/sharkdp/fd/master/doc/fd.1). Favorites picker offers Name + path (existing default), Name (saved display label only), and Full path (complete saved folder path only). These fields are not basename aliases: a favorite named “Проект GTK” can point to a folder named “Design”.

All five matching modes and three case policies use the chosen field. Both retains label-first ranking; single-field Fuzzy retains the existing scorer and nonfuzzy matches retain store order. Empty query shows the snapshot. Changing scope invalidates cache for unchanged queries, retires old rows, resets selection and cancels pending Enter until the new frame. Scope resets to Both on opening. No I/O or saved-bookmark mutation.

Existing limits/refusals remain: 128 query characters, 100 displayed results from the whole snapshot, bounded regex compilation, visible cached invalid-pattern errors. Full-path glob uses literal separators, so `**/Target*` can match saved paths whereas `*Target*` cannot cross `/`. New pattern modes skip non-UTF-8 path fallback; existing Fuzzy retains its prior lossy display matching behavior.

Two new Rust regressions cover all five modes across scopes, label/path disagreement, exact full paths, case policies, stable original indices, same-query scope cache changes, 150-row cap, error recovery and immutable store. Initial test compile failure and incorrect assumptions about Fuzzy order/full-path glob syntax are retained in focused logs. Seven final gates passed: 3517 tests, 0 failed, 3 ignored; audit/build/Clippy/default check/format/diff.

Native FavoritePicker extends from 102 to 124 steps, retaining existing navigation plus four case and nine pattern checkpoints. Five added scope checkpoints prove label-only/path-only differences and return to Both using actual controls. All 16 native checks passed on the final recorded binary with no external native effects. Final frame inspected. Short self-terminating QA can end before CUA accessibility observation returns; saved manifests/receipts establish completion without relaunch. Other platforms and persisted search options remain outside this transfer.

`run_checks.py`, `prepare_capture.py` and `verify.py` reproduce commands and validate source/binary hashes, manifests and all eighteen matching checkpoints.
