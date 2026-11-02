# Unix executable-mode search — 2026-10-28

Independent adaptation of fd's [executable file filtering](https://github.com/sharkdp/fd/blob/master/src/cli.rs). No upstream code copied. Commander defines this as a portable cached mode-bit condition, not a claim of fd effective-user access-check parity.

Find accepts `type:executable` and `type:exec`. A match is a lexical regular file with at least one Unix execute bit (owner, group or other). Filename extension and file contents do not decide this condition. Directories, symbolic links and special files are excluded; a native hard link to an executable regular file remains a regular-file match. This does not guarantee current-user execute access, usable file format, or successful execution. Other predicates combine through AND and expressions/JSON preserve the condition in all text modes.

Live search uses already observed lexical file metadata. New index records retain an optional executable-mode flag, with no live stat while searching a snapshot. Old records without the flag remain readable but cannot establish a match and contribute to the existing skipped/partial-coverage count; rebuild to populate it. Non-Unix native modes are unknown. ZIP Unix modes and TAR regular-file modes are supported; archive symlinks, directories and special types cannot become executable matches merely because their mode carries execute bits. Missing mode data remains unknown. Content and archive-inspection policies otherwise remain unchanged.

The regression matrix uses real owner-only/group-only/other-only execute bits, a non-executable file, a binary executable, a directory, a symbolic link, a hard link and an executable-mode FIFO. Live and JSON-round-tripped indexed searches agree with ZIP/TAR results in all modes, with archive inspection enabled/disabled and content conjunctions. A contradictory symbolic-link/executable conjunction returns none. Binary content contributes one skipped candidate. Changing permissions after indexing preserves the cached observation; legacy records with removed metadata count as skipped. The query regression covers aliases, serialization and unknown synthetic metadata.

No native UI capture or new widget. No commit or push.

Validation: **2526 passed, 0 failed, 3 ignored** in `cargo test --locked --features visual-qa`; both focused regressions passed. All-target visual-qa Clippy with warnings denied, formatting and scoped whitespace checks passed. The maximum-metadata serialization bound passed with the new field. No rejected run for this feature.
