# Archive filename-only match scope — 2026-10-30

Independent adaptation of [fd filename vs full-path matching](https://github.com/sharkdp/fd/blob/master/doc/fd.1) (`--full-path`), cohort #77 in the 99-project research. Commander retains its existing full internal-path default and exposes Filename only as an explicit filter option.

Filename only projects the candidate and cached lowercase candidate to their final slash-delimited component before literal or regex matching. Regex anchors therefore address the name: ^main\.rs$ matches src/main.rs. Case, Smart case, inline regex flags, bounded compilation and invalid-pattern failure remain shared. Trailing separators on directory names are ignored when finding the final component. The projection borrows slices and allocates no strings per member match.

The same compiled owner handles real members and synthetic navigation folders, and Exclude matches complements that scoped pattern. Size/type/folder/narrowing predicates still intersect independently. Displayed/exported paths remain full member paths. Changing scope invalidates the existing pattern/visible cache via Options identity; duplicate member IDs stay separate, hidden selections remain selected and invalid regexes still hide all rows.

Saved filters include scope in full-key identity and show Filename only / Full path in their labels. Recall restores it; Reset returns to full-path matching. Existing saved-filter removal therefore also distinguishes identical queries with different scopes. No archive payload read, extraction, host-path lookup or worker task is added.

Two regressions exercise literal/anchored regex matching, case/inline flags, Unicode, directory components, duplicate identities, visible-cache rebuild, synthetic navigation, exclusion, hidden selection, saved settings and Reset. Native checkbox/input/recall/focus/layout remains pending: CUA is not exposed in this continuation and no native launch was attempted. Existing captures are not reused to qualify the changed controls and saved-filter labels.

Validation: the locked visual-QA suite passed 2815 tests (3 ignored). Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
