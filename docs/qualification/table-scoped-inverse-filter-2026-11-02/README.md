# Scoped and inverse table row filters

Independent adaptation of fzf field-scoped and inverse search concepts, from cohort entry 75: https://github.com/junegunn/fzf#search-syntax and its --nth field restriction. Commander does not implement fzf query syntax or use fzf at runtime.

CSV/TSV preview now offers Search in: All columns or a numbered column, with the heading on hover, and Exclude matches. A match remains inside one cell, using the existing literal/fuzzy and Match case/Smart case rules. Exclusion negates the complete row predicate, after column selection. Missing cells do not match and are retained in exclusion mode. Empty/whitespace query shows all data rows; overlong queries or invalid column indices fail closed. Duplicate/empty headings cannot change numeric column ownership.

Heading retention, natural/source ordering, row identity, cached order invalidation, pane isolation and source-allocation reset remain shared. Copy visible CSV/TSV uses exactly the resulting records, preserving complete cells and all columns. Preview bounds remain 256 KiB, 500 rows and 64 columns; no file mutation, shell invocation or new dependencies.

Regression coverage includes ragged rows, case/fuzzy matching, all-column versus chosen-column negation, empty/invalid queries, exact exported records, and actual egui pointer interactions with the scope menu and exclusion checkbox. Native application capture/layout and OS pasteboard for these new controls are not yet qualified.

Final validation: all six gates passed (QA build, full suite, strict all-target Clippy, normal check, format and diff). 3296 tests passed / 3 ignored. Source and executable hashes were rechecked after the gates. No native capture or OS pasteboard claim is made for the new controls.
