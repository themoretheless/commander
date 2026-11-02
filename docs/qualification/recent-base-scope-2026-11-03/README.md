# Scope recent folders to the opening directory

Independent adaptation of [zoxide's base-directory filter](https://github.com/ajeetdsouza/zoxide/blob/main/src/db/stream.rs), cohort entry 78, consulted 2026-11-03. No upstream code copied.

Cmd+P Jump mode gains Recent folders under current folder. It captures the opening pane's directory, displays that base while enabled and filters recent destinations by path components before matching/ranking and row limits. The base itself and descendants are included; sibling string-prefix lookalikes are excluded. Original non-UTF-8 identities are retained. Scope is per switcher opening, defaults off and resets on reopening. Favorites and command rows retain their independent matching.

The filter is lexical and performs no filesystem probes or symlink resolution. Absolute base/candidate paths are required and parent-traversal components are refused. This is a navigation projection, not a physical-containment check. It composes with both fuzzy/ordered terms and frequency/chronological order without editing visits or their persisted ranking data. A scope change resets selection and restores input focus, suppresses stale Enter/forget actions and does not write a session preference.

While comparing the upstream filter implementation, its last-keyword matching exposed a prior weakness in Commander's ordered matcher: checking only the last fragment's basename could accept a complete final keyword from an ancestor. Matching now anchors the final complete keyword to the last path component boundary and searches preceding terms backward without overlap. Source examples distinguish foo/ from foo /, reject foo/bar in /foo/bar/other/bar, and retain the earlier Unicode, root, trailing-separator and bounds contracts.

Regressions cover component/root/sibling/traversal bounds, composition with both match/order modes, no visit/rank mutation, exact Unix non-UTF-8 identity, real egui pointer/focused-Space ownership and the expanded last-keyword cases. Native scope/reopen/cross-pane and visual-base-label qualification remain separate scope. No filesystem traversal or directory-discovery feature is claimed.

Final validation: all seven gates exited 0; tests 3374 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the existing diff_find.rs unused-response warning. verify.py checks the three new regressions plus expanded ordered-keyword cases and matching final source hashes. Preliminary successful logs are retained; the final run includes the unscoped path-vector move without an extra clone.
