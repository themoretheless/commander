# Edit reviewed rename proposals as a name list

Independent adaptation of Double Commander’s New File Name column list editing (section 5.3): https://doublecmd.github.io/doc/en/multirename.html, consulted 2026-11-03. No upstream implementation copied.

Edit preview names validates the current complete rule against the captured context, collects the current valid target names, then loads an enabled name-list-only rule. Other transforms reset because their effects are already present in the collected names; the resulting preview remains identical rather than repeating prefix/counter/case operations. Original context/source identities and preset selection/name remain owned by the existing editor; this does not save a preset, apply a rename or read files. The tooltip/status explicitly describes modifier reset.

Invalid/colliding/count-mismatched proposals, unresolvable plans or names that cannot round-trip through the plain list format fail before editor replacement. Unchanged valid targets may be loaded for editing. The shared original/preview list constructor preflights 1–10000 rows and 256 KiB before joining; it refuses CR/LF content, empty records and leading-BOM ambiguity rather than silently altering names. Unicode, whitespace and non-leading BOM content remain exact. Existing list byte/count validation and filename/collision checks still apply after subsequent manual editing.

Both source buttons participate in the existing focus/Enter interaction guard. Start from original names keeps its existing seed-and-transform semantics but now uses the same lossless/bounded constructor.

Tests verify equality of complete from/to/status plans before/after preview promotion with prefix/suffix/case/counter, reset of other rule fields, invalid-count/collision/control/BOM refusal, unchanged-name promotion, constructor roundtrip and exact size boundary. Twelve focused name-list regressions passed. Native button clicks, preset-selection retention in the full window and external-editor/file-import workflows remain open scope. This is an in-dialog adaptation, not full upstream external-editor parity.

Final validation: all seven gates exited 0; tests 3353 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. Source hashes match qualified code; focused/full logs retained.
