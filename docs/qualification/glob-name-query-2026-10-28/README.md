# Whole-name glob queries — 2026-10-28

Independent adaptation of [fd whole-name glob search](https://github.com/sharkdp/fd#searching-for-a-particular-file-name), from the researched 99-repository cohort. No upstream code copied.

Find accepts `glob:report-*.pdf` and `glob:"report ?.pdf"`. Masks match the complete name, including directory names, and combine with other conditions through AND. `*` matches any run, `?` one Unicode scalar, other characters are literal. Mask syntax follows Commander selection masks: no bracket classes, brace alternatives or shell escape syntax. Matching is case-insensitive and independent of the query's Exact/Fuzzy/Regex mode. JSON and expression serialization preserve the predicate; the Find tooltip includes examples.

The search compiler creates an anchored, escaped regex before candidate evaluation, using the original name instead of lowercase-expanded text. This preserves `?` for characters such as `İ` and handles newline-containing names. A compiled-size limit returns a query error for oversized masks. Result explanations identify the glob and highlight the whole matching name in original character coordinates. There is no new dependency or per-candidate pattern compilation.

Validation includes expression/JSON round trips in all text modes, whole-name boundaries, Unicode, newlines, literal bracket behavior, oversized masks and a recursive filesystem/content search fixture checking returned hits and highlight coordinates. No native UI interaction or capture was run; no commit or push.

Full suite after the Unicode compiler change and oversized-mask regression: **2475 passed, 0 failed, 3 ignored**. All-target Clippy with visual-qa and warnings denied passed after removing redundant visibility qualification in the private module (rejected log retained). Full test execution preceded only that visibility spelling correction; final Clippy compiled all targets with the correction. Formatting and scoped whitespace checks passed. Initial pre-Unicode runs are retained separately.
