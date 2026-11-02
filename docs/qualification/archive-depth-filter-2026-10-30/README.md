# Archive-root depth constraints — 2026-10-30

Independent adaptation of [fd min/max/exact depth](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77 in the 99-project research. The archive browser now has a separate Depth field alongside File size.

Depth accepts an exact unsigned level (2), minimum (2..), maximum (..3), or inclusive range (2..4). Nonempty member components are counted lexically from the archive root; root-level names have depth 1 and navigating inside the archive does not rebase the predicate. Constraints apply to real member records of every type, intersecting path/scope/mode/size/type/narrowing. Valid constraints retain synthetic navigation folders for browsing, even outside the displayed depth range. This is filtering of the loaded listing, not traversal pruning or a new archive walk.

Input is capped at 24 UTF-8 bytes with checked u32 parsing; surrounding/endpoint whitespace is accepted, signs/fractions/multiple separators/overflow/empty ranges and reversed bounds are rejected. Blank input disables the predicate. Invalid input fails closed for real and synthetic rows and shows a separate depth error. The Reset predicate includes raw input so invalid whitespace-only text can still be cleared.

The compiled depth range is cached independently from path and size predicates, and a blank range returns immediately without scanning path components. Saved snapshots include depth text in identity, labels and the combined 16 KiB retained-text cap. Depth-only saves are supported, recall restores the archive-root basis, Reset clears the field, and existing per-listing close/reload reset remains. Hidden selections, sorted IDs, extraction/copy membership and navigation history remain independently owned. No payload or worker I/O is added. The field has a stable ID and accessibility label.

Two model/pipeline regressions cover exact/min/max/range/zero/extreme levels, invalid and whitespace input, same-query saved identity, cache rebuilding, folder-basis stability, navigation folders, hidden selections and saved/Reset recovery. Native field entry/error/recall/focus/layout remain pending: CUA is not exposed in this continuation and no native launch was attempted. Earlier archive captures do not qualify the changed row and saved-filter labels.

The initial full suite passed, then Clippy rejected only field reassignment after Default in the new test fixture (`clippy-initial.log`). The fixture was rewritten as an initializer before the final check sequence; no production behavior changed for that correction.

Final-source validation: locked visual-QA suite passed 2823 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
