# Duplicate groups spanning captured folders — 2026-10-31

Independent adaptation of [fclones isolate selection](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md#finding-files), research cohort #85.

With **Include other panel folder**, **Only groups spanning folders** restarts the worker and retains verified groups containing distinct physical copies assigned to different captured roots. It removes sizes confined to one root before hashing and checks the final full-digest groups again. Overlapping roots use the most specific lexical root, shared with row labels; repeated roots cannot manufacture a crossing. Minimum copies still counts retained physical copies. Disabling the secondary root ignores the option but preserves the preference. JSON includes optional `only_cross_root_groups` only in two-root mode.

This is Commander's two-root selection policy, not complete fclones CLI isolate parity. Hard-link aliases remain collapsed before root ownership; aliases in another root alone do not qualify a group. Arbitrary root lists, fclones hard-link path semantics and full 99-repository parity remain open.

## Validation

- Six checks passed: locked QA build, full QA tests, all-target QA Clippy with warnings denied, default check, formatting and diff whitespace. **3069 passed, 3 ignored**. Default check retains the pre-existing QA-only response warning in `diff_find.rs`.
- New worker regressions cover same-size different contents, local-only size pruning, minimum three physical copies, overlapping-root order, repeated roots and hard-link aliases. Existing state reconstruction and JSON tests cover preference retention and optional context.
- Native `duplicate_cross`: **14/14** manifest checks, 24 egui pointer-input phases. Eight independently checked snapshots prove root-only union → crossing-only → recursive crossing → recursive union → crossing → single-root (preference retained) → crossing restored. Exact path sets exclude the two within-root groups and retain cross.bin plus nested/deep.bin.
- JSON: two groups, four files, two proposed trash files / 23 bytes, one keeper each, applied crossing context. One recording clipboard write; no filesystem mutation effects.
- Existing `duplicate_roots`: **14/14**, four union groups / 42 proposed bytes, crossing mode false. This retains prior captured-root/fresh-fallback scenario coverage.
- Screenshot inspected: both root and crossing checkboxes, recursive controls, two correctly labeled groups and footer visible. Native launch uses CUA; assertions inspect retained QA artifacts. Input is egui QA injection, clipboard is a recording backend; no claim of physical input, OS clipboard or executed Trash.

See `checks.json`, `native-assertions.json`, `source-sha256.json`, `native/`, `native-roots/` and associated logs. Launch records bind the captures to the tested executable SHA-256.
