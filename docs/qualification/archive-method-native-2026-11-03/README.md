# Native ZIP member metadata qualification

Closes the concrete native-visibility gap from archive-method-metadata-2026-11-03. New ArchiveMetadata scenario uses three real ZIP entries: Stored, Deflated and an intentionally undecodable entry whose local and central headers declare encrypted method 222. Fixture setup alone creates the bytes; subsequent selection, disclosure and scrolling use actual egui pointer/wheel events, never direct member-focus/filter mutations.

The driver clicks the stored member, expands Member details, then clicks the deflated and encrypted members. Read-only probes collect the actual rendered labels, values and full-row clip visibility. Each intermediate observation must be recorded before advancing; final capture requires all eight labels and values visible. Intermediate JSON proves semantic display/clip visibility; only the final encrypted framebuffer was visually inspected. No extraction is attempted.

ArchiveExport is also recaptured to verify that adding two CSV columns preserves its five-action exact clipboard contract on TAR, where new per-member ZIP values are blank. These are recording-backend clipboard writes, not OS clipboard claims.

The first ArchiveMetadata attempt reached and recorded all three states but failed the common screenshot hover gate: the pointer stayed on the archive pane. The driver now permits the standard background-hover step after phase 12. The failed capture is retained. An initial full check failed compilation because the explicit scenario-name test list was not updated from 136 to 137; rejected-checks retains that evidence. No production behavior was relaxed to pass either check.

prepare_capture.py stages a fresh app binary with launch hashes and rejects a live owned process. verify.py independently asserts fixed fixture display values, eight visible metadata fields, fourteen common native checks, expected effect counts, and equality of captured and final binary hashes. Gates and final source hashes accompany this report. Password extraction, encryption algorithm identification, minimum window, all archive variants and VoiceOver are outside this specific qualification and remain open.

Final verification: all seven gates passed; 3377 tests passed, 0 failed, 3 ignored. Strict visual-QA Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. Both successful native captures pass 14/14 common checks; verify.py passes against the final build.
