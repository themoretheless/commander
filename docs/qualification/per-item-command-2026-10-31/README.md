# Sequential per-item commands — 2026-10-31

Independently adapts [Strata whole-selection/per-item execution and error policies](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#how-an-invocation-runs) to Commander's existing shell-command owner.

## Behavior

Run command → Template name and file types → **Execution** offers Whole selection (legacy default), Per item: stop on error, and Per item: continue. Per item captures the ordered opening selection and runs one invocation at a time. Paths/names expand against exactly one current entry with the existing shell quoting/dash anchoring/opaque substitution rules. Opening dir/other-dir and process CWD remain captured and are not retargeted by later pane changes. Preview explicitly shows the first of N invocations. Empty per-item input is refused. Existing count/kind/whole-selection constraints apply to the complete opening selection before dispatch; legacy extension suggestions are not a request to silently filter that selection.

Live counters show completed invocation attempts, successes and failures. Final structured report includes total/completed/succeeded/failed, cancellation and stop-on-error state; remaining count is retained even when output is truncated. Each invocation has an item/path heading. Failures always record exit/error/signal information, including silent nonzero exits. Continue processes remaining entries; stop-on-error retains earlier results and leaves later entries unstarted. Aggregate stdout/stderr each retain at most 1 MiB plus fixed summary/truncation text; the existing per-process capture/drain bound remains in place.

Stop/close sets a shared cancellation flag before stopping the current process through the existing process-group owner. The plan checks it between invocations; shell publication checks it under the child-slot lock before spawning, preventing a missed pre-publication cancellation. A new run gets a fresh cancellation/progress owner. User cancellation has an informational outcome; completed file changes are not rolled back. Execution remains through the existing `sh -c` owner, selection binding, non-UTF-8 preflight and opening-root trust/mutation gates.

Mode is preserved in saved metadata and Commander JSON **version 5**. Missing mode remains Whole selection and is omitted on serialization; versions 1–4 are accepted with supported policies. Old envelopes carrying non-default execution mode and unknown mode strings are refused. [example.json](example.json) is a pasteable per-item name-printing template.

## Verification

**2985 tests passed, 3 ignored**, zero failed. Build, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks passed; see `checks.json` and logs.

New model tests exercise exact per-item order/one-path expansion/other-dir binding, stop/continue policies, cancellation between items/pre-cancel, whole-selection single-call compatibility and aggregate truncation. Real Unix shell regressions verify spaces/apostrophes/dash/token-shaped filenames, both error policies with actual ephemeral file outcomes, item-bound silent `exit 7` diagnostics, live process cancellation and untouched later paths, and cancellation before child publication with no marker creation. Serialization tests cover default/empty-mode handling, v5 round trips, old-envelope refusal and valid v4 kind-policy compatibility.

Native **per_item_command**: **16/16** capture checks, framebuffer inspected. Real input searches/selects the saved per-item template through All templates, activates Run, executes two separate harmless printf invocations and scrolls to completion output. Actual report is 2/2 completed, 2 successes, 0 failures, no cancellation, exit 0 and empty stderr. Independent assertions over `per-item-state.json` require exactly one complete output line per selected entry in original order. Binary/core/launch hashes match; source hashes are recorded. Native effect-port counters remain zero; shell execution is intentionally exercised separately from those ports. The scene is in the CI matrix (61 definitions); this run captures this scene only.

## Boundaries

Native happy-path execution is qualified; error/Stop/cancellation cases are real-process regressions rather than separate native captures. Native opening context/template data are fixture-injected, while the normal Run trust/mutation checks still execute. No host templates/trust settings are written; commands print fixture names only. The earlier green evidence is retained under `before-error-diagnostics`.

The existing shell/process-group and pipe-capture lifecycle remains authoritative; this adds no detached-program lifecycle contract, global Jobs dashboard, parallel per-item execution, retries/resume or transaction undo. Counts/kinds use the opening snapshot, not fresh identity/MIME validation. This is not Strata TOML/argv/runtime compatibility or full 99-repository parity.
