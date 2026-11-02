# Case-sensitive Find queries — 2026-10-28

Independent adaptation of Double Commander's [Case sensitive search option](https://doublecmd.github.io/doc/en/findfiles.html), from the researched cohort. No upstream code copied.

Find accepts `case:sensitive` and `case:insensitive`; absent an option, existing insensitive behavior remains. The setting applies to names, relative paths, positive/negative content, glob masks, extension suffixes, exclusion names/ancestors and typed pane-filter expressions. It applies in Exact/Fuzzy/Regex modes; regex inline flags retain their standard meaning. Conflicting controls fail validation. Expressions, chips and saved JSON retain the setting. Extension parsing now preserves original case so `case:sensitive ext:RS` can distinguish `.RS` from `.rs`; insensitive compilation still normalizes suffixes.

Fuzzy matching keeps the existing scoring/range algorithm and streaming fallback, with optional character folding. Pane-filter literal/anchor/negation/AND/OR matching and highlights use the same explicit case policy. Ordinary pane filtering and palette matching retain their existing defaults. Exclusion regexes are compiled with the policy before live/index/archive traversal, so provider pruning agrees with result matching. Existing absence-completeness and read limits remain intact.

Coverage includes query serialization and conflicts, all matcher modes, masks/extensions/exclusions, saved pane-filter expressions, Unicode fuzzy matching, long streaming candidates and branch/AND highlights in original character coordinates. A live/indexed/ZIP matrix checks extension and mask case, positive/negative content, paths and exclusion pruning in all modes.

Retained rejected evidence: the first compile exposed a scorer test missing the new argument; an early fixture incorrectly expected a fuzzy negative not to match a subsequence spanning words; Clippy then found an unused default wrapper. The fixture was corrected without changing fuzzy semantics, and the default matching route retains that wrapper. No native UI interaction/capture; no commit or push.

Validation: **2493 passed, 0 failed, 3 ignored** in the full locked suite. After the equivalent default-wrapper routing correction, all eight focused case tests passed, including the new branch-highlight test. Final all-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed.
