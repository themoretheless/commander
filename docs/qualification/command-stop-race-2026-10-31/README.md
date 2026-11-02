# Stop process-group ownership and pipe-drain race — 2026-10-31

Reliability follow-up to Commander's QtFM/lf-inspired captured and per-item custom commands (cohort #26 / #32), not a new upstream-code port. The recurring five-second Stop test timeout was reproduced rather than hidden by a longer timeout or ignored test.

## Reproduction

Eight parallel test processes ran the unchanged real-shell per-item cancellation case 64 times. Two attempts timed out. Their scoped process snapshots show the shell gone while an orphaned `sleep 30` remained in that shell's process group, holding the output pipes (`stress-failure-38.log`, `stress-failure-39.log`). The worker had reaped the shell with `try_wait`, then blocked joining pipe readers after a single group-kill attempt. Dropping the failed test's run made another Stop call and released the surviving holder. The retained logs print only members of that owned group, not the host's full process list.

## Change

Unix command workers now observe leader termination using [waitid WNOWAIT](https://man7.org/linux/man-pages/man3/waitid.3p.html), retaining the waitable leader until both output readers finish. That retains the PID while Stop still needs its [process group](https://man7.org/linux/man-pages/man3/kill.3p.html). While cancellation is requested, the worker repeats SIGKILL to its owned group during polling, catching a descendant left alive during the concurrent shell/fork/Stop window. It reaps the leader and clears the child slot under the same mutex used by Stop, closing the window for a late Stop to use an already-retired identifier. Unix Stop no longer calls a separate `Child::kill` path that could disturb that ownership protocol; the existing non-Unix fallback is unchanged.

The two existing output-drain threads notify the worker when their reads finish, avoiding a mandatory extra polling delay just to discover pipe completion. The channel carries at most two completion notifications. Existing per-stream caps, draining, output formatting, sequential scheduling, cancellation-before-publication gate and cancellation result/report owners remain. Commands run in the retained working directory/selection context as before.

The older whole-command Stop regression now supplies the same shared cancellation progress to its worker that production uses, instead of its standalone test convenience wrapper. A new real-process regression leaves an exited shell with a background pipe holder, requires the leader to remain waitable, then verifies Stop completes output and retires the slot; a repeated late Stop sees the retired slot. Diagnostic detail was added to the original timeout assertion without increasing its deadline.

## Verification

**3045 tests passed, 3 ignored, zero failed**. All six checks exit zero: visual-qa build/full tests/all-target Clippy with warnings denied, default check, formatting and diff whitespace. The focused real-process suite passes all five cases, including pre-publication cancellation, per-item policy/quoting and working-directory behavior. Existing whole-command Stop and capped-output tests remain green.

The final-source stress repeat passed **256/256** with eight parallel processes, versus **62/64** before the fix. Every attempt ID/result and the final test-binary hash are retained in `stress-before.json` / `stress-after.json`. This is finite race/reliability evidence, not a throughput benchmark or proof against every possible process behavior.

Fresh native `per_item_command` capture passed **16/16**, with framebuffer inspected. It qualifies ordinary two-item execution, correct retained working directory, complete stdout/report copies, regex search, line navigation and scaled output under the new process owner. Independent assertions require two successes, exit 0, both expected CWD/name outputs, exact stdout/report/context agreement, three recording clipboard writes, no other native effect-port calls and matching launch/core hashes. Native Stop-button delivery itself was not added to that scene; cancellation is exercised by real host-process tests and the 256-run repeat. Source hashes and assertion script are retained.

This corrects ordinary Unix process-group cancellation, including an exited leader with inherited output holders. Descendants that intentionally detach into another session/group, privilege changes that prevent signaling, every non-Unix platform and arbitrary daemon containment remain outside the guarantee. No stronger process-tree sandbox or zero-latency cancellation is claimed.

Follow-up: the separate [native Run/Stop scene](../command-stop-native-2026-10-31/README.md) now qualifies button delivery and cancelled result/report state for an ordinary two-file run. The original normal-run capture above remains unchanged.
