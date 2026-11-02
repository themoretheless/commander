# Archive stem and internal-parent clipboard formats — 2026-10-30

Independent adaptation of [Yazi copy name_without_ext / dirpath](https://yazi-rs.github.io/docs/configuration/keymap/#copy), cohort #27 in the 99-project research. Native pane and Find already offer related formats; the missing archive-browser variants now use its existing visible/selected member-row owner.

## Implemented

Names without extension removes only the last suffix from the final component: archive.tar.gz becomes archive.tar, .env stays .env, .env.local becomes .env, and name. becomes name. The transformation is lexical for all member kinds, including directories and symbolic links; it never follows links or consults the filesystem. Duplicate member identities and caller order remain intact.

Parent folders copies distinct internal archive parents in first-occurrence order. A member at the archive root contributes `.`. These are internal member folders, not host directories containing the archive; no host path is invented. Deduplication happens before escaping and does not skip validation of later rows. The Copy button count and toast identify the source member count, which can exceed the number of unique folders. The toast now says “listing for N archive members” to express that distinction.

Both modes share existing strict UTF-8 validation, control escaping, complete-result failure and 4 MiB field/output limits. Copy selected retains hidden archive-wide choices in the current sort order. Synthetic navigation rows are omitted by the existing row owner. Copying reads listing metadata only, without decoding/extracting member contents. The default remains Member paths; Names, Member paths and CSV remain available through the same wrapped controls.

## Evidence and remaining gate

Two formatter regressions cover stems, directory names, duplicate identities, Unicode/control text, ordered parent deduplication, explicit root, late invalid IDs and size rejection. Existing non-UTF-8 tests now iterate all five modes. A third regression passes actual MemberFilter visible/selected rows through both new formats, covering hidden selected members, sort order and synthetic navigation exclusion.

Native format selection, wrapped layout, routed recording clipboard and OS pasteboard are unqualified for these new modes. No CUA tool is exposed in this continuation; no native launch was attempted. Earlier ArchiveExport captures cover the earlier formats/binary and are not reused as evidence for this change.

Validation: the locked visual-QA test suite passed 2802 tests (3 ignored). Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
