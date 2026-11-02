# Custom semantic themes

Inspired by [Kiorg custom color themes](https://github.com/houqp/kiorg/blob/master/README.md), implemented independently. This is Commander semantic-palette JSON, not Kiorg TOML or Strata palette-file compatibility.

View and settings -> Custom theme / JSON opens a dedicated modal with scrolling and Escape/Close. Use current colors as draft seeds all 15 semantic colors; color controls and JSON edit only the draft. Apply validates commander.theme version 1, all color roles and #RRGGBB syntax before switching the live palette. Unknown, missing and duplicate fields, invalid colors, incompatible versions or text over 16 KiB are refused without replacing applied data. Copy applied JSON exports the applied set, not the draft. Reset disables the custom palette while retaining the draft. Choosing Light, Dark, System or a bundled palette also disables it.

Applied data persists through the existing atomic session store. Invalid saved optional data remains available for correction but is not activated. Increase Contrast temporarily overrides custom colors using existing accessibility preferences; custom data is retained. User-selected colors are not automatically contrast-corrected.

Four new tests cover draft/applied isolation, rejected replacement, strict schema, color round trips, reset, invalid optional data and actual atomic session save/reopen. The native custom_theme scenario seeds an unapplied draft with panel #182838 and accent #389dcc, then clicks Apply through egui pointer events and checks the applied model and framebuffer. It does not test physical OS input or process restart. Source hashes and final checks are retained alongside this report.

Validation: all six gates passed (build, full tests: 3,261 passed / 3 ignored, strict Clippy, ordinary check, formatting and diff whitespace). Final custom_theme capture passed 16/16, the prior shortcut_exchange workflow 14/14. Exact draft/applied JSON, native effect counters and source/binary/tool hashes were independently checked; both screenshots were visually inspected. The theme scenario has zero native effects; the shortcut control records one clipboard write. Final capture records are native-launch.json and shortcut_exchange-launch.json. The initial theme attempt is preserved; its oversized JSON editor was corrected with a separate 150-point scroll area, and the scenario retries an uncommitted Apply within the unchanged deadline.

Native coverage here is Apply from a seeded draft. Pasted JSON, invalid Apply, Copy and Reset are now covered by the [theme-exchange follow-up](../theme-exchange-2026-11-02/README.md). Color-picker editing, the settings-menu entry, OS Increase Contrast transitions and process-level restart still need dedicated native runs. Persistence, parser/refusal, draft isolation and reset data behavior are covered by the tests.

No dependency changes. The previous dependency audit failures and broader 99-repository transfer goal remain open.
