# Literal replacement with regex search

Independent adaptation of Double Commander’s separately enabled regex substitution: https://doublecmd.github.io/doc/en/multirename.html (section 6.2.4, consulted 2026-11-02). No upstream implementation copied.

The batch-rename dialog adds Literal replacement, enabled with regex search. When selected, replacement strings `$1`, `${name}` and `$$` are inserted exactly, while the find expression still uses regex syntax and capture matching. Turning it off retains the existing expansion behavior. Literal search remains literal regardless of this option. The option applies to primary and sequential pairs, shares existing target/first-match/case options, and is saved in rename presets. Old rules default to expansion; the false field is omitted in JSON. Disabled UI retains the choice for later regex use.

Regression coverage checks named/numbered/dollar replacement, first/all matching, sequential pairs and existing output bound, old/new JSON, preset persistence, preview cache invalidation/submitted rule, dialog rule load/reset, and filesystem rename with content preservation and Undo/Redo. Native pointer/layout qualification of this new checkbox remains open. No full Double Commander mask/substitution parity claim.

The preliminary compile log records an incorrect UI field name, corrected before qualification.

Final validation: all seven gates exited 0, including strict visual-QA Clippy. Full tests: 3335 passed, 0 failed, 3 ignored. Ordinary check retains the pre-existing unused-response warning in diff_find.rs. Source hashes and complete logs are retained.
