# Positional tab expansion in text comparison

Independent adaptation of [Midnight Commander's Ignore tab expansion option](https://github.com/MidnightCommander/mc/blob/master/po/pl.po). No upstream code copied.

**Expand tabs for comparison** expands tabs to the next fixed eight-character stop in comparison keys. Other spacing remains significant: `x<TAB>z` matches `x` plus seven spaces plus `z`, but not one space or no space. Expansion uses original Unicode scalar positions before lowercase mapping; it does not claim terminal display-cell or grapheme-width alignment. Each source line starts at column zero. Original loaded text and displayed rows remain intact.

Ignore space changes / Ignore all spaces override the tab policy and disable its checkbox while preserving its value. Case and CRLF policies compose independently. Immediate recomparison uses retained sources, and Reload/Swap retain the tab setting. Files are never rewritten.

## Validation

- Three new tests cover 24 starting columns, consecutive/multiple tabs, Unicode, incorrect spacing, CRLF/EOF distinctions, lowercase expansion, policy precedence, bounded expansion and retained-source/Reload/Swap behavior.
- Full suite **3105 passed / 3 ignored**. All six commands in `checks.json` exit zero.
- Native `diff_options`: seventeen real three-event interactions (**51 phases**), starting with tab on/off/on before other policies. Difference counts change 5 → 4 → 5 → 4; subsequent combined policies leave only the substantive old/new pair. Disabled tab preference survives overriding spacing policies, Reload and Swap.
- Existing native `diff_swap` runs on the same executable. Options passes twice on the same executable; Swap passes once. Each capture is **14/14**, recorded native-port effects zero.
- `verify.py` checks final executable hashes, command results, every phase's policy/counts, raw retained tabs/CRLF, original display text and source line 5. `diff_options/frame.png` is the inspected final frame.

Existing source-loading/LCS limits remain. Expansion is bounded by eight output characters per input tab. Custom tab widths, editor display-tab configuration and complete MC/GNU diff compatibility are outside this change.

Two early runs stalled after repeated pointer input; launch records and earlier checks are retained under `before-input-cadence/` and `before-input-diagnostics/`. The final scenario spaces events by 80 ms, records checkbox input delivery and retains a diagnostic on any state mismatch. Both final runs completed all snapshots without a waiting diagnostic. This establishes the observed final passes, not universal native-input timing reliability.
