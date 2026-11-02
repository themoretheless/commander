# Archive folder completion — 2026-10-30

Independent adaptation of [Yazi cd URL completion](https://yazi-rs.github.io/docs/configuration/keymap/#cmp), cohort #27. Upstream provides a completion component for cd input; Commander independently adds archive-index-only Suggestions, not full URL completion or keyboard parity.

Go to folder now offers a Suggestions menu. It shows up to 12 exact case-sensitive prefix matches in existing directory-index order. Choosing a suggestion fills the draft, clears its error and dispatches the existing history-aware archive navigation. If a 13th match exists, the menu shows an explicit refine-prefix hint. Empty input lists the first indexed folders; root itself remains available via Go. A trailing slash requests descendants rather than matching a sibling prefix. Leading virtual-root slashes, repeated separators and dot components normalize through the same pure path validator as Go. Parent components, NUL and over-limit inputs produce no suggestions. Literal spaces and Unicode are preserved.

Only indexed directories with UTF-8 paths and at most 4096 characters are suggested, so every displayed result can be entered by the typed-path API. Non-UTF-8 or longer directory names remain accessible through existing navigation but are not offered as unenterable text suggestions. File leaf paths are not completion targets. No host filesystem access, tilde/environment expansion, fuzzy matching or payload reads occur.

The index scan occurs when the menu opens with a changed draft, and caches at most 12 paths plus overflow state per immutable listing. It does not rescan on unchanged repaints. Matching stops after 13 admitted results, although no-match prefixes can scan the bounded full index (at most 100,000 folders). Draft cache and suggestions reset with the listing. Menu labels truncate to bounded width with full-path hover text. Selection, current filters and extraction remain owned by existing navigation/action code.

Two regressions cover normalized and descendant prefixes, exact case/spaces/Unicode, directory-only and resolvable results, parent/NUL/missing rejection, deterministic first-12 cap/overflow, single result and input/result length limits. Existing Go/history regressions continue to cover the shared navigation owner.

Native menu selection, overflow affordance and layout remain pending because desktop CUA is unavailable. Earlier archive native gates remain open; no native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2772 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied passed (`clippy.log`).
- Formatting and diff whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked default check and visual-qa build passed (`check.log`, `build.log`). Source/binary hashes and pending native-validation record accompany this report.
