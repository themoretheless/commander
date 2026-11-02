# Batch selected-folder Favorites — 2026-10-31

Independently adapts [Double Commander Directory Hotlist: adding multiple directories](https://doublecmd.github.io/doc/en/directoryhotlist.html), section 3.5, cohort #4.

The palette command **Add selected folders to Favorites** appends selected visible directory rows from the active pane, in current display order. Files, filtered/hidden rows and selections retained in other folders do not participate. An empty selection does not fall back to the cursor. Directory links use the listing's directory classification and retain their lexical path; the command performs no new path observations or canonicalization.

Existing entries retain their labels, slots and order. New entries use the listing name and have no quick-jump slot. All candidate names/absolute UTF-8 paths are checked before a single guarded save; only a successful commit installs the live model. Invalid input and pre-commit errors retain the old model; a zero-addition batch does not write. Existing persistence diagnostics and command notices report results. Selection and navigation are unchanged.

## Validation

Full suite: **2958 passed, 3 ignored**, zero failed. See `checks.json` and logs for the final run. The workspace regressions exercise real directory listings, a mixed selection, natural display order, existing metadata/slots, persisted reload, idempotence, hidden rows, blocked persistence and whole-batch rejection for a control-character name. The earlier failed test setup is retained under `initial-test-setup`; final setup explicitly refreshes the test pane and accounts for loader-side hidden-file exclusion.

This change has core/command regression coverage; no dedicated native palette capture was performed for the new command. It does not implement hotlist tree groups, configurable insertion positions or filesystem availability scanning. User configuration was not touched by the injected-persistence tests.
