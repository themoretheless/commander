# Open the opposite pane's folder here — 2026-10-28

Independent adaptation of [Double Commander LeftEqualRight/RightEqualLeft navigation](https://doublecmd.github.io/doc/en/cmds.html). No upstream code copied. Open other pane’s folder here pulls the opposite pane's current path into the focused pane without switching focus. Existing Equalize panels pushes the focused pane's current path to the other side.

Both directions use normal destination navigation and its Back/Forward trail and remembered view/selection handling. The source pane and other tabs remain owned independently. Already-equal lexical paths are unavailable/no-op rather than resetting the destination view. The destination preview is cleared when its path changes. No canonicalization or filesystem probes were added to capability checks.

Both directions now reject pending operations, active deletion and pending workspace modal ownership before changing folders. This also repairs the original Equalize panels route's missing modal guard. The new action has no default shortcut; Cmd+E retains its existing push direction.

Validation:

- Full local suite: **2443 passed, 0 failed, 3 ignored**. New regression exercises both focused sides × both push/pull commands, source snapshot preservation, destination path, focus retention, same-path capability/no-op, Back restoring the previous path/filter/selection and Forward returning to the matched path.
- Separate modal regression checks both directions from both focused sides cannot change either pane's saved state while a rename modal is pending. Existing Equalize panels tests also passed.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native palette interaction/capture or actual desktop restart was run for this command. No commit or push.
