# Archive fuzzy path/name filter — 2026-10-31

Independent adaptation of fzf's subsequence matching, cohort #75:
https://github.com/junegunn/fzf#search-syntax

Fuzzy is an optional exclusive archive path/name mode alongside Regex, Exact and Glob. It uses Commander's existing allocation-free subsequence predicate over original Unicode characters with its one-scalar case folding and existing Match case/Smart case policy. Query cap remains 256 characters. Outer spaces are trimmed; inner spaces are literal. This does not implement fzf extended-search grammar, scoring or relevance ordering. Existing archive sorting remains authoritative.

The shared Pattern owner applies to real records and synthetic navigation folders, with filename/full-path scope, Exclude matches and independent size/type/exclusion predicates. Options equality includes Fuzzy in cache invalidation, saved recipe identity/description/recall and Reset. Defaults remain literal substring. Conflicting matching flags fail closed, even on empty queries. No payload reads, extraction or worker tasks are added.

Matcher regression covers noncontiguous vs substring behavior, scope, Cyrillic/dotted-I case behavior, Smart/explicit case, spaces, cap and conflicts. Pipeline regression covers same-query rebuild/cursor clearing, duplicate record IDs, size/type/glob intersections, saved mode/label/reset/recall, hidden selected exports and synthetic-folder inversion. Rendered checkbox regression exercises exclusive mode toggles while preserving case/scope flags.

Dedicated native ArchiveFuzzy opens the existing seven-member TAR fixture in src with size sorting and a hidden selected docs/notes.txt. It enters frs, requires no literal file matches, then clicks Fuzzy. Capture requires exactly file2.rs/file10.rs visible among real members, retained src folder/hidden selection, no operation/modal and zero native effects. Native saved/Reset/case/scope/invalid variants, OS input, VoiceOver and relevance ranking remain open.

Fresh native phase-7 capture passed 14/14 checks, all effects zero. Required literal-empty checkpoint precedes Fuzzy toggle. Inspected final frame shows frs/Fuzzy, only file2.rs and file10.rs, preserved Size/reverse/natural ordering and Copy selected (1). Native state requires selected hidden docs row 0 and src location. Native-launch binary hash verified. CUA returned running-app-not-found after successful short-lived capture; fresh artifacts establish completion.

Final validation: locked visual-qa build passed; full suite 2878 passed, 3 ignored, 0 failed; all-target Clippy with warnings denied, default-feature cargo check, rustfmt and git diff whitespace checks passed. Exact commands and exit codes are recorded in checks.json.
