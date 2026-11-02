# Metadata/status change time

Independently adapts [eza changed-time display and sorting](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. No upstream implementation copied.

**Sort by metadata changed** toggles ascending/descending order of captured Unix ctime, with unknown-last ordering, existing folder/name policies and preserved cursor/selection. The active date column becomes Changed; accessibility retains Modified and adds Metadata changed. Get Info adds Metadata changed, and listing JSON adds nullable changed_utc with subsecond precision. View settings serialize Changed independently of Created and Modified.

This is the filesystem object's status/metadata change time. Permissions, ownership, link-count and other changes can update it; it is distinct from both birth time and content modification time. The listing captures POSIX signed seconds plus a nonnegative nanosecond fraction from existing lexical metadata, using checked SystemTime arithmetic. Symlinks retain their own time rather than their target's; unknown facts remain unavailable. Rendering, sorting, Info and JSON use retained observations without new stat calls. Non-Unix metadata change time is not implemented, and no timestamp-editing or ctime search predicate is added.

## Validation

Signed-part regression covers epoch, pre-epoch nanoseconds, invalid fractions and checked extreme conversions. A real Unix fixture compares directory enumeration and one-object observation to independent symlink_metadata values for file/folder/live and dangling links, verifies permission changes preserve mtime, and retains Info after source removal. Existing sort regression now covers Changed before/after epoch, unknown-last both ways, stable ties, folder grouping and serialized views; Info unknown and listing JSON pre-epoch/subsecond cases include the new field.

Native changed_sort drives six real palette commands (Changed ascending/descending, Modified ascending/descending, then Changed ascending/descending), preserves a hidden selection and cursor, and records listing JSON. Independent raw ctime seconds/nanoseconds and followed display mtime facts drive verify.py. Existing file_identity_info is the neighboring native regression. Source, binary and external checksum-oracle hashes bind acceptance; native Windows execution and general filesystem freshness are not established.

Final evidence: all six checks passed, **3147 tests passed / 3 ignored**; changed_sort and file_identity_info each **14/14** on the same source-bound binary with zero external effects. The independent verifier matched every captured Changed/Modified order against raw observations and checked exact JSON ctime nanoseconds, cursor and hidden selection. Both final images were inspected, including the visible Metadata changed Info row. The ordinary non-QA check retains the pre-existing unused-response warning in diff_find.rs. Full parity with the original 99 repositories remains open.
