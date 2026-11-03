# Exclude recent-folder path fragments — 2026-11-03

Independent adaptation of cohort #75 [fzf inverse-exact search tokens](https://github.com/junegunn/fzf#search-syntax), integrated into Commander's existing Ordered path terms mode in Cmd+P.

Examples: `src !archive` matches a final folder containing src unless its full path contains archive. Multiple exclusions are conjunctive and may occur between positive terms; positive terms retain their existing ordering and final-folder rule. Matching lowercases Unicode and normalizes the host path separator, as before. `\!literal` searches a literal leading exclamation mark. Empty exclusions such as `src !` refuse with an explicit error and no fuzzy fallback. Positive and negative terms together share the existing 32-term/1024-input-byte limits.

Omnibar routing is preserved: an input beginning with ! is still Shell mode, so enter exclusions after a positive term when searching recent folders. The tooltip documents the supported form and literal escape. This adds inverse substring exclusions, not the full fzf grammar (anchors, alternation, quoting and regex are not added). Fuzzy mode, history ranking, captured-base scope, persistence, file access and navigation semantics remain unchanged.

One Rust regression covers multiple exclusions, ancestor/full-path exclusion, Unicode/case, mixed token order, literal exclamation, ordered/final-component constraints, empty exclusion refusal, combined term/byte bounds, fuzzy-mode error separation and unchanged usage stats.

All seven gates passed: audit, visual-qa build, 3468 tests passed / 0 failed / 3 ignored, strict clippy, ordinary check, fmt, diff. Native recent_forget was extended from 61 to 76 scripted phases while preserving earlier ranking/removal/mode/persistence/oversize checks. New pointer/text phases type `kee !KeepMe` (no results), `kee !` (error), then `RECENT_FORGET / KEE !ForgetMe` (recovery). It passed 16/16 checks, with zero native port effects. Independent Python verification checks 77 snapshots, query/visible rows, ranking/history preservation, source bytes, binary/source hashes and unchanged pane locations. The final frame was visually inspected.

Reproduce using run_checks.py, prepare_capture.py plus CUA launch, then verify.py. An initial verifier invocation occurred before the seven-gate runner finished and correctly refused the incomplete check list; it was rerun after the same runner completed, without restarting builds or tests. No new dependencies or external processes are involved in matching.
