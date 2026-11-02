# Command working folders — 2026-10-31

Independently adapts the per-action working-directory control in [Strata custom actions](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#manifest) to Commander's dual-pane shell-template model. Its choices are Commander-specific, not Strata manifest compatibility.

## Behavior

Run command → Template name and file types → **Working folder** selects Opening folder (legacy default), Other pane, or Single selected folder. Resolution uses the immutable opening selection/pane paths, never current live pane state. Single selected folder requires exactly one captured directory row and participates in applicability/Run gating. Paths must be absolute. Resolution performs no filesystem reads and preserves native path values; the OS resolves the directory at spawn time. Missing/inaccessible directories fail without fallback or creation.

The resolved folder is displayed before Run. Existing mutation gates still apply; the trust owner must permit both the opening root and the chosen CWD, so changing CWD cannot bypass opening-root trust. Identical roots are checked once. No labels are changed automatically. The chosen PathBuf is captured once for the entire batch; editing the draft or panes cannot retarget an active run.

Selection values do not move: absolute paths, basenames and opening dir/other-dir placeholders remain captured as before. Relative shell arguments, including names, resolve under the chosen CWD; use paths for absolute opening inputs. Process CWD is local to the child; pane navigation is unchanged.

Save/update retains working-folder policy in its original row. JSON **version 6** exports `working_dir`; versions 1–5 remain readable with their supported policies. Non-default CWD policy under an older envelope and unknown directory modes are refused. Import review identifies working-folder choice. [example.json](example.json) demonstrates a whole-selection pwd command in the other pane.

## Verification

**2991 tests passed, 3 ignored**, zero failed. Build, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks passed; see `checks.json` and logs.

New regressions cover opening/other/selected resolution, immutable original inputs, empty/file/multiple-folder and relative-path refusal, source/target trust rejection and same-root deduplication, persisted metadata update, legacy defaults, v6 exchange/old-envelope refusal, valid v5 execution-mode import, cache invalidation and applicability. Real Unix shell tests use pwd to verify both alternate target modes, and prove that a missing CWD does not execute a marker command in the opening directory or create the target.

Native **per_item_command**: **16/16** capture checks, framebuffer inspected. A saved per-item template chooses Other pane, invokes pwd plus name printing twice, and completes 2/2 successfully. Independent assertions over `per-item-state.json` require two physical-CWD output lines equal to the other pane, exactly one line for each original input name, and original selection paths still in the left pane. The native fixture canonicalizes its expected path for macOS /var → /private/var aliasing without changing production resolution. Binary/core/launch hashes match; source hashes are recorded. Native effect-port counters remain zero while harmless shell execution is intentionally exercised separately.

## Boundaries

Native coverage qualifies other-pane per-item CWD; selected-folder CWD and trust-denial paths are real-process/model regressions rather than additional native captures. Native fixture context/templates are injected without host template/trust writes; normal Run trust/mutation checks execute. The earlier green run before expected-path normalization is retained in `before-native-path-normalization`.

This is path/snapshot binding through existing shell/trust owners, not an atomic directory-identity or sandbox guarantee. No Home preset, action-owned directory, arbitrary-path editor, new MIME/runtime/manifest compatibility or full cohort parity is claimed. Existing cancellation, output capture and persistence owners remain unchanged.
