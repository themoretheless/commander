# Archive unified filter reset — 2026-10-30

Independent adaptation of [ranger filter_stack clear](https://github.com/ranger/ranger/blob/master/ranger/config/commands.py), cohort #30. Upstream clears its filter stack and refilters. Commander independently applies a single reset to its archive filtering controls, extending the reset to path query, regex/case flags, type flags and selection narrowing rather than claiming exact ranger stack parity.

Reset filters clears the path query; disables Regex, Match case, Smart case and Exclude matches; enables all member types; and removes narrowing. It preserves current folder/mode, sort order, archive-wide selection, saved selection, selection totals cache, navigation history and listing. The UI requests query-field focus after the click. It is disabled during initial listing and when query/options/types/exclusion/narrowing are already at defaults. Even whitespace-only text enables reset. Reset does not read/reload/extract members or touch clipboard/filesystem.

Invalidates query and visibility caches explicitly, so removal of type/narrowing-only restrictions rebuilds even when text and pattern options were already default. Errors clear when the default pattern is compiled during the same UI pass. Existing cursor/anchor visibility checks retain their ownership; reset does not invent a new cursor or navigate to another folder. Repeating reset is idempotent.

Two regressions cover simultaneous query/error/case/type/exclusion/narrowing reset; preservation of folder/sort/history/saved and live selection; actual visible-row rebuild; default/whitespace state; and cache invalidation for narrowing/empty-query exclusion without pattern-option changes.

Native button, focus transfer and layout verification remain pending because desktop CUA is unavailable. Earlier archive native gates remain open. No native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2766 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied and locked default check passed (`clippy.log`, `check.log`).
- Formatting and diff whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked visual-qa build passed (`build.log`). Source/binary hashes and pending native-validation record accompany this report.
