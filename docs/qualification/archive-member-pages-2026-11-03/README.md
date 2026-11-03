# Browse every archive group member — 2026-11-03

Large SHA-256 groups previously exposed only their first ten paths. Each group now has Previous members / Next members controls, a displayed range/total and bounded ten-path rendering. Original archive entry numbers and ordering remain intact, including duplicate paths. Group selection still applies to every member, with an explicit all-pages label, and full report exports are unchanged.

The page map is runtime-only and keyed by absolute group rank. Widget IDs are scoped per group; moving a group page does not change another group. New scans and completed results clear this map. Group pagination and member pagination share clamped, saturating range logic.

One new headless egui regression clicks through two independent 21-member groups, reaches the final member and verifies boundary refusal and original ordinal preservation. Existing group-page regressions continue to cover reverse navigation, empty/exact/partial pages, complete coverage and extreme arithmetic. The new member paging controls are qualified headlessly; native TAR and encrypted ZIP scenes cover integration regressions with smaller groups, not a large-member paging capture.

All seven gates passed: audit, build with visual-qa, 3463 tests passed / 0 failed / 3 ignored, strict clippy, ordinary check, fmt, diff. Both native scenes passed 14/14 checks with exactly two recording clipboard writes each. The independent Python verifier checks binary hashes, source archive bytes, member identities, exact selection transitions and complete text/JSON exports. Both final frames were visually inspected.

Reproduce: run_checks.py; prepare_capture.py plus CUA launch for each supported scene; verify.py. source-sha256.json records qualified source files. No new dependencies, archive rereads, extraction, deletion or saved preferences. Existing scan/export limits and SHA-256-plus-size comparison semantics remain.
