# Synchronizer review name masks

Independent adaptation of [Krusader synchronizer filename masks and selection actions](https://docs.kde.org/trunk_kf6/en/krusader/krusader/synchronizer.html). Commander retains its explicit review workflow: enabling **Name masks** changes how Find plan rows matches names; it does not automatically omit unseen rows from Sync. Existing Skip shown, Restore shown defaults and Reverse shown commands operate on the matching original row IDs. The UI continues to show the hidden-planned count and warns that Sync includes hidden rows.

Whole-name * and ? globs are case insensitive. Commander uses comma/semicolon separators, allowing spaces within filenames, and leading ! or - exclusions. Positive terms are OR, exclusions win; exclusion-only expressions start from all rows. Empty input shows all rows subject to the current scope. This reuses Commander's mask tokenizer and Unicode glob matcher; extension shorthand and Krusader pipe syntax are not supported in this mode. Query length remains bounded at 128 characters. Reset view clears mask mode, query, scope and ordering. Mode participates in the cache key, so the same text is re-evaluated when toggled.

The regression verifies case-insensitive inclusion/exclusion, exclusion-only and OR/question-mark matching, unchanged plan before explicit editing, exact Skip shown targets with hidden rows preserved, cache hits/mode invalidation and empty query. Native interaction/layout for this toggle remains unqualified. This is a review selection tool, not a recursive comparison input filter and not saved with comparison profiles. No dependencies were added. Wider cohort parity and previous dependency-audit failures remain open.

The initial failed run is retained in initial-failed/: its regression used space-separated patterns, revealing a mismatch with the existing Commander tokenizer. The example, tooltip and regression now consistently use semicolon separators. Final qualification was restarted after the source correction.

Final validation: all six gates passed; full suite 3280 passed / 3 ignored. Source/binary hashes were rechecked. Native UI interaction remains unqualified for this new toggle.

Subsequent native qualification: [pointer/key workflow and byte-comparison control](../sync-masks-native-2026-11-02/README.md). The earlier native gap above describes this original snapshot; the linked bundle qualifies the later mask workflow.
