# Native rename name-list file import

Extends the native rename_sanitize scene with actual path-field typing, Load names from file and Use loaded names pointer events. A two-row file for one source loads without changing the prior editor/preview; clicking the disabled Use action retains the editor. Changing to a valid UTF-8 BOM/CRLF one-row file then loading still retains the editor until a separate Use click. Final target must be imported.txt. The source file is never renamed by this capture.

Seventy-seven input phases are acknowledged only after the UI processes them; phases 63 and 74 wait for actual background import results before clicking. Seventy-eight snapshots observe editor, validated preview and import state. Python checks fixed expected rules/targets and byte-exact source/import artifacts, captured binary/source hashes, native effects and all seven gates. The normal ten-second deadline and 300 ms settling time remain.

Full native apply, multi-file file-import mapping, cancellation interaction, permission-error rendering and intermediate layout remain separate scope. Core reader and real-egui workload tests are documented in the preceding file-import qualification. No OS clipboard/rename side effect is expected.

Final receipt: 1793654385940525000/rename_sanitize, captured 16/16 with zero native-port effects. Owned process terminal. The final sheet image was inspected: file path, Load action and imported editor text are visible; lower target preview/submit rows are offscreen and not visually qualified. App-not-found after launching reflects the already terminal capture, resolved through the completed manifest and process check. verify.py passed for all 78 editor/cache snapshots and fixed file-import states with matching source/binary hashes. All seven gates exited 0; tests 3357 passed, 0 failed, 3 ignored. Ordinary check retains the pre-existing diff_find.rs unused-response warning.

The BOM/CRLF import artifact is marked -text -diff locally so Git stores its original bytes without line-ending normalization. The staged blob was compared byte-for-byte with the captured source.
