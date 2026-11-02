# Negative name/path text conditions — 2026-10-28

Independent adaptation of Double Commander's [negative text comparisons and search exclusions](https://doublecmd.github.io/doc/en/findfiles.html), from the researched cohort. Commander syntax is `notname:` / `notpath:`, not a claim of grammar parity. No upstream code copied.

Find accepts `notname:draft` and `notpath:vendor/`, including quoted values and conjunctions with name/path/content/metadata conditions. They reject matches using the same Exact/Fuzzy/Regex matcher and case policy as their positive counterparts. Multiple negative conditions are ANDed. Names use the full entry name; paths are relative to the search root, as for existing path terms, so the root directory's own name is not searched in every result. Virtual archive-member paths follow the existing positive path semantics. Expressions, chips and saved JSON retain the conditions; invalid regexes fail query validation.

These are result filters, not directory-pruning rules: traversal/archive inspection continues subject to existing budgets. Use exclude: for basename pruning and archives:off to disable member inspection. Native/indexed file content evaluation occurs after metadata rejection; archive providers can preload bounded member text before filtering. Negative metadata text does not require opening content, and successful absence explanations have no invented match highlights.

Regression coverage includes parsed query serialization in all modes, case sensitivity, actual literal/fuzzy/anchored-regex distinctions and invalid/empty parser values. A live/indexed/ZIP matrix uses a root itself named vendor, nested vendor paths, member paths, differing case and a binary .rs file. Result sets agree across providers and modes. Skip counts confirm that a native/indexed binary candidate rejected by metadata is not inspected as content.

No native UI interaction or new capture was performed for this query-language change. No commit or push.

Validation: **2516 passed, 0 failed, 3 ignored** in the full locked visual-qa suite; both focused regressions passed. All-target visual-qa Clippy with warnings denied, formatting and scoped whitespace checks passed. No rejected run for this feature.
