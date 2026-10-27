# First occurrence and ignore-case batch rename — 2026-10-27

Independent adaptation of case and first-occurrence controls in
[Double Commander's Multi-Rename Tool](https://doublecmd.github.io/doc/en/multirename.html).
No upstream code or new dependency was added.

## Behavior

**Ignore case** and **First match only** now appear beside **Find is a regex**.
Both start unchecked, preserving the previous case-sensitive/all-occurrences
behavior. They work with Whole name, Name stem and Extension replacement scopes.
The first match is the leftmost match within that chosen component, independently
for every selected entry. Other configured case/prefix/suffix/numbering edits
still run in their existing order after find/replace.

Ignore case uses the existing Rust regex engine's Unicode simple case folding.
It supports the tested Latin/Cyrillic case pairs; it does not normalize Unicode,
perform locale-specific casing or expand multi-character folds such as Straße
versus STRASSE. Inline flags in a regex retain their standard engine semantics.
Filesystem collision/name-folding policy is unchanged by this matching setting.

Literal case-sensitive replacement keeps the existing string operations, using
replacen for First match only. Ignore-case literal search compiles an escaped
matcher once per batch; NoExpand preserves all replacement text literally,
including $1 and ${name}. Regex replacement still expands captures and supports
first/all occurrence choice. Braced ${1} references disambiguate a capture from
trailing letters/underscores, consistently with the existing engine.

An empty Find remains a no-op, including with these options enabled. A nonempty
regex that matches an empty position follows the engine's Unicode-boundary
semantics; First match only limits it to one replacement. No new wildcard,
multiple-pair replacement, locale selector or recursive rename is introduced.

## Planning and persistence

Both flags participate in RenameRule equality and the cached live-plan key.
Changing either option rebuilds validation before submission and participates
in the dialog's Enter guard. Compiler errors are surfaced through the existing
planning/application error gate before filesystem mutation. The normal captured
context, collision checks, rename ordering, rollback and Undo/Redo are retained.

Save/update/load presets preserve both options alongside the component scope.
Missing fields deserialize as false, and false defaults are omitted from JSON
to retain the old default serialized shape. Invalid non-boolean fields fail
preset deserialization. Loading or clicking an option only changes the editor
and preview; Rename remains the explicit applying action.

## Verification

- Corrected rename-filter tests: **126 passed**.
- Full current regression: **2326 passed, 3 ignored**.
- Clippy with all targets/visual-qa and -D warnings: passed.
- Formatting and scoped Git whitespace checks: passed.

Tests cover the option matrix, Unicode simple folding, name/extension scopes,
zero-width regex versus empty Find, literal dollar preservation, expanded regex
captures, legacy defaults, preset round-trip/type validation, and invalidation of
the cached plan. A workspace integration test renames real temporary files,
checks unchanged content and first-only output, and exercises Undo/Redo.

The initial run had one wrong fixture expectation for the unbraced $1_$1 capture
syntax: the engine consumes the underscore as part of the first reference name.
The expectation was corrected to the documented engine behavior; an explicit
${1}_${1} case verifies the intended separator form. The original failed log is
retained under rejected/. No production replacement semantics were changed just
to satisfy that fixture.

The native rename_options scenario opens TXT-TXT.txt with Find txt / Replace doc.
Pointer input enables Ignore case and requires the actual cached preview
`doc-doc.doc`, then enables First match only
and requires `doc-TXT.txt`. The source must still exist and that destination
must remain absent, confirming that option edits did not apply a rename. The
macOS eframe/Glow screenshot was visually inspected and all manifest checks
passed. Rename/Cancel remain visible at the captured size. Clipboard, opener
and trash ports were not called. The CI/README matrix now includes 20 scenarios;
no remote CI run, push or performance improvement is claimed.
