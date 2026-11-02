# Favorites fuzzy hotlist — 2026-10-31

Independently adapts the [Double Commander Directory Hotlist](https://doublecmd.github.io/doc/en/help.html) navigation idea to the existing Rust omnibar.

`Open favorite folder` opens `@` mode. All entries in the immutable opening snapshot participate in fuzzy label/path ranking, labels first; only the best 100 are displayed with total counts. Empty queries retain source order. Queries over 128 characters return no results. Here, Other panel, New tab and Background bind to the opening panel and original path. Dispatch rechecks current bookmark membership and existing lifecycle/capacity guards. Search performs no filesystem probes; missing destinations use ordinary asynchronous navigation.

## Validation

- Full suite: **2953 passed, 3 ignored**, zero failed. Build, all-target Clippy with warnings denied, default-feature check, formatting and whitespace checks passed; see `checks.json` and logs.
- Focused regressions cover ranking beyond the display cap, Cyrillic matching, label priority, query caching, opening-panel binding, removed paths and deferred same-frame text/Enter identity.
- Native `favorite_picker`: **16/16** manifest checks, framebuffer inspected. With 152 bookmarks, `@пркт` selects original index 150. Real egui events open Background using simultaneous text/Enter, then Other panel by row click and New tab by Enter. Final capture retains the hotlist open in Here mode. Here dispatch itself is covered by the workspace regression.
- Independent assertions over `picker-state.json` verify original label/slot, index, query and exact left/right tab paths and active indices. `native-validation.json` binds results to identical build/core SHA-256. Native effect counters are all zero; fixtures and persistence are isolated from user configuration.

## Boundaries

This is a Favorites snapshot search, not live filesystem or provider discovery. Legacy mixed Jump results and their existing path checks are unchanged. The native scenario qualifies this interaction slice, not every scene or parity with all 99 repositories. No dedicated hotkey was assigned.
