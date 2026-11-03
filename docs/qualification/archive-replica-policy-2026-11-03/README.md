# Archive replica count and keeper order — 2026-11-03

Independent continuation of cohort #85 [fclones replica preservation](https://github.com/pkolaczk/fclones): its preserved-replica count and ordering model are adapted to Commander's read-only archive selection workflow.

Keep copies per group accepts 1 through 100,000; First entries / Last entries rank by original archive index, independently of name, listing sort, member order and current page. Per-group/global selection actions use the same runtime policy and show its current count/order in their labels. Changing the policy does not itself mutate selection. Applying it selects redundant members, deselects the configured kept members and preserves unrelated selections. If the count reaches/exceeds group size, all members of that group are kept (deselected). The policy defaults to one first entry whenever an archive pane opens and is not persisted.

Identity resolution, malformed/overlapping/stale group refusals and mutation remain atomic. Count zero is refused before mutation. Groups are ranked without changing the scan report, listing or exports. Existing hash-plus-size matching and scan budgets remain unchanged; no extraction, deletion or archive rewriting is added.

One new Rust regression covers shuffled/non-monotonic entry indices, duplicate paths, first/last policies, counts 1/2/all/usize::MAX, preselected keepers, unrelated selections and zero refusal. Existing identity/refusal regression remains green. Numeric count application is qualified in Rust tests; native UI specifically qualifies switching to Last entries with count 1 and applying the global action.

All seven gates passed: audit, build, 3467 tests passed / 0 failed / 3 ignored, strict clippy, ordinary check, fmt and diff. Native TAR and encrypted ZIP each passed 14/14 checks. TAR retains all earlier scan/display/group/full-export/default-keeper checks, then clicks Last entries and the global action. Independent Python verification derives highest-index keepers from the fixture and checks exact selected indices [0,2,4,8,9]. Effects remain four recording clipboard writes for TAR, two for ZIP and zero for other ports. Both final frames were visually inspected.

Reproduce via run_checks.py; prepare_capture.py plus CUA launch for each scene; verify.py. source-reference.json records upstream behavior and source-sha256.json records qualified source. Path/time-based keeper priorities, pairwise byte comparison and archive deletion proposals remain open.
