# Archive member type filter — 2026-10-30

Independent adaptation of [ranger filter_inode_type](https://github.com/ranger/ranger/blob/master/ranger/config/commands.py), cohort #30. Verified upstream supports combining directory/file/link categories. Commander independently filters archive metadata and adds Other members for remaining record types.

The Types menu combines Regular files, Directories, Symbolic links and Other members; all default on, and Show all types resets the four flags. All-off is permitted. An active Types filtered indicator remains visible outside the menu. Classification uses existing admitted archive metadata: symbolic link first, directory next, regular file next, otherwise Other. It does not follow links or inspect member payloads. TAR hardlinks and special records belong to Other, not Symbolic links. Availability of metadata follows the existing ZIP/TAR parsers, not MIME inference.

Type predicates intersect current path query, folder and narrowing snapshot. Cache invalidation includes the four flags independently from regex compilation. Hidden selections, saved selection and narrowing snapshots remain intact; selected exports/extraction retain archive-wide scope. Hidden-selected counts account for type filtering. Real directories appear in All members as before; folder mode retains its established direct-file policy. Synthetic navigation folders retain path filtering and remain available irrespective of type flags, unless narrowing suppresses them; they never become extraction members. Thus all-off can still show navigation folders in folder mode. Filtering away cursor/anchor clears stale IDs through the existing visibility owner.

Two regressions exercise cached type transitions and combined flags, actual folder trees, navigation exclusion from real selection, hidden/cross-type selection and exports, narrowing intersection, invalid regex and default reset.

Native Types clicks, indicators and layout remain pending because desktop CUA is unavailable. Earlier archive native gates remain open; no native capture success is claimed.

Validation:

- Initial locked visual-qa full suite: **2760 passed, 3 ignored, zero failed** (`test.log`).
- Initial Clippy rejected a test's post-default field assignment; rewritten as a struct initializer without changing behavior. Rejected output retained (`clippy-initial.log`); final Clippy with all targets/visual-qa and warnings denied passed (`clippy.log`).
- Final source test rerun: **2760 passed, 3 ignored, zero failed**, retained separately (`test-final.log`). Locked default check, build, formatting and whitespace logs accompany this report.

Locked default check, visual-qa build, formatting and whitespace checks all passed. Source/binary hashes and pending native-validation record accompany this report.
