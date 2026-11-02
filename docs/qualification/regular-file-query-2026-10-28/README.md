# Ordinary-file search — 2026-10-28

Independent adaptation of fd regular-file classification from cohort repository #77 ([primary CLI source](https://github.com/sharkdp/fd/blob/master/src/cli.rs)); no upstream code copied.

`type:regular` (alias `type:regular-file`) selects the entry itself using observed lexical metadata. Native hard links qualify; symbolic links, dangling links, directories, FIFOs and sockets do not. Existing `type:file` retains its non-directory semantics. ZIP regular payloads and TAR regular headers qualify; TAR symbolic/hard-link headers do not. ZIP explicit special Unix types are excluded; missing/permission-only mode metadata uses ordinary payload classification.

New index snapshots retain optional regular-file classification through JSON round trips, with no live restat during indexed matching. Old snapshots load but unknown classification cannot satisfy this predicate and counts as skipped; rebuild to populate it. The existing serialized-document budget is regression-tested with the new field. Saved queries preserve the predicate and canonical expression.

Focused tests cover all text modes across native, JSON-roundtripped index, ZIP and TAR, ordinary/hard-linked files, directories, live/dangling symlinks, native FIFO/socket, archive symlinks and TAR hard-link headers. They also verify unchanged type:file results and skipped accounting for old index records. Explicit ZIP special attributes and native device nodes are not fixture-tested. No UI widget change, new native screenshot, commit or push.

Validation: **2544 passed, 0 failed, 3 ignored** in the full locked visual-qa suite, including the maximum serialized-metadata budget regression. Both focused tests passed. All-target visual-qa Clippy with warnings denied, formatting and whitespace checks passed.
