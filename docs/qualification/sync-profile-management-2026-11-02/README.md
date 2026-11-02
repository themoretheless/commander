# Managing saved synchronization profiles

Continuation of the Krusader-inspired synchronizer profiles. Saved sync profiles now offers **Rename selected to name** using the existing name field, and **Delete selected profile**. Neither action loads a comparison or touches files. Rename preserves every comparison input and list position; duplicate or invalid names are refused. Delete removes only the selected saved record and clears selection, including when removing the last record. The versioned session format is unchanged and an empty collection reopens normally.

All collection mutations serialize and enforce the existing 64 KiB bound before publishing entries/selection. Errors leave stored JSON and comparison inputs unchanged. Invalid stored books remain protected from editing until explicit Clear. A new regression checks exact input preservation, collection/session representation round trips, collision and malformed-name non-mutation, selected-only deletion, absent-selection refusal and deletion of the last record.

Validation results are recorded in checks.json and tests.log. Native pointer/keyboard interaction and visual layout remain unqualified for this section. Existing full-suite checks do not prove native interaction. No dependencies were added; the wider cohort transfer scope and previous dependency-audit failures remain open.

Final checks: all six gates passed; full suite 3,278 passed / 3 ignored. Source and executable hashes were rechecked after the gates completed.

Later native save/rename/delete qualification: [actual pointer/key workflow](../sync-profiles-native-2026-11-02/README.md). The native gap above describes this earlier snapshot.
