# Synchronize: compare file contents

Independent adaptation of [Double Commander Synchronize directories by content](https://doublecmd.github.io/doc/en/syncdirs.html). No upstream code copied. Commander compares all paired regular files in this mode, including pairs with differing dates.

**Compare file contents** performs fresh, bounded-buffer reads in the existing background plan worker. Equal bytes default to Skip; differing bytes override metadata-only equality. Same/Different/Unverified contents scopes and CSV/JSON row facts expose the result. Links, special files, failed reads and changed bindings are unverified and cannot authorize a copy. Switching mode cancels/rescans and resets an inapplicable content-only scope. Directory pairs remain shallow; one-sided rows are unchecked.

Protected file descriptors and captured lexical identities reject replacement and link traversal during comparison. Mode and file identity are included in guarded-plan context. This is a scan-time snapshot: later guarded copying is not an atomic multi-file transaction and does not re-read every compared byte at commit. Arbitrary writers preserving metadata are outside that guarantee. Explicit row edits may copy a verified equal pair; bulk defaults skip it.

## Validation

- Six checks in `checks.json`: build, full tests, strict Clippy, default check, formatting and diff whitespace. Full suite: **3096 passed, 3 ignored**.
- Nine new tests cover metadata-equal/different bytes, date-different/equal bytes, empty files, sizes, all policies, fresh reads after a warmed hash cache, short/interrupted reads, premature EOF, cancellation, link/socket/replacement refusal, review/default/report handling and actual guarded copying. The integration test verifies destination bytes and preserves the timestamp of an equal file that must not be copied.
- Native `sync_contents`: **15/15**, two recording clipboard writes. Actual checkbox/scope/copy interactions toggle the mode twice, verify unknown-link skipping and discover changed bytes after restoring the same size/date. Initial and final full reports contain all 14 rows, including the unknown pair.
- Existing native `sync_review`: **15/15**, four recording clipboard writes. No opener, trash or other native-port effects in either scene. Native captures review/export plans; real copying is exercised by the isolated integration test.
- `verify.py` independently checks final executable hashes, captured manifests, all nine phase snapshots and exported content facts. `sync_contents/frame.png` is the inspected final frame.

Launch records retain source capture directories. The verifier copies their artifacts locally for review and can subsequently reuse the local copies. Earlier checks before adding the real-copy test remain under `before-copy-integration/` and are not final qualification.
