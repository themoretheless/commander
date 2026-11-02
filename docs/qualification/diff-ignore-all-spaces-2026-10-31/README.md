# Ignore all spaces in text comparison

Independent adaptation of [Midnight Commander's “Ignore all whitespace” diff option](https://github.com/MidnightCommander/mc/blob/master/po/pl.po). No implementation copied. Commander's deliberately explicit policy removes ASCII spaces and tabs from comparison keys only; non-ASCII spacing remains significant.

**Ignore all spaces** makes `a + b` and `a+b` match, including leading/trailing spacing and missing whitespace. It takes precedence over Ignore space changes (whose retained setting is disabled while overridden) and composes with Ignore case. Original rows, source coordinates, line boundaries, CRLF/LF and final-newline distinctions remain intact. Turning it off recomputes from the retained sources; Swap and Reload preserve the option. No file is edited.

## Evidence

- Three new regression tests cover absent spacing, original text, case composition, precedence, rollback to exact keys, empty/blank lines, non-ASCII spaces, endings, meaningful changes, retained-source recomparison, Swap and Reload.
- Full suite: **3099 passed / 3 ignored**. Six required checks in `checks.json` must all exit zero.
- Extended native `diff_options` uses eleven actual three-event button interactions (33 phases): existing space/case combinations, all-space enable/disable/enable, Reload and Swap. Snapshots preserve options, exact displayed text and source coordinates. The fixture has four initial changed pairs; all selected policies leave only the substantive `old`/`new` pair.
- Existing native `diff_swap` runs as a separate regression on the same final executable.
- `verify.py` validates check results, binary hashes, phase transitions and native manifests. `diff_options/frame.png` is the inspected final frame.

This adapts the useful behavior rather than claiming complete GNU diff/MC whitespace semantics, locale handling, Unicode normalization or syntax-aware comparison. The existing bounded LCS and source-load limits are unchanged.
