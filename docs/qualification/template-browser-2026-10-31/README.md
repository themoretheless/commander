# Saved-template browser — 2026-10-31

Independently adapts the searchable/filterable [Strata action recipe library](https://github.com/lgse/strata/blob/main/docs/custom-actions.md) pattern to Commander's saved shell templates.

**Find templates** in Run command searches names, raw command text and extension filters with Unicode lowercase matching. Every whitespace-separated term must appear in at least one field; terms never match across field boundaries. Results retain saved order and full-store indices. **All templates** includes entries for other file types, permitting their discovery and management; default applicability still uses the immutable opening selection. Selecting any row only fills the editable command/name/extensions draft. Clear restores the current scope. Search over 128 characters produces no matches with an explanation. Search/scope reset on closing the bar.

The projection is cached against full template contents, opening selection names, query and scope. Same-length metadata edits, moves, imports, removals and changed names invalidate it; stable frames do not rebuild lowercase fields. Move/Delete reuse original store indices and visible neighbours rather than filtered positions. No new filesystem probing, persistence or execution is performed by search.

Search input/clear/scope interaction claims keyboard activity, including simultaneous text+Enter, so the run bar's implicit Enter cannot fall through from these controls. Existing explicit Run, quoting, selection binding, trust gate and command execution remain the owners.

## Validation

**2966 tests passed, 3 ignored**, zero failed. Build, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks all passed; see `checks.json` and logs. Model regressions cover mixed-field AND matching, Unicode case, applicability vs all, unchanged order/original indices, query bounds and cache invalidation with unchanged row counts. A headless egui test clicks/fills the actual input with simultaneous text+Enter, clears it and toggles scope. Existing menu regressions cover full-store move/delete semantics.

Native `template_browser`: **16/16** capture checks, framebuffer inspected. Real pointer/key events start from applicable indices [1,2], search `АРХИВ` with simultaneous Enter (zero applicable results), enable All (index [0]), click that original row, then Clear to restore [0,1,2]. The exact resulting draft/name/extensions, unchanged three-template store, applicable count 2, absent run and absent output are recorded in `browser-state.json` and independently asserted in `native-validation.json`. All native effect counters are zero. Binary/core/launch SHA-256 match; source hashes are recorded.

The native fixture injects the run-bar opening context and template list without reading/writing host templates or changing trust configuration. It qualifies browser interactions, not the production opening trust decision, command execution, new native Move/Delete actions, or the previously added import/export section. These boundaries remain separate. `headless-pass` retains the earlier green source before adding the native scene. The scene is included in the CI matrix (60 scenes total); this run captures this scene only.
