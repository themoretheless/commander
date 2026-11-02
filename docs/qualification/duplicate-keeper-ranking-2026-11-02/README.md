# Duplicate keeper name and path-depth ranking

Independently adapts [rmlint original-detection rank criteria](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86: alphabetical name order, basename length and path depth, in both directions. No upstream implementation copied.

Duplicate files → Keep now adds name A–Z, name Z–A, shortest name, longest name, shallowest path and deepest path. Existing shortest path, oldest and newest remain available. Clicking a different policy immediately replaces manual decisions in every group; Apply policy reapplies the chosen rule after manual changes/Keep all groups. Group sorting never changes keeper/member identities. JSON reports expose explicit stable policy keys.

Names compare in literal native OS order (case-sensitive, not locale/natural order). Name length counts encoded native bytes. Depth counts named parent components of the full lexical path, excluding root/prefix markers. Paths are not resolved or read to choose a keeper. Equal keys keep the earliest listed member; missing name/parent observations rank last in both directions. Existing unknown-date handling and shortest-path semantics stay unchanged. One kept member per nonempty included group remains invariant; Keep all groups skips deletion. This does not implement rmlint's multi-key rank strings or external-hard-link accounting.

## Validation

New pure regressions cover six distinct winners, stable ties, missing components, empty groups, native case/natural-order distinctions, byte lengths and invalid UTF-8 without lossy aliases. A report regression covers all six policy keys, exactly one keeper per included group, and all-skipped proposals. The existing rendered controls test now clicks every new policy through pointer events.

Native duplicate_ranking enables recursive discovery, selects all six policies, skips every group, reapplies the last rule, sorts groups, then restores shortest path. Every transition preserves ordered file paths, keep indices and the JSON report; seven fixture files are preserved for independent Python/hashlib key, digest and action verification. Neighboring duplicate_policy must retain chronological rules, Keep all/reapply and three intercepted CSV writes. All test/native acceptance is source/binary-bound. Arbitrary byte names and Windows rendering are not qualified by the macOS fixture; the invalid-byte regression needs no filesystem I/O.

Final qualification: all six commands passed; **3164 tests passed, 3 ignored**. Native `duplicate_ranking` and neighboring `duplicate_policy` each passed **14/14**. Ranking produced no external effects; legacy policy produced exactly three intercepted clipboard writes, with all other effects zero. Independent Python/hashlib checks validated all six distinct keeper winners, stable membership/index ownership, skip/reapply, sorted proposals, per-row actions/counts/bytes and final source/binary/tool hashes. Both final screenshots were inspected.

An earlier native attempt missed the first new policy click and timed out; its manifest remains in `native-captures`. Acceptance uses the later captured run with the same binary, assertions and 10-second deadline. Ordinary non-QA checking retains the existing unrelated unused `response` warning in `src/diff_find.rs:136`; visual-QA all-target Clippy with warnings denied passed.
