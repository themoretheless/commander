# Optional invalid-character replacement in batch rename

Independent adaptation of Double Commander’s Replace invalid filename character by setting (section 10.2): https://doublecmd.github.io/doc/en/multirename.html, consulted 2026-11-03. No upstream implementation copied.

The batch-rename rule/editor adds an opt-in replacement for slash and NUL, matching Commander’s existing prohibited-character policy. Default off preserves earlier refusal behavior. Enabled empty text strips those characters; otherwise each occurrence is replaced literally with the chosen text. Replacement is at most 32 UTF-8 bytes and cannot contain slash/NUL. This does not claim the complete set of Windows or other platform restrictions.

The shared planner performs replacement on the final generated name after existing transformations and before target validation/collision checks. Invalid replacement settings produce a rule error and invalid plans; empty/dot/dot-dot/oversized final components remain invalid. Introduced collisions are visible and block the batch. Expanded output length is checked before replacement allocation with the existing 64 KiB output limit; no recursive replacement occurs. Parent/date policy validation remains separate.

Presets retain the optional replacement, including enabled-empty versus disabled. Older JSON defaults to disabled and omits the absent field. Editor load/reset and cached-plan equality include the setting. Filesystem application uses the same rule/planner as preview and retains existing identity/race/collision checks and Undo/Redo.

Regression coverage includes default refusal, replacement/strip, literal POSIX punctuation, old/new JSON, malformed/oversized replacement text, empty/dot targets, intra-batch and sibling collisions, bounded expansion, preset roundtrip, cached preview/submission and real filesystem apply/Undo/Redo with unchanged contents. Native checkbox/text-entry/layout qualification remains open; no full Double Commander mask/platform parity claim.

Final validation: all seven gates exited 0; tests 3346 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. Final source hashes and focused/full logs are retained.
