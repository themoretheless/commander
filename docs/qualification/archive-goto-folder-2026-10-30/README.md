# Archive typed folder navigation — 2026-10-30

Independent adaptation of [Yazi interactive cd](https://yazi-rs.github.io/docs/configuration/keymap/#cd), cohort #27. Upstream changes directory from an interactive URL input. Commander independently applies direct typed navigation to its archive-only virtual folder index, not the host filesystem or general Yazi VFS.

Expand Go to folder, enter a root-relative internal folder path and use Go or Enter. Empty, / and . resolve to archive root. An initial / denotes this virtual root; repeated slashes and dot components normalize through Path components. Parent components are rejected even when they would remain inside root. No tilde/environment expansion, fuzzy lookup, globbing, link following, file opening or filesystem stat occurs. Lookup is exact and case-sensitive, preserves literal spaces/Unicode, and accepts only directories recorded in the loaded bounded index, including synthesized parents and explicit empty directory entries. Real file leaf paths are rejected. Index truncation/unavailability does not cause host fallback.

The pure resolver caps input at 4096 Unicode characters, rejects NUL, normalizes a PathBuf and binary-searches the tree's sorted directory vector. Root remains valid for an empty index. UI text has the same character cap. Errors remain beside the input and clear on edit/success; failure leaves current view/history unchanged. Successful navigation uses the existing deferred navigate action, switches to folder mode, records Back/Forward, clears the range anchor/cursor and preserves selection, query/type/exclusion/narrowing/sort. Active filters can hide destination rows. The draft text is not a live breadcrumb and is retained for retry; it resets with the listing. The panel is absent when folder indexing failed.

Two resolver regressions cover indexed/synthetic/empty directories, exact Unicode/spaces, virtual root, dot/slash normalization, case and file rejection, host-looking/missing paths, parent traversal, NUL and input cap. Existing navigation-history regressions cover the shared action owner. No autocomplete, relative-to-current-folder, host directory access or full Yazi URL parity is claimed.

Native Go/Enter, focus/error and layout validation remain pending because desktop CUA is unavailable. Earlier archive native gates remain open; no native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2770 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied passed (`clippy.log`).
- Formatting and diff whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked default check and visual-qa build passed (`check.log`, `build.log`). Source/binary hashes and pending native-validation record accompany this report.
