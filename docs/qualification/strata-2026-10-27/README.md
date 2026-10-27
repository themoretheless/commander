# Strata transfer verification — 2026-10-27

Scope: icons layout/interaction, bounded static document rendering, bundled
palette selection/restoration, and keyboard-help modal. This is not full Strata
parity; see [ledger](../../strata-parity.md).

- `cargo check --all-targets --all-features`: passed.
- Initial full regression after view/document changes: 2184 passed, 3 ignored.
- Full regression after palettes/help: **2188 passed, 2 failed, 3 ignored**.
  Both failures were catalog expectations: F1 was absent from the test shortcut
  parser, and the new help entry displaced Copy at the head of the palette.
  Kept Copy first, registered F1 in the parser, then reran all 46 command tests:
  **46 passed**. The original failing transcript is retained alongside the rerun.
- App tests after header placement and shared UI changes: **371 passed**.
- Final file-list/grid tests after uniform icon-slot layout: **40 passed**.
- Theme tests: **13 passed**, including the 95 palettes and high-contrast fallback.
- Input tests after grid arrows/Shift selection changes: **17 passed**.
- `cargo clippy --all-targets --all-features -- -D warnings`: **passed on final code**.
- `cargo fmt --check` and `git diff --check`: passed.
- `cargo deny --all-features --locked check advisories bans licenses sources`:
  initially rejected existing yanked `clipboard-win 5.4.1`. Updated only that
  Windows transitive package to `5.4.3`, retaining its BSL license exception.
  Final result: **advisories ok, bans ok, licenses ok, sources ok**. Existing
  duplicate dependency groups remain warnings. No Windows runtime test was run.

## Native captures

Both runs used the real eframe/Glow renderer and temporary fixtures, with fake
native effects. Each manifest reports **14/14 checks passed** and zero native
effect calls. Screenshots were visually inspected.

- [Icons grid](icons_grid/frame.png): 1280×760 logical points, 2× backing scale,
  actual decoded PNG thumbnail; the harness waits for the thumbnail before capture.
- [Minimum window](minimum_window/frame.png): 900×500 logical points, light theme,
  sidebar visible; controls, lists and footer remain within the viewport.

Commands:

```sh
cargo run --features visual-qa -- --visual-qa icons_grid --output target/visual-qa-strata
cargo run --features visual-qa -- --visual-qa minimum_window --output target/visual-qa-strata
```

Captures establish layout/thumbnail behavior for these fixtures, not complete
VoiceOver, native context-menu, network-provider or media-player qualification.
The full suite was not repeated just to replace the retained failing transcript;
the affected command, app, grid, theme and input tests were rerun instead.
