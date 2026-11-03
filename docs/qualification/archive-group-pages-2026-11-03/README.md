# Archive content-group pages — 2026-11-03

The archive SHA-256 view previously rendered only the first 100 groups. Previous groups / Next groups now exposes every retained group, at most 100 per page, with an absolute range and total. Selection buttons retain the original absolute group rank; global selection and complete text/JSON reports remain independent of the displayed page. New scans/results reset the runtime-only page. Stale/empty page state clamps safely before arithmetic.

Two Rust regressions exercise empty/exact/partial pages, complete coverage through 10,001 groups, stale state and usize limits, plus actual egui pointer clicks across 201 groups, final-page refusal and return to the first page. These page transitions are headless UI tests, not a native large-archive capture. Existing TAR and encrypted ZIP native scenes independently qualify group selection and report export with the final binary; each passed 14/14 checks with exactly two recording clipboard effects. Both final frames were visually inspected.

All seven gates passed: audit, visual-qa build, 3462 tests passed / 0 failed / 3 ignored, strict clippy, ordinary check, fmt and diff. See checks.json and raw logs. Initial test compilation failures (integer inference and the egui run_ui API) are preserved under initial/; both were fixed before the final run. The copied independent TAR fixture oracle is checked against captured archive bytes and Python tarfile iteration.

Reproduce: python3 docs/qualification/archive-group-pages-2026-11-03/run_checks.py. Native captures use prepare_capture.py followed by CUA launch of target/CommanderFolderQA.app; verify.py validates both recorded launches, source archive hashes, exact group members, selection transitions and exports.

Limits remain: only ten member paths are displayed per group; full membership is available through report export and group selection. SHA-256 plus size is not a pairwise byte comparison. This change adds no archive reads, extraction, deletion, or persisted state.
