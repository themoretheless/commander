# Precise indexed modification times — 2026-10-28

Qualification follow-up to the fd-inspired [absolute modification filters](../absolute-modified-dates-2026-10-28/README.md). This closes an observed provider mismatch, rather than adding another query spelling. No upstream code copied.

A real filesystem regression first proved that a file modified at 2026-01-01T00:00:00.5Z matched a fractional after/before window in live search but disappeared from indexed search: cached modified_secs rounded down. Source inspection also showed that dates before 1970 were dropped by unsigned epoch conversion. The failing run is retained as reproduced.log.

New index documents carry an optional exact timestamp: epoch direction, unsigned duration seconds and subsecond nanoseconds. Reconstruction uses checked arithmetic and rejects invalid nanoseconds. An explicitly invalid exact value produces unknown time rather than silently falling back to a different legacy value. Existing modified_secs remain for older readers; older snapshots without the exact field remain usable with their original second precision. Rebuild an old snapshot to obtain exact metadata. Access-time storage is unchanged.

The document JSON size allowance was increased to cover the additional metadata; the existing maximum-metadata and serialized-budget regressions qualify that bound. The schema remains compatible with existing documents. No live stat is introduced while reading indexed results, and the index still represents its cached observation.

Regressions verify actual filesystem times on both sides of the epoch, fractional-window parity between live/indexed searches in every text mode, exact timestamp JSON round trips, legacy fallback and rejection of corrupt/overflowing exact values. No native UI capture for this data-format change. No commit or push.

Validation: **2522 passed, 0 failed, 3 ignored** in `cargo test --locked --features visual-qa`. Both focused regressions passed before persistence coverage was added; the full suite then passed that expanded coverage as well as the existing maximum-metadata and serialized-budget tests. All-target visual-qa Clippy with warnings denied, formatting and scoped whitespace checks passed. The intentional pre-fix failing reproduction is retained.
