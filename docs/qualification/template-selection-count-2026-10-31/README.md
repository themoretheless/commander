# Command-template selection counts — 2026-10-31

Independently adapts [Strata action min_items/max_items constraints](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#manifest) to existing saved shell templates.

## Behavior

Run command → Template name and file types offers **Minimum items** and optional **Maximum items**. Bounds are inclusive and count the entire immutable opening selection, not the subset matching an extension. Set 1–1 for a single-input tool or minimum 2 without a maximum for batch work. Default minimum 0/unbounded maximum preserves existing behavior. Bounds use portable u32 values; zero maximum is supported by the model. Reversed ranges are retained for correction but disable Save/update and Run; invalid imported ranges are refused.

Count constraints compose with existing extension and whole-selection policy. They apply even without extensions or with legacy any-match mode. Browser applicability honors them; All templates can reveal a record but cannot bypass the actual Run/Enter gate. Selecting a saved row restores limits into the draft. Successful metadata updates retain original row identity/order. Source selection and command expansion are unchanged.

Limits are defaulted/omitted in legacy persistent records. Commander JSON export now uses **version 3** and includes configured limits. Versions 1/2 remain readable with their supported policies; either old version carrying non-default count constraints is refused. Version 1 still cannot carry enabled whole-selection matching. Nested unknown fields, negative/overflowing bounds and invalid ranges fail validation. Import review displays configured counts. [example.json](example.json) illustrates one-item media inspection; no example command was executed.

## Verification

**2973 tests passed, 3 ignored**, zero failed. Build, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks passed; see `checks.json` and logs.

New regressions cover exact/open/zero/inclusive/invalid ranges, composition with extension/whole-selection rules, limits without extensions, in-place metadata update and persistence round trip, old defaults, malformed numeric/nested fields, v3 round trip, v1/v2 downgrade refusal and valid v2 whole-selection import. Browser tests cover selection-size and policy-content cache invalidation plus All templates without Run bypass.

Native `template_browser`: **16/16** capture checks, framebuffer inspected. Two fixture rows are captured in the opening selection. A one-item template is loaded through All templates with whole-selection matching disabled, proving count policy independently blocks the actual Run response. Pointer down/up on the disabled button yields no run and no output; search plus simultaneous Enter, row selection and Clear also complete. `browser-state.json` records actual count 2, limits 1–1, preserved store limits, response enablement and execution/output state. `native-validation.json` independently asserts them and identical binary/core/launch SHA-256. Native effect counters are zero; source SHA-256 are recorded.

## Boundaries

The native fixture injects the opening context/template model without host template or trust writes. It tests loaded limit enforcement, not native spinner editing or positive command execution. Model/serialization tests cover configured bounds and persistence semantics. Existing trust, quoting, output/cancellation and legacy persistence ownership remain unchanged. This is not Strata TOML/runtime/MIME/kind compatibility or full 99-repository parity.
