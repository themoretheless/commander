# Rename preset management

Independent adaptation of [Double Commander preset management](https://doublecmd.github.io/doc/en/multirename.html#presets), whose documentation lists renaming saved presets. Commander now offers **Rename selected** and **Duplicate saved**, taking the destination from the existing name input.

Rename changes only the saved label and retains list order/rule. Duplication copies the saved source rule, not unsaved editor changes. Both leave the editor intact; duplication retains selection on the source, so updating the selected rule still targets that source. Loading the copy explicitly selects it for editing. Normalized, case-insensitive collision rules and the existing 32-preset cap remain; case-only self-renames work. Errors leave the collection intact. No JSON schema change.

Two new core tests cover Unicode/case normalization, collisions, invalid/missing names, atomic refusal, independent copies, serialization, and capacity. The real atomic session save/load regression now persists both a renamed preset and a duplicate. Seven gates are recorded in checks.json; no native pointer-driven capture for these two new commands is claimed.

Result: seven gates passed; full suite 3317 passed, 0 failed, 3 ignored. Focused preset/session suite: 21 passed.
