# PDF document Smart case — 2026-10-30

Independent Rust adaptation of [lf smartcase](https://github.com/gokcehan/lf/blob/master/doc.md#smartcase-bool-default-true), cohort #32, extending Commander's existing per-page preview setting to whole-document PDF search. No upstream code copied. The live primary source describes uppercase query characters overriding ignorecase; explicit case matching remains authoritative.

Search entire PDF now has an optional Smart case checkbox, default off. Lowercase queries ignore case; any Unicode uppercase query character enables case matching. Explicit Match case wins. Search snapshots the original manual and smart settings separately, so opening a result transfers Smart case to the page toolbar rather than freezing its effective case value. Editing the page query then recalculates the policy. Global option changes discard the old result/error; controls remain disabled while the job owns its snapshot. Existing page/job/text bounds, cancellation, source checks and regex/word behavior remain unchanged.

Both owners call the same case-policy helper. As in Commander's existing preview search, regex syntax participates in uppercase detection (for example `\B`); inline regex flags still apply. This differs from [ripgrep's regex-literal analysis](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/flags/defs.rs). No exact ripgrep regex smart-case parity is claimed. Settings remain per-PDF session and reset with the existing slot lifecycle.

Two new regressions cover literal/regex Unicode behavior, manual priority, inline flags, default-off behavior, result snapshots and changing the transferred page query from lower to upper case and back. Existing preview smart-case cancellation tests remain applicable.

The native PdfRegex scenario now clicks Regex, Whole words and Smart case, enters lowercase `second\s+page`, requires sole hit page 2, opens it and requires both global flags and page Smart case, ready regex indexing and one highlight. The first native run reached the correct search state but failed pane containment (bottom 767.5 versus viewport 760). Its artifacts are retained in `pdf_regex-initial`. Wrapped controls exposed ScrollArea's default minimum height; page text now uses the actual remaining height with a zero minimum.

Review also found that the production query constructor depended on a test/visual-qa-only constructor. It now initializes the bounded query directly; a locked default-feature check passed in addition to the visual-qa gates.

Final verification: locked visual-qa suite **2708 passed, 0 failed, 3 ignored**; all-target visual-qa Clippy with warnings denied, locked visual-qa build, default-feature locked check, fmt and diff checks passed. Fresh CUA launches of extended PdfRegex and ordinary PdfSearch each passed **14/14**, all native effects zero. Both manifests postdate their launch records and match the final binary hash. Both frames inspected: global/page Smart case enabled in the regex case, disabled in the ordinary literal case; one highlighted result on page 2 and panes contained in the viewport. Captures and launch records live in `pdf_regex` and `pdf_search`; source hashes accompany this report.

This milestone does not complete the 99-repository transfer scope. No commit or push.
