# Rename name-list file import

Inspired by Double Commander [existing lists, section 5.1](https://doublecmd.github.io/doc/en/multirename.html). No upstream code copied.

The rename sheet accepts an absolute or batch-folder-relative file path. Load names from file submits a cancellable interactive Preview-lane task. Closing the sheet, changing the path, or Cancel loading drops the receiver and cancels its workload handle. Results never overwrite editor text automatically: Use loaded names is an explicit second action, enabled only when the loaded row count matches the current target order/count. Other transforms remain active. The existing preview, collision checks and Rename confirmation remain authoritative.

The reader accepts regular UTF-8 files up to 256 KiB; preserves BOM/CRLF and significant whitespace for the shared list parser; refuses malformed lists, non-UTF-8, symlinks and directories. Unix opens use NOFOLLOW/NONBLOCK/CLOEXEC. Descriptor and path identities are compared before/after reading to reject observable replacement/change. Cancellation is checked before open and each 8 KiB read. This does not provide an atomic filesystem snapshot or interrupt a stalled kernel filesystem call.

Regression evidence covers byte-exact source preservation, Unicode/BOM/CRLF, exact byte limit, oversize/malformed/nonregular/missing sources, cancellation and source replacement. Real egui pointer events drive Load through the workload and require a separate Use click; current count changes disable application. The resulting planner retains original source identity and applies the existing prefix transform to imported seeds. Native full-sheet capture remains separate scope.

All seven gates exited 0: dependency audit, visual-qa build, full tests (3357 passed, 0 failed, 3 ignored), strict visual-qa Clippy, ordinary check, fmt and diff. Ordinary check retains the existing diff_find.rs unused-response warning. verify.py rechecks terminal results, all four named import regressions and source hashes.
