# Reveal a symbolic link's resolved target — 2026-10-31

Independent adaptation of [Yazi follow](https://yazi-rs.github.io/docs/configuration/keymap/#follow), cohort #27. The command palette adds **Follow symbolic link**, offered only for a highlighted symbolic-link row and a closed confirmation. It uses the cursor, independently of selected rows, rechecks the live lexical file type and resolves the path with the host filesystem's canonicalizer. Relative targets and link chains therefore resolve against their actual containing directories; a broken, looping, missing or replaced non-link entry produces a named error notice before changing navigation.

A successful action navigates the active pane to the resolved target's parent and focuses the target row. A target directory is revealed as a row rather than implicitly entered; a root target navigates to the root itself. Normal pane history records the transition, so Back returns to the source folder. The inactive pane is not navigated. Filters in the target folder are cleared, and a dot-prefixed target enables the existing transactional hidden-file listing before requesting focus. The focus request uses the existing deferred child-focus owner when enumeration is asynchronous. Target payloads are neither opened nor modified; the command is classified as navigation rather than a filesystem mutation.

The action uses the existing display-name child-focus convention, not a new native-byte identity/focus system. Canonical path resolution is synchronous, with the operating system's link-chain/error behavior; no new network-provider or custom resolver worker is introduced. Existing unreadable-directory/listing and concurrent replacement semantics still apply.

## Verification

3034 tests passed, 3 ignored, zero failed. Six checks exit zero: visual-qa build/full suite/all-target Clippy with warnings denied, default check, formatting and diff whitespace.

Four new regressions use actual host symlinks. They verify a relative link through a chain to a file (while another row is selected), target cursor placement, unchanged inactive pane/payload and Back history; a folder target is revealed in its parent; a hidden target clears a remembered excluding filter and becomes visible/focused; broken/looping links emit error notices without navigation, and a regular row is unavailable. A deterministic background-listing regression qualifies the hidden-toggle/navigation/focus sequence after queued enumeration publishes. Existing command catalog uniqueness, availability/reason and classification checks cover the new entry/predicate. Existing relative-link creation tests remain green.

No new GUI layout was added beyond the existing command catalog entry. Native GUI palette selection, physical key delivery, unreadable target-directory races and ambiguous lossy display-name focus remain unqualified. Read-only navigation and host filesystem behavior are the scope of this transfer; full Yazi/99-project parity remains open.

## Native workflow follow-up

A later [native palette scenario](../symlink-workflow-native-2026-10-31/README.md) passed 14/14, covering real relative-file-link creation with a collision, both recording clipboard payloads and Follow target-row focus. This supersedes the normal palette-delivery pending boundary only for that workflow; the report specifies remaining variants and OS pasteboard/input limits.
