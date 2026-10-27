# Copy names without their last extension — 2026-10-27

Independent implementation inspired by [Yazi’s copy name_without_ext action](https://yazi-rs.github.io/docs/configuration/keymap/). No upstream code or dependency was copied.

## Behavior

Copy name without extension is available in the command palette and uses the existing typed CopyPaths request and ClipboardPort path. It copies visible selected entries in the existing display order, falling back to the cursor when there is no selection. This retains the same scope as other path-copy commands; it does not add cross-folder selection collection.

The formatter uses the lexical final-component stem, removing only the final extension: Photo.JPG becomes Photo, archive.tar.gz becomes archive.tar, .env remains .env, .config.json becomes .config. It also applies lexically to dotted directory and symlink names without querying the filesystem. Root paths fall back to their path text. Unicode and existing lossy handling of non-UTF-8 names follow the existing clipboard formatter. Duplicate stems remain separate lines in input order. Embedded LF/CR/tab characters use the existing one-line escape convention. Source files, cursor and selection remain untouched. The copied feedback label handles singular/plural names without extensions.

## Verification

Three focused tests passed for edge cases, ordering/duplicates/control escaping and workspace request routing with selection/cursor fallback. The existing fixed-command request matrix now includes the new command, and the copied-feedback label test covers its singular/plural labels.

The full regression run recorded **2384 passed, 1 failed, 3 ignored**. Its sole failure exposed an inconsistent dispatch guard: the new command skipped the typed request on an empty panel, while the existing fixed clipboard commands always emit their typed request and leave empty selection handling to the app. Dispatch was made consistent. On the corrected source, the three feature tests and the previously failing fixed-command matrix test passed. The latter test’s original failure is retained under rejected/. The full suite was not rerun solely to replace that rejected log; this report does not claim a fully green final broad run.

Final all-targets Clippy with visual-qa and warnings denied, formatting and scoped whitespace checks passed. The initial compile’s Result/iterator API mistakes are also retained under rejected/. No real system clipboard was changed during tests. Native window/OS clipboard behavior was not newly qualified for this feature; the existing 22 native scenarios and their effect policies remain unchanged.

No commit or push was requested or performed.
