# Native saved rename preset ordering

The RenamePresets scenario now runs 56 phases. After the previous rename/copy/default/close/reopen workflow, pointer input sorts descending (Copy, Album), moves selected Album up (Album, Copy), moves it down (Copy, Album), then sorts ascending (Album, Copy). Each real UI frame acknowledges its input. The scenario waits at least 300 ms after the final step before accepting capture, so the reopened sheet fade settles. No deadlines or production animation settings are changed.

Receipt: native-captures/1793638139156712000/rename_presets. All 16 checks passed, 46 phase observations retained, zero native effect calls. `verify.py` independently checks every ordering transition, unchanged saved rule/editor/selection, default identity, and the earlier management/reopen transitions. The successful framebuffer was inspected: the visible preset controls are opaque, readable and do not overlap. The rest of the vertically scrolling sheet is not a full-layout qualification.

This supersedes the ordering success-path native gap and the prior opening-fade screenshot limitation in rename-preset-native-2026-11-02. Native errors, boundary-disabled input, session restart and date-control interactions remain separate scope; core/session tests cover order persistence and boundaries. The CUA call reported an accessibility error; the subsequent authoritative manifest and process check showed successful capture and a terminal process. No retry was needed.

Seven gates and source hashes accompany the capture. No new production feature or full cohort parity is claimed by this qualification pass.

Final result: all seven gates passed; 3321 tests passed, 3 ignored. Current rebuilt binary matches the successful capture SHA-256.
