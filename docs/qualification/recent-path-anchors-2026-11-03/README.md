# Anchored recent-folder path terms — 2026-11-03

Independent continuation of cohort #75 [fzf search syntax](https://github.com/junegunn/fzf#search-syntax): prefix/suffix exact terms and their inverse forms are adapted to Commander's existing Ordered path terms mode.

`^text` anchors a term at the beginning of the full remembered path; `text$` anchors it at the end. `!^text` and `!text$` exclude matching full paths. Other positive fragments retain ordered non-overlapping matching, with the last positive fragment in the final folder. Examples: `^/work src$ !archive` and `src !old$`. Backslash escapes a leading ! or ^ and a final $, including within exclusions. Empty operator-only tokens refuse rather than silently widening results. Existing 32-term/1024-byte input limits, Unicode lowercase behavior, host-separator normalization, ranking and captured-base scope remain shared.

Positive terms and edge-anchored exclusions ignore trailing path separators consistently; unanchored exclusions retain their earlier literal full-path substring scope. The Omnibar mode prefixes remain: initial ! means Shell mode, so recent-folder exclusions follow a positive term. No full fzf grammar or regex support is claimed. No filesystem reads or navigation are added by matching.

One new Rust regression covers positive/inverse start/end anchors, combined anchors, case/Unicode, literal operators (including an escaped ! inside an exclusion), ordered-term constraints, trailing separators, empty anchors and count bounds. Existing ordered/exclusion/ranking/history regressions remain green.

Seven gates passed. Final build and full tests were explicitly rerun after the final escaping/trailing-separator refinement: 3469 passed / 0 failed / 3 ignored. Strict final Clippy also passed. Earlier green intermediate logs are preserved under initial/; final build/test/Clippy logs are separate. No production edits followed the final build.

Native recent_forget retains its prior 76 phases and adds suffix success, suffix mismatch, wrong prefix and combined anchored-prefix/anchored-suffix/exclusion success, ending at phase 96. It passed 16/16 checks with zero native port effects. Independent Python verification checks 97 snapshots against ordered-fragment/regex-anchor predicates, unchanged usage/ranking/history and pane locations, source bytes and source/binary hashes. The final frame was visually inspected; the long input uses the existing horizontal text-field scrolling.

Reproduce using run_checks.py, final build/test/Clippy commands captured in their logs, prepare_capture.py plus CUA launch, then verify.py. Alternative terms/OR and quoted whitespace remain open.
