# Select redundant archive members — 2026-11-03

Independent adaptation of cohort #85 [fclones replica preservation](https://github.com/pkolaczk/fclones): fclones preserves one replica by default and uses group input ordering to choose it. Commander applies this idea to its existing completed SHA-256 archive scans as a selection action.

Select all but first entry acts on one group. Select copies; keep first in each group acts on all retained groups, including hidden members and other view pages. The keeper is the lowest original archive entry index, independent of sorted listing positions, names and current UI order. All other members are selected, keepers are explicitly deselected even when previously selected, and unrelated selections remain unchanged. Tooltips describe this policy and scope.

Every affected member is resolved against listing index/path/size/type before any selection mutation. Invalid group ranks, missing results, malformed groups, overlaps and stale listing identities refuse atomically. No archive reads, extraction or deletion are performed. This builds a selection from SHA-256-plus-size matches; it does not add pairwise byte comparison or archive rewriting.

One Rust regression uses duplicate names and non-monotonic original indices to verify per-group/global behavior, preselected keepers, unrelated selections and atomic unavailable/stale/overlap refusals. All seven gates passed: audit, build, 3466 tests passed / 0 failed / 3 ignored, strict clippy, ordinary check, fmt and diff.

The native TAR scenario retains prior scanning, display, selection and report-export checks, then clicks the group action and global action. An independent Python verifier recomputes keeper indices from the TAR fixture oracle: group-only selection yields indices [2,4,5,8,9,10]; global yields [2,5,8,9,10]. Duplicate TAR names remain separate entries. Existing clipboard counts stay four for TAR and two for ZIP; all other native effects stay zero. Both native scenes passed 14/14 checks and both final frames were visually inspected.

Reproduce with run_checks.py, prepare_capture.py plus CUA launch for each scene, then verify.py. source-reference.json records reference behavior; source-sha256.json records qualified source. Configurable keeper policies, archive deletion proposals and pairwise comparison remain open.
