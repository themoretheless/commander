# Favorites ordering — 2026-10-31

Independently adapts [Double Commander hotlist entry sorting](https://doublecmd.github.io/doc/en/directoryhotlist.html#usage-sorting), cohort #4. Commander adds its own path and quick-jump slot choices; upstream code was not copied.

Three palette commands and Favorites row context menu → **Sort all Favorites** reorder the whole saved list by name, path or quick-jump slot. Name/path keys use Unicode lowercase and the existing natural comparator (numeric runs compare without integer parsing/overflow). This is not locale collation. Stable equal keys retain existing order. Slots 1–9 come first, unassigned entries retain relative order. All bookmark records and slot bindings are preserved; no folder probes, file moves or renames occur. This is a one-time operation, not an automatic sort preference.

The workspace clones a candidate, skips unchanged order without writing, saves through the existing guarded store, then installs the candidate. Blocked stores and pre-commit errors retain prior live order; existing persistence diagnostics report the error. Durability warnings use the existing committed-save policy.

## Verification

- `checks.json`: build, full tests, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks all passed.
- Full suite: **2956 passed, 3 ignored**, zero failed.
- Pure regression covers numeric names/paths, Cyrillic names, stable case-insensitive ties, assigned/unassigned slot ordering, metadata preservation and idempotence.
- Workspace regression dispatches palette commands, checks exact saved/reloaded order, skips unchanged writes, preserves navigation and slot identity, and refuses a blocked save. Existing injected conflict regression also covers sorted candidate rejection without replacement.
- Headless egui pointer regression opens the actual context menu and its sorting submenu and clicks By name, asserting the explicit sort action with no navigation.
- Native **regression** scene `favorites`: **14/14** checks, screenshot inspected, identical debug/core/launch binary hashes, zero native effects. It verifies existing rename/slot/navigation interactions after menu expansion. This scene does **not** click the new sorting submenu; native pointer/layout qualification of all three sort choices remains open. No host Favorites configuration was changed.

Artifacts include test/check logs, `source-sha256.json`, native `manifest.json`, `frame.png`, `navigation-state.json` and launch record. This scoped transfer does not establish full cohort parity.
