# File-only and exact-size search — 2026-10-28

Independent adaptation of fd file-type and exact-size controls documented in [fd CLI source](https://github.com/sharkdp/fd/blob/master/src/cli.rs), a repository in the researched cohort. No upstream code copied.

Find accepts `type:file` (alias `type:files`) and explicit exact size `size:=0` or `size:=1KB`. Combining `type:file size:=0` finds zero-byte files without directories, including indexed and ZIP member results. File-only uses Commander's existing non-directory classification across all extension categories; this does not introduce a separate regular-file/symlink/device taxonomy. Existing `type:folder` aliases are unchanged.

Exact size is represented by the existing inclusive min/max predicates, so saved JSON and expression round trips preserve equality. Existing bare `size:1KB` remains a minimum, and KB remains 1024 bytes; this is an adaptation rather than fd syntax parity. Integer sizes now use checked integer parsing/multiplication to retain byte precision above 2^53 and through u64::MAX, rejecting overflow. Existing decimal size parsing/rounding remains in place. The Find tooltip includes the empty-file example.

Regression coverage: all text modes, file category independence, synthetic same-size folders, adjacent size mismatch, query persistence, old minimum semantics, malformed equality expressions, exact large integers and overflow. A real recursive fixture checks identical live/indexed results for empty/nonempty files, a directory and an empty ZIP member. No native UI interaction or capture; no commit or push.

Validation: **2481 passed, 0 failed, 3 ignored** in the full locked suite. Both focused regressions passed. All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected test or Clippy run for this feature.
