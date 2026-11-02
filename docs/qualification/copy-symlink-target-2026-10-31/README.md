# Copy recorded and resolved symbolic-link targets — 2026-10-31

Commander-specific extension of the [Yazi follow](https://yazi-rs.github.io/docs/configuration/keymap/#follow) transfer, cohort #27: the resolved target can now be copied without navigating. This is an independently designed pair of clipboard commands, not a claim that Yazi exposes the same clipboard actions or shortcut set.

The command palette adds **Copy symbolic-link target** and **Copy resolved symbolic-link target** for the highlighted symlink. The first reads the target stored in the link, preserving relative versus absolute spelling; it works for dangling links. The second uses the host canonicalizer to resolve the complete chain to an existing absolute target; broken/looping/inaccessible resolution fails explicitly. Both recheck that the live path is still a symlink, use the cursor independently of selection, and preserve both pane roots/cursor/source content.

Successful actions stage one existing CopyText request, retaining normal clipboard success/error ownership. Failure stages only a notice, so the current clipboard is not replaced with an empty/partial result. Text must be valid UTF-8; non-text target bytes are refused rather than replaced lossily. Newlines, carriage returns and tabs use the existing one-line path escaping policy; ordinary spaces/backslashes remain normal path text. This is a text path copy, not file-object copy/cut/paste or a shell-quoted command. Resolver/readlink metadata calls do not read target payloads or write files. Resolution is synchronous with the existing native filesystem semantics.

## Verification

3038 tests passed, 3 ignored, zero failed. Six checks exit zero: visual-qa build/full suite/all-target Clippy with warnings denied, default check, formatting and diff whitespace.

Four new regressions use real host symlinks and the workspace command dispatcher. They distinguish a chain's recorded `middle` target from its resolved absolute file path while another row is selected; verify unchanged roots/cursor/payload and read-only classification; copy an escaped dangling target while broken/looping resolved requests emit only notices; replace a previously listed symlink with a regular file and require live-type refusal; and create an actual symlink containing target byte `ff`, requiring a lossless-refusal notice and preserving the original byte. Existing command availability/catalog checks cover the two registry entries. Existing clipboard formatter/port ownership remains unchanged.

The tests qualify exact staged clipboard text and absence of a copy request on failure. Physical OS pasteboard readback and native GUI palette delivery are not newly qualified. No assertion of upstream clipboard compatibility or complete 99-project parity is made.

## Native workflow follow-up

A later [native palette scenario](../symlink-workflow-native-2026-10-31/README.md) passed 14/14, covering real relative-file-link creation with a collision, both recording clipboard payloads and Follow target-row focus. This supersedes the normal palette-delivery pending boundary only for that workflow; the report specifies remaining variants and OS pasteboard/input limits.
