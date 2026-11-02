# Archive excluding path filter — 2026-10-30

Independent adaptation of [ranger filter-stack NotFilter](https://github.com/ranger/ranger/blob/master/ranger/core/filter_stack.py), cohort #30. Verified upstream negates its subfilter; Commander independently adds a path-predicate exclusion flag, without claiming the full ranger filter stack.

Exclude matches is default off. When enabled, literal and regex path matches are hidden, for both real archive members and synthetic navigation folders. Existing case/Smart case/inline-flag policies remain unchanged. This negates only the valid path predicate: type, folder and narrowed-snapshot restrictions continue to intersect normally. Invalid or over-limit patterns still produce an error and no rows, regardless of the flag. A blank pattern matches every path; excluding it intentionally hides all rows. Literal whitespace trimming and regex meaningful spaces retain existing semantics.

The flag participates in visibility-cache invalidation independently from regex compilation. Cursor and range anchor clear when hidden through the existing owner. Hidden selections and saved/narrowed snapshots remain unchanged. Archive-wide selected exports/extraction retain their scope and the hidden-selection count accounts for exclusion, including empty queries. Navigation/history do not reset the flag; replacing/closing the listing does. No member reads, extraction, clipboard writes or persistent settings are added.

Two regressions cover cache toggling, duplicate records, hidden cursor/selected export, literal/regex/Smart case, empty patterns, invalid syntax under both directions, default reset, synthetic-folder filtering, folder/type/narrowing intersections.

Native checkbox, filtered navigation and layout validation remain pending because desktop CUA is unavailable. Earlier archive native gates remain open; no native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2764 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied and default locked check passed (`clippy.log`, `check.log`).
- Formatting and diff whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked visual-qa build passed (`build.log`). Source/binary hashes and pending native-validation record accompany this report.
