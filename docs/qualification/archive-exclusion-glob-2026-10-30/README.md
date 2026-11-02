# Archive independent exclusion glob — 2026-10-30

Independent adaptation of [fd --exclude glob constraints](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Exclude glob is separate from the existing main query and its Exclude matches switch. It always performs case-insensitive glob matching, defaulting to each filename/ancestor directory name; Full paths compares archive-relative path prefixes. A match on any ancestor excludes the descendant. Thus main query report can retain report names while {*.bak,*.tmp} excludes backups, and build excludes that directory and its contents.

The existing bounded glob compiler supplies wildcard/classes/alternatives/escaping and 256-character limits. The ? wildcard counts UTF-8 comparison bytes as in the existing Glob mode. A single glob can contain brace alternatives; repeated fd options and ignore-file import are outside this transfer. Empty input disables the exclusion; meaningful spaces are retained. Invalid input fails closed for real and navigation rows and cannot be saved. Prefix matching borrows the cached lowercase path without allocating per ancestor and performs no member I/O.

An independent settings/cache owner recompiles only when query or Full paths changes. It participates in visible-row invalidation, all existing predicate intersections, synthetic navigation filtering, saved identity/labels/menu search, 16 KiB saved text bounds, recall and Reset. Hidden selected IDs remain selected and selected-only copy retains them. Scope and query are stored together. The main query mode/case/exclusion settings stay independent.

Two regressions exercise combined include/exclude, brace alternatives and case, ancestor/recursive exclusion, filename/full-path scope, duplicate identities, navigation removal, selected hidden copy, invalid/overlong input, saved scope identity, recall/Reset and actual pointer entry without changing the main query.

ArchiveBrowser is extended to type *2.rs into the actual exclusion field after its hidden-entry toggle. Capture readiness requires that exact applied query and file2.rs omitted, together with the prior hidden member/navigation/selection, sorting and folder invariants. Native and full-check results follow below. No commit or push performed.


Fresh final native ArchiveBrowser passed 14/14 capture checks with all recorded effects zero. The framebuffer was inspected: the focused exclusion field contains *2.rs, Copy visible is 2, Copy selected remains 2, Hide dotfiles remains enabled and the sheet stays bounded/scrollable. This qualifies native rendering with injected pointer/text events. Physical keyboard delivery, VoiceOver, native Full paths/saved recall/Reset and broader invalid-query/ancestor/format cases remain open.

The rejected initial native run entered and applied the exclusion but retained an old final assertion requiring file2.rs for natural-order comparison. Its manifest/log/hash are retained. The corrected scene requires that exact natural order before entering the exclusion (phase 3), then requires file2.rs absent at final phase 7. No ordering requirement was dropped from its applicable phase. CUA's post-exit application-not-found response is consistent with fresh successful capture artifacts.

Final verification after the scene correction: locked visual-QA suite passed 2847 tests (3 ignored); Clippy with warnings denied, default locked check, formatting and diff whitespace checks passed. The final visual-QA build passed (build-final.log) before the fresh native capture, and its current SHA-256 matches launch metadata.
