# Archive declared file-size filtering — 2026-10-30

Independent adaptation of [fd size constraints](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77 in the 99-project research. The archive browser now has a separate File size field, composed with the existing path/type/folder/narrowing filters.

## Behavior

Use an exact size (`0b` or bare bytes), inclusive minimum (`+1mi`), inclusive maximum (`-10mi`), or two whitespace-separated constraints for a range (`+1mi -10mi`). All constraints are intersected; contradictory ranges are valid and show no real members. Units are case-insensitive: b or no suffix is bytes; k/m/g/t are decimal, ki/mi/gi/ti binary. Up to two constraints and 64 input bytes are accepted. Parsing and unit multiplication use checked u64 arithmetic throughout, with no floating point. Fractions, unknown units, missing numbers, overflow and excess constraints are rejected.

Active size constraints match declared unpacked regular-file sizes only. Real directories, links and other records are excluded; independently generated navigation folders remain available under valid constraints. A blank expression disables the size predicate. Invalid expressions fail closed for both real rows and synthetic navigation, with a visible error. These are listing metadata sizes, not decoded/verified payload lengths or disk usage.

The compiled predicate is cached separately from the path pattern, so size edits rebuild displayed rows without recompiling an unchanged regex. Hidden selections and their archive-wide copy/extraction owner remain intact. Existing sort, narrowing, cursor/anchor invalidation and bounded virtualized scrolling remain shared. No new worker or source read is started.

Reset filters clears size text, including invalid/whitespace input. Saved filters capture/restore/deduplicate the size expression with all other query settings; invalid sizes cannot be saved, size-only snapshots are supported, and the 16 KiB retained-text cap now includes query plus size text. Closing/reloading retains the existing per-listing reset behavior. The new field has a stable ID and accessibility label.

## Evidence and gate

Two regressions cover inclusive endpoints, decimal/binary/case-insensitive units, zero, values above 2^53 and u64::MAX, unit/numeric overflow, invalid syntax, contradictory constraints, invalid whitespace reset, links vs regular files, synthetic navigation, hidden selection, and saved/Reset restoration. Existing saved-filter and pointer regressions are included in the full run.

Native field entry, error text, saved recall, cursor reveal and layout remain pending. CUA is not exposed in this continuation; no native launch was attempted. Previous archive captures do not qualify the new field or changed saved-filter row labels.

Final-source validation: locked visual-QA suite passed 2811 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. The complete suite was repeated after the invalid-whitespace Reset regression was added. Source/binary hashes are recorded alongside the logs. No commit or push performed.
