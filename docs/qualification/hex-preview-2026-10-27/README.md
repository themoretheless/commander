# Paged read-only hex viewer — 2026-10-27

Independent implementation inspired by **Show as Hex** in
[Double Commander's built-in viewer](https://doublecmd.github.io/doc/en/viewer.html).
No upstream code or new dependencies are included.

## User behavior

**View as hex** in the command palette opens the file under the cursor in a
floating **Hex preview** window. It uses the cursor file rather than the marked
selection, preserving that selection. Directories, symlink entries and special
files are refused. This is an explicit binary-inspection action; the ordinary
text preview's binary rejection and text encoding policies are unchanged.

Each row has an absolute 16-digit hexadecimal byte offset, 16 hexadecimal bytes
in two groups of eight, and printable ASCII. Other byte values show as dots in
the ASCII column; the hex column preserves every byte. These dots are a display
choice, not source-data replacement. Partial final rows remain correctly padded.

The viewer reads 4096 bytes per page. **First**, **Previous**, **Next**, **Last**
and a byte-offset field allow navigation throughout the file. Decimal offsets
and `0x`/`0X` hexadecimal offsets are supported; unaligned byte offsets are valid.
Offsets beyond EOF fail explicitly. Offset zero is valid for an empty file;
exact EOF displays an end-of-file state. **Copy page** explicitly copies the
current page's offsets, hex and ASCII through Commander's clipboard port.
It does not copy the entire file or modify source data.

**Reload** accepts the currently observed file and returns to zero. Page reads
otherwise remain bound to the initially loaded file's identity/version. A detected
change clears the page and asks for Reload. **Cancel read**, closing the window,
or opening another Hex preview cancels the viewer's outstanding task and drops
its result receiver. There is no automatic content editing, byte search,
decimal-byte mode, raw-byte export, linked hex/ASCII selection or native editing
claim; wider upstream viewer parity remains open.

## Read and resource boundaries

Reads run on the shared workload scheduler as interactive Preview jobs. Only
one page task can be pending per viewer. The job seeks directly to a u64 byte
offset and allocates at most 4096 raw bytes. Visible rows are virtualized; the
explicit copy formatter is bounded by the current page. There is no whole-file
load, mmap or dependency on the text preview's 256 KiB file cap. A sparse file
beyond 4 GiB is tested without allocating its logical contents.

Before opening, after opening and after reading, the worker compares regular-file
identity, size and mtime. Unix additionally compares ctime seconds/nanoseconds;
opens use O_NOFOLLOW for the final path component and nonblocking/close-on-exec
flags. The path and open descriptor are both rechecked before publication.
Replacement, changed metadata or short reads do not publish a partial page.
These checks do not create an atomic filesystem snapshot or exclude undetectable
concurrent writes; pages already displayed are snapshots of their last read.
Parent-directory symlink traversal is not prohibited. The qualification host is
macOS; equivalent native behavior on other hosts is not claimed.

Cancellation is checked around metadata/open/read work and between short reads.
It cannot preempt a blocked OS call. An abandoned or disconnected worker produces
an explicit terminal UI error rather than an indefinite spinner. Old receiver
completions cannot replace a newer request. Shared scheduler admission errors are
shown without leaving a task active; cancelling this viewer does not cancel
other preview consumers. Successful completion disarms the task's cancel-on-drop
path, avoiding recording normal completion as a user cancellation.

## Verification

Tests cover all 256 byte values, exact hex and safe ASCII columns, multiple pages,
unaligned offsets, partial final rows, empty files/EOF, parse overflow, a sparse
file past 4 GiB, unchanged source bytes, cancellation inside a page read, file
replacement, final-component symlink substitution, directories and FIFO refusal.
Deterministic scheduler tests cover one pending task, close cancellation,
completed-task accounting, discarded old completions, abandoned jobs and
admission failure. A workspace integration test verifies cursor targeting and
preserves the other marked file.

The native `hex_preview` scenario creates a real 8197-byte fixture, invokes the
workspace command, clicks Next (offset 4096) then Previous (offset zero), enters
`0x2000` into the offset field and clicks Go. Capture requires the resulting
8192-byte offset after the asynchronous read. The macOS eframe/Glow screenshot
was visually inspected: its final row shows bytes 00 01 02 03 04 at offset 2000.
The native framebuffer checks also validate glyphs, pane hover, layout and port
isolation; no clipboard/open/trash effect was requested. The CI matrix includes
this new scenario (16 cases); no remote CI run is claimed.

Final results: **11/11 targeted hex/workspace tests**, full suite **2298 passed,
3 ignored**, Clippy with all targets/visual-qa and `-D warnings`, formatting
and scoped diff checks passed. Native frame checks: **14/14 passed**. Changes
remain local and uncommitted; no push or remote CI result is claimed.
