# Ordered recent-folder path terms

Independent adaptation of [zoxide's documented matching algorithm](https://github.com/ajeetdsouza/zoxide/wiki/Algorithm), cohort entry 78, consulted 2026-11-03. No upstream code copied. The new optional mode uses case-insensitive contiguous path fragments in order, literal slash constraints, and a final-keyword component that must occur in the final directory name. Trailing directory separators do not hide the last folder name. Unicode comparison uses lowercase text; no normalization/transliteration or zoxide score-formula parity is claimed.

Cmd+P Jump mode gains Ordered path terms. Disabled remains the old fuzzy search. Only recent-folder matching changes; favorites and commands retain their own matching. Original PathBuf identities reach navigation unchanged. Ordered mode ranks accepted paths with the existing frequency/recency score or preserves the chronological trail, without adding fuzzy-score bonuses.

The setting is saved via the existing session service, defaults to Fuzzy for old sessions, and is restored on launch. Changing it retains the query/open switcher, resets selection and restores input focus, suppressing same-frame navigation or a stale forget request. Failure to save is reported while keeping the choice in memory. Sorting/removal preferences remain intact.

Ordered queries are limited to 1024 UTF-8 bytes / 32 whitespace-separated terms. Invalid queries yield no recent matches and an explicit UI error; they never fall back to fuzzy matching. This is a bounded displayed-path matcher, not a filesystem scan. Paths with non-UTF-8 names use the existing lossy display text for matching while retaining original identities.

Regressions cover the upstream examples, order/separator/final-component refusal, repeated terms, Unicode case, trailing separators/root, byte/term limits, both ranking modes without mutation, real egui pointer and focused-Space toggle ownership, full session round-trips and missing-field compatibility. Native toggle/query/limit/reopen/save-failure qualification remains separate scope.

Final validation: all seven gates exited 0; tests 3371 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the existing diff_find.rs unused-response warning. verify.py checked four named regressions and matching final source hashes. A fresh full run includes the trailing-separator and whitespace-limit boundary cases; preliminary successful logs are retained.
