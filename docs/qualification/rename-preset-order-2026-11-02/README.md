# Saved rename preset ordering

Independent adaptation of the sorting capability documented in [Double Commander preset management](https://doublecmd.github.io/doc/en/multirename.html). **Sort A–Z / Sort Z–A** explicitly order saved rules by NFC-normalized lowercase Unicode keys. This is deterministic Unicode ordering, not locale-aware dictionary collation. **Preset up / Preset down** move the selected saved rule one slot. Boundary and stale selections do nothing; their controls are disabled.

The existing array order persists without a schema change. Whole preset records move together, retaining rule/name/launch choice. The active editor, selected label and unsaved rule are not reloaded. Only actual ordering changes trigger persistence. Button focus/clicks take part in the dialog's existing Enter-submission exclusion.

Core regression covers Unicode canonical lookup, both sort directions/idempotence, manual movement/boundaries, saved-rule identity, launch preference and serialization. A pointer-input egui regression clicks all four controls and verifies final order/default. Native window and OS input capture for the new ordering controls remains open. Seven gates are recorded in checks.json.

Result: seven gates passed; 3321 passed, 0 failed, 3 ignored. Pointer-input regression passed. Actual atomic session save/load also verifies the sorted order and unchanged launch choice. Preliminary incompatible egui test API compiler error retained separately and corrected to run_ui.
