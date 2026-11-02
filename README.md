# Commander

Batch rename can add a **Modified date (UTC)** prefix or suffix, using a
strftime format such as `%Y-%m-%d_`. Dates come from the listing captured when
the dialog opens and stay attached to each original file when rows are reordered.
Missing dates or invalid formats block the batch. Saved rename presets retain
the format and position, without storing file timestamps.
Saved rename rules also support **Rename selected** and **Duplicate saved** using
the name field. Duplication copies the saved rule and keeps the source selected;
load the copy to edit it. Current unsaved editor changes are retained.
**Use selected at launch** loads that saved rule when the next rename dialog
opens; **Start empty** restores the empty-rule default. Loading never renames files.
**Preset up/down** and **Sort A–Z / Z–A** organize saved rules; the order is
restored with the session, while the current editor and launch choice are retained.

Panels offer **Columns** alongside List and Icons: scroll through parent folders
beside the current folder, descend with one click, or bring an ancestor file
into the current action column. Selection and keyboard actions stay with that
column. Left goes to the parent; Right enters the highlighted folder.
Drag column dividers to resize; double-click to restore automatic widths.
Widths are restored for open tabs across restarts; up to eight parent
columns are shown. Each pane also restores its eight most recently closed tabs
after restart, including their names and view settings. The Closed menu also
lets you forget individual entries or clear that pane’s history.

The **Workspace profiles** palette command saves named tab sets for both panels,
including active tabs, active side, names, view settings and current-panel filters.
Apply replaces open tabs while preserving closed-tab history; protected tabs block
replacement. Rename or duplicate a saved profile without applying it. Older
profiles retain their current-folder-only behavior. The profile window also exports
a selected profile as JSON and reviews pasted JSON before adding a new profile;
importing does not apply it or overwrite an existing name.

The synchronizer’s **Saved sync profiles** saves named folder pairs and comparison
settings with the session. **Load and compare** builds a fresh plan and clears old
row edits and large-plan approval; loading itself never copies files. Rename or
delete individual saved profiles without loading them. **Swap comparison sides**
exchanges folders and hidden-file settings while preserving the physical copy
direction and rebuilding the plan.

In synchronizer review, **Name masks** finds rows with patterns such as
`*.jpg;*.png;!thumb*`. Use **Skip shown** to explicitly exclude matching actions;
filtering alone leaves the plan unchanged.

When duplicate search includes both panel folders, **Prefer primary folder** and
**Prefer other folder** choose one keeper per matching group. Groups without a
copy in that folder retain their current choice; review the proposed Trash list.
**Copy keeper paths** and **Copy proposed Trash paths** export the current choices
as plain full paths, one per line. **Undo plan edit / Redo plan edit** restore
keeper choices before deletion; a new scan clears this in-memory history.

**View and settings → Custom theme / JSON** edits a personal palette through
color controls or versioned JSON. Apply saves it with the session; applied JSON
can be copied for sharing. Increase Contrast takes priority over custom colors.

**Select by mask** offers **Add**, **Remove** and **Invert** for masks or pasted
exact filenames, with a preview of changes in the current filtered view.
Remove/Invert preserve mask exclusions; Add keeps exclusion subtraction.

The command palette offers **Save selection for this tab**, **Restore tab selection**
and **Forget tab selection**. Saved sets follow open and recently closed tabs
across restarts; restoring is explicit and skips missing entries. Each set is
limited to 1,024 names and 14 KiB of native path data.

**Keyboard shortcuts (F1)** lets you assign custom workspace key sequences,
for example `Alt+G H`, to commands. Apply validates conflicts; settings restore
with the session. Basic navigation and selection motions can also be rebound;
holding their single keys repeats movement in List and Icons. Fields, dialogs
and viewers keep their own input. Applied keys appear in F1, the command palette
and the action bar; the palette can also search by a configured key sequence.
The F1 **Import / export shortcuts** section exchanges applied sets as Commander
JSON; loading changes the draft, and Apply activates the reviewed set.

Get Info (**Cmd+I**) shows format and header dimensions for JPEG, PNG, TIFF and
WebP even without EXIF, plus available camera, lens, capture time, exposure, ISO,
orientation and GPS. It also shows available author/rights/software, recorded
capture UTC offset and fractional digits, and exposure/metering modes.
Metadata reads run in the background
and stop at 2 MiB; missing or limited metadata is reported explicitly.
**Copy metadata JSON** copies the loaded display snapshot with its source identity,
without rereading the image.

GPX and KML previews show an offline route schematic, separate track segments, estimated
distance and recorded elevation profile. **Route / Source** switches to the XML.
Files are limited to 256 KiB and 10,000 points. KML supports LineString and
gx:Track; only absolute altitude enters its elevation profile.
GPX point timestamps and aligned KML gx:Track times show covered duration and
average speed across timed legs; missing times and segment gaps are excluded,
with coverage shown. Recorded elevation also shows raw gain/loss totals from
adjacent samples, with missing values and segment gaps excluded. Hover near a
recorded map point to inspect its coordinates, elevation, time and distance.
**Copy route JSON** copies parsed points, segment boundaries and timing/elevation
statistics as a versioned report, limited to 1 MiB.

Find’s **Copy as → Relative paths** copies paths from the applied search root,
including archive `!/member` references, in displayed order or for selected rows.

Find offers **Sort by** for original rank, natural name, full path, listed size,
allocated size, modification/access/creation/status time and hard-link count.
**Descending** reverses the key; date groups keep their age order and unknown
values stay last. Highlight follows the same result, and copied reports follow
the displayed order. Sorting uses retained metadata without rerunning the query.

Find’s Copy as → Parent folders copies distinct containing directories in result
order, for all visible results or the selection. Archive members use the folder
containing the physical archive; directory hits use their parent.

Size queries parse whole and decimal binary-unit values with integer arithmetic,
including values beyond floating-point exact-integer precision. Fractional sizes
round to the nearest byte, ties upward, before applying comparisons. Overflow
after unit scaling/rounding is rejected. `size:>18446744073709551615B` is rejected
because no supported byte count can satisfy it; `>=` at that limit remains valid.

Find supports precise elapsed modification age: `age:<=30min`, `age:>2h`, or
`age:>=30min age:<2h`. Operators `<`, `<=`, `=`, `>=`, `>` compare elapsed time
without rounding away available subseconds; omitted operator means `<=`.
Use whole values with `s`, `m`/`min`, `h`, `d` or `w` (long unit names also work).
Unknown modification times do not match; future timestamps have age zero.
Conditions persist in saved queries and use live/indexed/archive metadata.
The legacy `date:` field retains whole-day behavior and `m` = 30-day month;
`age:` uses `m` = minute. Archive time precision follows the recorded metadata.

Right-click a Find result for reveal/open, background-tab, row path/name copying,
hide and match-explanation actions. Right-clicking outside the selection picks
that one result; right-clicking a selected row keeps the group for group actions.
Row copy actions explicitly copy that row's path/name regardless of Copy as.
Archive paths retain member references when copied; reveal/open uses containers.
Actions share the existing tab guards, clipboard feedback and result hiding.

Choose **Details (CSV)** in Find's **Copy as** selector to copy result metadata:
full path, name, file/folder kind, byte size and UTC modification timestamp.
CSV quoting preserves commas, quotes and multiline names; paths retain virtual
archive members. Folder sizes and unknown/unrepresentable dates are blank, and
available fractional seconds are retained. Both copy scopes follow visible order
and exclude hidden rows, using the already observed metadata without file reads.

Find's **Copy as** selector offers full paths, names, POSIX shell arguments,
names without extension, file URLs and CSV details for both copy buttons. Paths/names remain one result per line with control
characters escaped. Shell arguments are individually single-quoted and joined
with spaces, preserving embedded controls inside quotes; archive hits use their
real container paths in this format. Plain full paths retain virtual members.
The choice lasts for the current Search window and does not execute a command.

**File URLs** copies one percent-encoded `file://` URL per visible or selected
result, preserving order and repeated targets. It uses real native paths;
archive members refer to their container file, without extraction or a virtual
member suffix. Spaces, URL punctuation, controls and raw Unix filename bytes
are encoded by the same formatter as pane file-URL copying. Relative legacy
targets are resolved against the process working directory without probing or
canonicalizing their targets. This copies text for links, not a file-transfer
clipboard payload. The format picker explains archive behavior on hover.

**Names (no extension)** copies only the final component with its last dot
suffix removed: `archive.tar.gz` becomes `archive.tar`, while `.env` stays
`.env`. This is a lexical operation, including dotted directory names, using
the same formatter as pane copying. Archive results use the member's name.
Visible/selected order and duplicate names are preserved; line controls are
escaped so each result occupies one line. Files are not renamed or read.

Use **Remember selection** in Find to keep one selection snapshot for the current
search root, then **Restore selection** to replace the selection with remembered
locations present in the visible results. The snapshot survives query refinements
and temporary hiding; missing rows can be restored when they return, on another
explicit restore. Other roots cannot use it. Changing roots before new results
arrive disables these actions for the old result list. Remembering again replaces
the snapshot; closing Search discards it. This does not persist in saved queries
or change pane selections.

Find provides **Select all visible** and **Invert selection** for the currently
received result list. Hidden rows are excluded; new streaming results remain
unselected until another selection action. Both actions reset the Shift-range
anchor, keep the keyboard cursor independent and return focus to the query.
They do not affect pane selections or change Cmd+A text selection in the query.

Use **Open selected tabs** in Find to reveal each selected visible result in a
background tab, in displayed order. The group opens beside the active tab while
keeping Search, both live panes and their selections in place. Targets are
focused when their new tab is activated. Archive hits reveal their containers;
each selected result gets its own tab, including repeated containers. The entire
group must fit the existing eight-tab limit and pass operation/target guards;
otherwise no tabs are created and Search displays a refusal.

Use **Hide selected** in Find to remove a selected group from the visible result
list. Files and total search counts stay unchanged. **Show all results** restores
the rows without reselecting them. The action is disabled without a visible
selection and returns focus to the query. Result actions wrap in narrow windows.

Find results support group selection: use the row checkboxes or Cmd-click to
toggle individual results; Shift-click adds the visible range from the last
selection anchor. **Copy selected** copies only selected visible paths in their
displayed order; **Clear selection** resets the selection. **Copy paths** still
copies every visible result. Ordinary row clicks retain reveal/open behavior.
Selection follows result locations across incoming batches and sorting; hiding
a row deselects it, and restoring rows does not reselect them. A new search resets
selection. Group selection does not change files or the pane's selection.

Use **Copy paths** in Find to copy currently visible result paths in displayed
order, including time grouping. Hidden results are omitted; archive members keep
their full virtual member paths. Each result occupies one line, with line
controls escaped using the same formatting as pane path copying. The action
keeps Search and its results open and reports clipboard failures normally.
During streaming, it copies the results received so far; an empty visible list
disables the action.

Find only ordinary files with `type:regular` (alias `type:regular-file`).
Live search and new index snapshots classify the entry itself, excluding
symbolic links, directories, FIFOs and sockets. Native hard links qualify.
ZIP and TAR regular payloads qualify; TAR hard-link headers do not.
Old index records without this classification count as skipped; rebuild the
index to populate it. Existing `type:file` keeps its non-directory meaning.

Search observed Unix permission bits with `perm:ur+uw-ux+`: `+` requires a
bit and `-` forbids it. Concatenate requirements, or repeat `perm:` conditions;
all requirements must hold. Supported labels are `ur/uw/ux` (owner),
`gr/gw/gx` (group), `or/ow/ox` (others), `us` (setuid), `gs` (setgid) and
`sb` (sticky). These inspect mode bits, not effective access or ACLs, and can
match directories and links using their own metadata. Combine `type:regular`
to limit results to ordinary files. Native Unix search, new index snapshots
and ZIP/TAR mode metadata are supported. Missing modes (including old indexes
and native non-Unix metadata) count as skipped; rebuild old indexes. Saved
queries retain the conditions.

Use `|` between permission groups for OR: `perm:uw+|gw+|ow+` finds any write
bit. Prefix a group with `!` to negate its entire conjunction:
`perm:!ur+uw+ux+` means the owner lacks at least one of those bits. Terms
within a group combine through AND; repeated `perm:` fields also combine
through AND. Octal masks of one to four digits use the same signed rules:
`perm:0750+` requires all specified bits, `perm:222-` requires all write bits
clear, and `perm:!700+` negates the full owner-access requirement. Other bits
remain unconstrained. Unknown modes never match, even under negation.
Empty groups, contradictory requirements and unsupported punctuation are
rejected. A condition is limited to 64 groups and 4096 bytes.

Find numeric owners with `uid:501` and groups with `gid:20`. Prefix an ID
with `!` to exclude it (`uid:!0 gid:20`); repeated conditions combine through
AND. IDs are decimal unsigned integers, independent of text matching mode.
Native Unix search observes the entry's own UID/GID, including a symbolic link
itself; cached searches retain the index observation until rebuilt. TAR uses
member header ownership; ZIP and old/non-Unix native indexes have unknown
ownership and count as skipped, including under exclusion. Combine with
`archives:off` for native files or `type:regular` to exclude directories and
links. Saved searches preserve the conditions. No account-name lookup,
permission/effective-access inference or ownership changes are performed.

Find regular files with any Unix execute bit using `type:executable` (alias
`type:exec`). This is a mode-bit filter, independent of filename extension;
it does not guarantee access for the current user or a runnable file format.
Directories, symbolic links and special files do not qualify. Live search and
new index snapshots retain the observed flag; ZIP Unix modes and TAR regular
file modes are supported. Missing mode metadata (including old index records)
counts as skipped; rebuild old indexes to populate it. Combine with content,
name and other conditions through AND; saved searches retain the predicate.

Find links whose targets are missing with `type:broken-link` (alias
`type:broken-symlink`). Missing or non-directory path components qualify;
permission errors and link loops stay unknown and count as skipped. Live
search observes target metadata on its worker; index searches retain the
cached observation until rebuilt. Old link records without that observation
need rebuilding. Combine through AND or `types:broken-link,regular` alternatives.
ZIP/TAR symbolic-link targets remain unknown, although known alternatives can
still match them. This finds results for review without modifying links.

Find symbolic links with `type:symlink` (alias `type:link`), including dangling
links and links to directories. Classification uses the link's own metadata.
Live search, new content-index snapshots, ZIP and TAR member search
retain link kind; hard links are not symbolic links. Existing conditions still
combine through AND and saved searches retain the predicate. Old index records
without link-kind metadata cannot satisfy this filter and count as skipped;
rebuild the index to populate it. Content conditions and archive inspection
retain each provider's existing behavior; unavailable text can exclude links
when content conditions are present.

Find supports absolute modification times: `after:2026-10-01 before:2026-11-01`
or `after:2026-10-28T12:30:00+04:00`. Bounds are strict; date-only values mean
midnight UTC. RFC 3339 values require a timezone and retain fractional-second
precision. Missing modification times do not match. Conditions combine through
AND, work with live/indexed/archive metadata and persist in saved searches.
Existing relative `date:` conditions retain their meaning. Leap seconds are
rejected because filesystem timestamps cannot distinguish them.
New content-index snapshots preserve modification times before the Unix epoch
and their subsecond precision. Older snapshots remain readable at their original
whole-second precision; rebuild an old index to obtain exact observed times.

Use `exts:rs,toml,tar.gz` to match any listed extension, including compound
suffixes. Optional leading dots are accepted. Each list is combined with other
conditions using AND; `case:sensitive` also applies to these lists. Empty list
members and wildcard extensions are rejected. Existing `ext:` remains a single
literal extension condition.

Search queries support `ext:rs` and compound suffixes such as `ext:tar.gz`.
Matching is case-insensitive, requires a dot boundary and excludes directories;
an optional leading dot is accepted. This literal condition is independent of
Exact/Fuzzy/Regex mode and survives saved-query round trips. Dotfiles such as
`.env` match `ext:env`. Repeated extension conditions are combined with AND.

Find also accepts `glob:report-*.pdf` or `glob:"report ?.pdf"`: `*` matches any
run and `?` one Unicode character. The mask matches the entire name, ignoring
case, independently of the selected text mode. Other characters (including
brackets and braces) are literal, as in Commander's selection masks; shell glob
classes and escaping are not supported. Combine it with `path:`, `ext:` or
`content:` conditions. Saved searches retain the mask.

Use `depth:<=1` in Find to search direct children only, `depth:>=2` to skip
the top level, `depth:2` for an exact level, or `depth:1..3` for a range.
The search root is level zero and is never itself a result. Maximum depth
stops filesystem descent; indexed results use the same boundaries. Archive
members add their virtual path components (`bundle.zip!/dir/file` is level
three below the containing folder). Archives at the maximum level are not
opened. Depth conditions are retained in saved searches.

Exclude names with `exclude:node_modules`, `exclude:*.bak` or
`exclude:"build cache"`. Each mask matches a whole name without regard to case,
using the same `*`/`?` syntax as `glob:`. Any matching exclusion removes the
entry and its descendants below the search root. Live search skips excluded
directories and archives; indexed results and archive members use the same
name rules. The root's own name is not excluded. Masks containing `/` are
rejected: these are name masks, not path masks. Saved queries retain exclusions.

Use `excludefile:*.tmp` to exclude matching non-directory results while keeping
matching directories and searching their children. Repeat the condition for
multiple masks, for example `excludefile:*.tmp excludefile:*.bak`. Masks use
whole-name `*`/`?` matching and respect `case:sensitive`; quote spaces, as in
`excludefile:"draft ?.txt"`. Live, indexed and archive-member searches share
these rules. Filtering an archive container's result does not disable searching
its members; use `exclude:` to prune containers or `archives:off` to disable
archive inspection. Saved queries retain file exclusions.

Use `excludedir:node_modules` or `excludedir:*.tmp` to skip matching directories
and their descendants while retaining non-directory entries with the same
name. Symbolic links keep their lexical non-directory classification. The
search root itself is exempt. Live search prunes matching directories; indexed
search filters directory ancestors, and ZIP/TAR search applies the rule within
member paths, including implicit directories. Archive containers are files,
so matching their names does not disable member search. Repeat masks for
additional exclusions; `*`, `?`, quoted spaces and `case:sensitive` behave as
in other name masks. Saved queries retain directory exclusions.

Directory masks containing `/` match a root-relative path: for example,
`excludedir:vendor/cache` skips that branch while preserving `other/cache`.
`*` and `?` stay within one path component; a whole `**` component matches
zero or more directory levels (`excludedir:vendor/**/cache`). Use `./cache`
to target only the root's immediate `cache` directory. A trailing `/` is
optional. Inside ZIP/TAR, paths start at the member root, without the container
name or `!` prefix. Empty/internal `.`/`..` components and embedded `**`
inside a path component are rejected. Existing masks without
`/` continue to match a directory name at any depth.

On Unix, a leading `/` anchors a directory mask to the real filesystem path:
`excludedir:"/Users/me/project/vendor/**/cache"`. These lexical absolute
masks apply to native directories and indexed native paths, including archives
stored below an excluded directory. They do not match virtual member paths
inside ZIP/TAR. Use a relative mask for member directories. The current search
root itself remains exempt; symlinks are not resolved while matching masks.

Find empty files with `type:file size:=0`, or use `size:=1KB` for an exact
size. `type:file` (also `type:files`) selects non-directory entries in
Commander's file model, across all extension categories. Exact sizes use the
existing units (KB = 1024 bytes) and are retained in saved queries. The older
`size:1KB` syntax still means at least 1024 bytes. `type:folder` remains
available for directories.
Integer sizes retain byte precision through the full `u64` range; overflowing
integer sizes or unit multiplication are rejected.

Use `archives:off` in Find to disable searching inside archives while keeping
archive files eligible for ordinary name/metadata matching. `archives:on`
enables member search; omitting the option retains the existing enabled
behavior. `true`/`false` are accepted aliases. Conflicting options are rejected.
Live and indexed search both obey the setting, show it in the provider label
when disabled, and retain it in saved queries.

Find text files without a match using `notcontent:draft`, or combine conditions
such as `content:ERROR notcontent:expected`. The chosen Exact/Fuzzy/Regex mode
applies to both conditions. Empty text files can satisfy a negative condition;
directories cannot. Binary/unreadable input and incomplete reads are reported
as skipped, never treated as proof that text is absent. Live reads remain
bounded at 16 MiB. Indexed absence searches require a complete cached snapshot;
refresh older indexes to populate the new completeness marker. Saved queries
retain negative content conditions.

Use `contentword:cat` for a literal whole-word content match, or
`notcontentword:cat` to require its absence. `cat` matches `(CAT)` by default
but not `catalog`, `cat_1` or `catβ`. Letters, digits, underscores and Unicode
word characters prevent an adjacent match. Punctuation is literal, and quoted
phrases such as `contentword:"error code"` check the boundaries of the whole
phrase. These two conditions stay literal in Exact, Fuzzy and Regex modes;
`case:sensitive` still applies. Conditions combine through AND and persist in
saved queries across native, indexed, ZIP and TAR search.

Whole-word matches inside a bounded prefix are accepted only when both edges
are known. The end of a truncated prefix, including an incomplete UTF-8 tail,
does not establish a word boundary. If no match can be proved in that prefix,
the file counts as skipped. Negative whole-word conditions always require
complete text; old indexes without proof of completeness count as skipped.
Binary/unreadable input keeps the existing skipped behavior.

Use `contentline:READY` to find a complete literal line, or
`notcontentline:READY` to require its absence. Spaces and tabs are significant:
`contentline:" READY "` differs from `contentline:READY`. LF, CR and CRLF
terminate lines; a final unterminated line qualifies only at proven EOF.
`contentline:` (also `contentline:""`) finds an actual empty line. Empty files
have no lines, and a final line terminator does not create an extra empty line.
Values cannot contain CR/LF. These conditions stay literal in every match mode,
respect case settings, combine through AND and persist in saved queries.
Native, indexed and ZIP/TAR text share the same rules and bounded-context
checks as whole-word search; negative conditions require complete text.

Content search detects UTF-8, UTF-16LE/BE and UTF-32LE/BE byte-order marks and
searches decoded Unicode text on disk, in new index snapshots and in ZIP/TAR.
The initial BOM is removed; later U+FEFF characters remain part of the text.
Malformed units use replacement characters, matching the existing lossy text
policy. Without a BOM, UTF-8 decoding and the NUL-based binary check remain in
use; byte order is never guessed from an extension. NUL code units in the
sniffed head still classify the input as binary.

Read/size limits use original file bytes; the index's text quota uses actual
decoded UTF-8 bytes, which can be smaller or larger. Whole-word/line checks
keep their EOF and truncated-tail safeguards after decoding. Rebuild older
indexes to populate previously skipped Unicode content and refresh BOM handling;
existing cached strings are not rewritten in place.

Use `type:empty` to find zero-byte ordinary files and directories with no entries.
Combine with `type:folders` for directories or `type:regular` for files;
`types:empty,socket` accepts either an empty item or a socket. Hidden or excluded
children still make their containing directory nonempty. Links and special
objects are not empty files. Native directory checks read one entry at most;
indexed emptiness reflects the last build, so rebuild older or stale indexes.
ZIP/TAR directory matches require complete, bounded inspection of their member
paths. Partial or unreadable evidence remains skipped rather than proving empty.

Search Unix special files with `type:fifo` (alias `type:pipe`), `type:socket`,
`type:block-device` (alias `type:block`) or `type:char-device` (alias `type:char`).
These conditions use lexical metadata, so a symlink to a FIFO remains a symlink.
ZIP Unix attributes and TAR special-entry headers are also supported. Rebuild
older indexes to capture native file types; missing or invalid cached facts are
reported as skipped. Content conditions do not read these special objects.
Use `types:regular,symlink` or `types:fifo,socket` to accept any listed type.
Lists support all existing `type:` values and aliases, including categories such
as `images` and `folders`. Each list permits 1–64 values; quote the value if it
contains spaces around commas. A known matching alternative is sufficient even
when another alternative's facts are unavailable in an old index. Otherwise
unresolved type facts remain skipped. Repeated `types:` lists and ordinary
`type:` conditions remain AND, consistent with other Commander predicates.

Use `nameexact:"report[1].txt"` to match a whole literal basename, or
`notnameexact:"report[1].txt"` to exclude that exact name. These conditions
always treat punctuation, wildcard characters and regex syntax literally,
independently of Exact/Fuzzy/Regex mode. They use the global case policy,
including smart case, and apply to files, directories and archive member names.
Ordinary `name:` keeps its existing matching behavior.

Repeat `anyname:` to search alternative names:
`anyname:invoice anyname:receipt ext:pdf` accepts either name pattern and still
requires the PDF extension. All `anyname:` fields form one OR group; other
conditions and repeated ordinary `name:` fields remain AND. Exact/Fuzzy/Regex
and the global case policy apply to every alternative. The first successful
pattern supplies the score, highlight and explanation. Names are matched in
native, indexed and archive results without reading file content. Saved queries
retain order; the group allows 1–64 nonempty patterns and 64 KiB of combined
pattern text. Quote phrases normally; commas and pipes stay part of the pattern.

Repeat `anycontent:` to search for alternatives:
`anycontent:error anycontent:warning ext:log` requires at least one of the
content patterns. All occurrences form one OR group, combined through AND
with every other condition. Repeated ordinary `content:` terms still require
all their matches. Exact/Fuzzy/Regex mode and case settings apply to each
alternative, including smart-case analysis of every member. Quote phrases
normally; commas and pipes are not additional list delimiters.

The group reads each candidate's text once and tests alternatives in declared
order. The first successful pattern supplies its score, highlight and
explanation. Existing content bounds, BOM decoding, negative completeness and
binary skipped rules apply across native/index/ZIP/TAR results. Patterns and
order persist in saved queries. A group permits 1–64 nonempty patterns with
at most 64 KiB of combined pattern text; regex compilation limits still apply.

Use `notname:draft` or `notpath:vendor/` to reject text matches in an entry's
name or its path relative to the search root. These use the same Exact/Fuzzy/
Regex mode and case setting as `name:`/`path:`, combine through AND and persist
in saved searches. Negative name/path terms filter results; use `exclude:`
when you want to prune directory traversal instead.

Use `case:sensitive` in Find to distinguish uppercase and lowercase letters,
or `case:insensitive` to state the existing default explicitly. It applies to
name/path text, positive and negative content conditions, masks, exclusions,
extensions and saved pane-filter expressions, in every text mode. For example,
`case:sensitive ext:RS` matches `.RS` but not `.rs`. Case settings persist in
saved searches; conflicting settings are rejected. Pane filtering and palette
matching retain their existing defaults outside Find.

Use `case:smart` for automatic global case selection. A literal uppercase
character in any name/path/content condition, mask, extension or saved pane
filter makes the whole query case-sensitive; otherwise literal text matches
without regard to case. Structural values such as types, dates and permission
labels do not participate. Regex conditions inspect parsed literals, so
`ready\S*`, Unicode property names, capture-group names and comments do not
mistake syntax for uppercase text. Uppercase class-range endpoints and escaped
uppercase literals do participate. A regex with no literal characters retains
case-sensitive class semantics unless another matching term supplies literals.
Inline regex case flags keep their normal precedence. `case:smart` persists in
saved queries; combining different case modes is rejected. The default remains
case-insensitive when no case mode is specified.

Control dot-hidden search entries with `hidden:include` (the default),
`hidden:exclude` or `hidden:only`. Aliases `hidden:on`/`hidden:off` mean
include/exclude. A dot-prefixed file or directory below the search root makes
that entry and its descendants hidden for this query. The explicitly chosen
root is exempt, even when its name starts with a dot. Exclude prunes hidden
native branches; Only still walks visible folders to discover hidden children.
Indexed and ZIP/TAR results follow the same path rules, including members of
hidden archive containers. These are lexical dot-name rules, independent of
the pane's visibility setting and Finder/Windows hidden attributes. Conditions
persist in saved queries; conflicting hidden modes are rejected.

In Find results, use the row's `×` button to hide an unsuitable match without
changing its file. **Show all results** restores hidden rows in the current
rank/time order. Keyboard navigation skips hidden rows; hiding survives incoming
search batches and completion sorting. Total search counts include hidden
matches, with a separate hidden count. Starting a new search resets hiding.

Use a Find result's **Tab** button to reveal it in a new background folder tab
while keeping Search and the current tab open. The cursor lands on the target
when the new tab's listing loads. Hidden targets enable hidden rows only in
that new tab. Archive-member hits reveal their containing archive. Existing
tab limits and pending-operation guards apply; refusal is shown in Search.
With the query field focused, Up/Down selects a visible result and **Cmd+Enter**
opens it in a background tab while keeping query focus. Ordinary Enter retains
its existing reveal/start-search behavior. Cmd+Enter without a selected visible
result does nothing; it never opens a hidden row or starts a new search.

A dual-pane file manager for macOS, built in Rust with [egui](https://github.com/emilk/egui).

Two side-by-side panels, keyboard-first navigation, and native macOS
integration (Quick Look, Finder tags, share sheet, APFS clone copies).

The file-manager logic lives in a UI-independent, unit-tested core; the `app`
module is a thin egui layer over it.

## Features

### Panels and navigation

- **Folder tabs in each pane**: `Cmd+T` opens a tab, `Cmd+Shift+W` closes the
  current tab, and `Cmd+Shift+[` / `]` cycle tabs. Each pane supports eight tabs;
  directory, selection, history and view settings stay independent. Open folders
  and per-tab view settings restore after restart; inactive tabs load lazily.
  **Show full folder path** in a tab's context menu replaces its short label with
  a compact path retaining the beginning and a longer identifying tail. Hover
  and accessibility expose the complete path. This per-tab choice survives
  duplication, moving, copying, closed-tab recovery and restart. Custom titles
  remain stored and return when the option is disabled.
  `Cmd+Shift+T` restores a closed tab with its runtime state; **Closed** lets you
  choose among the last eight closed tabs in that pane (current run only).
  Middle-click a tab, or use **Close this folder tab** in its context menu, to
  close it directly. Background closes preserve the live listing/preview and
  pane focus; at least one tab remains in each pane.
  **Duplicate this folder tab** copies the pointed tab's folder and view
  settings beside it in the background, including when that tab is inactive.
  The live selection, filter, preview and pane focus remain unchanged; copies
  load lazily and count toward the pane's eight-tab limit.
  **Move this folder tab to other pane** transfers the tab with its runtime
  selection, filter, view and navigation history beside the other pane's active
  tab, in the background. Keep at least one source tab and space for the moved
  tab in the destination. Moving an inactive tab preserves both current folders;
  moving the visible tab opens its neighbor in the source pane.
  A tab's context menu offers **Close other folder tabs** and **Close folder
  tabs to the left/right**. These act relative to that tab, including inactive tabs;
  closed tabs remain recoverable through the same bounded history.
  **Copy this folder path** copies the pointed tab's folder without activating
  it or changing the current selection. Control characters use the same
  single-line escaping as other copied paths.
  A tab's context menu also edits its **Tab title** (up to 64 characters).
  **Use folder name** restores the automatic label. Titles follow the tab through
  navigation, duplication, copying, moving and closed-tab recovery, and are saved
  between sessions. The full folder path remains in the tooltip.
  **Protect from closing** marks a tab with `*` and blocks direct/middle-click
  closing. Close groups and duplicate cleanup keep protected tabs. Navigation
  remains available. Protection is saved and follows duplication/copy/move;
  turn it off in the tab menu before closing the tab. The command palette offers
  **Protect folder tab from closing** and **Unprotect folder tab** for the current
  tab, with availability reflecting its protection state.
  Duplicate cleanup keeps every protected tab. Otherwise it prefers a custom
  title, then the active tab among ties, then the first. If the active copy is
  removed, its surviving peer becomes visible and the removed runtime tab can
  be reopened from Closed history.
  **First folder tab** and **Last folder tab** in the command palette jump to
  either end of the current pane's tab strip and restore that tab's state.
  Each command is unavailable when its destination is already active.
  **Go to folder tab 1–8** selects a numbered tab directly from the palette;
  missing tabs and the current tab are unavailable. Cmd+number keeps its
  existing saved-folder shortcut behavior.
  The palette also offers **Close other folder tabs**, **Close folder tabs to
  the left** and **Close folder tabs to the right**, relative to the current tab.
  Protected tabs survive; groups with no eligible tabs are unavailable in both
  the palette and tab menu. Closed tabs retain their state in Closed history.
  **Open highlighted folder in background tab** is available in the command
  palette and by right-clicking **+**. It inserts beside the current tab without
  switching away; contents load when the new tab is activated.
  **Copy all folder tabs to other pane** (palette or a tab's context menu)
  copies the whole group and its view settings beside the other pane's current
  tab. Both current folders stay open; the group must fit within eight tabs.

- **List and Icons views** per pane, with bounded image thumbnails in the active
  grid, the same selection/drag/context-menu operations, and remembered view
  mode. Choose a mode in the pane header, View menu, or command palette.
- **Keyboard reference** (`F1`) lists catalog shortcuts with unavailable reasons.

- **Dual panels** with an active-panel marker (`Tab` switches sides), a
  breadcrumb path bar, and a folder tree sidebar with a **Favorites rail**.
- **Bookmarks** with quick-jump slots: `Cmd+1`..`9` jump the active panel to a
  pinned folder, `Cmd+Shift+1`..`9` assign one.
  Right-click a Favorites row to edit its display name and **Save favorite name**,
  or assign a quick-jump slot directly. Labels are trimmed, limited to 256
  characters and reject control characters. Renaming keeps the folder path,
  row order and slot; assigning an occupied slot transfers it to that bookmark.
  The same menu opens a favorite in the **other panel**, a **new tab**, or a
  **background tab**. Middle-click opens a background tab while keeping the
  current pane and tab selected. Tab capacity and pending-operation guards
  apply; opening a favorite does not change its stored name or quick-jump slot.
  **Open favorite folder** (palette), or an `@` query in the omnibar, searches
  all Favorites by fuzzy label/path, prioritizing labels. Choose **Here**,
  **Other panel**, **New tab** or **Background**, then use arrows/Enter or click.
  The opening snapshot shows at most 100 ranked results with a total count;
  queries are limited to 128 characters. Removed bookmarks are rechecked before opening.
  **Sort all Favorites** in a row's context menu, or the three **Sort favorites**
  palette commands, reorder the saved list by natural case-insensitive name,
  path, or assigned quick-jump slot. Equal keys retain their relative order;
  unassigned entries follow slots 1–9. This is a one-time reorder, and manual
  moves/additions continue to work afterward. A failed save keeps the prior order.
  **Add selected folders to Favorites** (palette) appends selected visible
  directories from the active pane in display order, skipping files and existing
  Favorites. Existing labels/slots remain intact; new entries have no slot.
  The whole batch is saved once before the live list changes. Hidden, filtered
  and other-folder selections are excluded, and an empty selection never uses
  the highlighted row implicitly.
  **Import favorites from GTK text** (palette) reviews pasted local `file://`
  bookmark records before appending selected new paths. Existing entries/slots
  win; duplicates, invalid and remote records are marked. Import does not probe
  or create folders. **Copy favorites as GTK text** exports current Favorites;
  quick-jump slots stay Commander-only. Both text paths are limited to 64 KiB,
  with at most 1000 input lines and a bounded, scrollable review window.
- **Per-pane history**: `Cmd+[` / `Cmd+]` walk back and forward through the
  directories you visited (a vim-style jump trail).
- **Go to path** (`Cmd+L`), **recent folders** (`Cmd+P`, last 200 visited
  folders ranked by frecency or chronology), type-ahead jump, and a per-panel
  filter box with quick-filter
  facets (kind, size, and date buckets: Today / Week / Month, plus an
  Older-than-a-month bucket).
- **Command palette** (`Cmd+K`): fuzzy-filter every command, ranked by recency
  and frequency. Availability comes from the same workspace policy as both
  toolbar layouts; unavailable matches stay visible with a reason, and Enter
  selects the first action that can actually run. Direct shortcuts use the
  same typed predicates and explain a refused action instead of disappearing.
- **Streaming search** with cancellable generations, stable result identity,
  one fielded query grammar, Exact/Fuzzy/Regex modes, replayable history,
  optional inspectable content indexes, and bounded ZIP-member search.
- **Project collections** combine multiple roots into non-owning virtual views;
  the compressed disk tree stays cancellable and bounded on large roots.
- **Per-folder view memory**: sort, filters, hidden, and density are
  remembered per directory for the running session (not persisted across
  restarts) and restored when you navigate back.
- **Vim-style chords**, modifier-free: `j` / `k` move the cursor down/up
  (with a leading count, e.g. `5j`, picked up from the type-ahead buffer),
  `g g` jumps to the top, and `s s` reverses the sort.

### File operations

- **Batch rename presets** (`Cmd+Shift+R`): save up to 32 named rules, reload
  them into the live preview, explicitly update or delete saved rules. Presets
  persist with session settings; loading one never applies a rename by itself.
  **Reset rule** clears current edits and restores initial counter controls,
  detaches the selected preset and rebuilds an unchanged-name preview. Saved
  presets and the target-file list are retained.
  Find/replace can target the whole name, its stem, or just the last extension;
  the selected scope is saved with each rule. Extensions exclude their dot.
  **Ignore case** and **First match only** work with both literal and regex
  replacement and are remembered in presets. Literal replacement keeps `$1`
  as text; regex replacement supports capture references.
  **Add replacement** appends up to 16 additional Find/Replace pairs, applied
  in order after the primary pair and before case/numbering. They share the
  selected target and matching options; each pair sees the previous result.
  Preview rows also have Up/Down controls to reorder files before numbering.
  Moving a file preserves the batch membership and recalculates its counter;
  this order belongs to the current batch and is not stored in rule presets.
  Exclude removes a preview row from this batch without deleting its file and
  recalculates numbering. Excluded names still participate in collision checks.
  Excluding the last target closes the empty batch without applying changes.
  Copy original names and Copy proposed names export the current row order;
  proposed names require a valid conflict-free plan. Text lists escape line
  controls. Copy mapping (CSV) retains raw quoted names and validation statuses,
  including invalid plans, for review without applying changes.
  **Character edits** selects Name stem (default), Extension, or Whole name for
  positional range edits/insertion, independently of the find/replace target.
  Extension excludes its dot and skips folders; deleting it completely removes
  the separator, while inserting into an empty extension creates one. Whole name
  can change the extension boundary. Extension case applies after these edits.
  **Range** with **Remove** deletes a range from the chosen component after replacements;
  **Keep** retains only that range. Both precede insertion, case, prefix/suffix and
  numbering. Skip counts from the start, or from the end when enabled; Count sets
  the range length. For example, Keep with Skip 4 / Count 8 turns
  `IMG_20261030_raw.jpg` into `20261030.jpg` in Name stem mode. Short names clamp safely,
  Unicode grapheme clusters are kept whole. Presets
  remember the range; Reset disables it.
  **Insert text** inserts literal text at a grapheme position in the chosen component after
  range removal. **From end** counts the anchor backwards; **Overwrite** replaces
  following graphemes equal to the inserted text length and appends any remainder.
  Positions clamp to the component and presets remember the
  text and options. Reset disables insertion.
  **Counter format** offers Decimal, lowercase letters and uppercase letters.
  Letters use `1=a`, `26=z`, `27=aa` (or uppercase); zero stays `0` and negative
  values get a minus sign. Start, step and counter position work in every format.
  Padding applies only to Decimal and is retained when switching formats. Presets
  remember the format; older rules and Reset use Decimal.
  Up/Down beside replacement pairs immediately refresh the preview. Remove deletes a
  pair; presets retain the chosen order and Reset clears the chain.
  Extra pairs need nonempty Find values and permit 64 KiB of combined text.
  Each intermediate replacement result is limited to 64 KiB; exceeding it blocks
  the whole batch before any names change. Regex capture interpolation follows
  the regex engine's normal rules; literal replacements keep dollar signs literal.
  Case modes include **First capital** and **Title Words** (Unicode word
  boundaries). Independent **Extension case** options Keep / lower / UPPER
  change only the resulting last extension after find/replace; Keep is the
  default. Prefixes and suffixes are added after stem case conversion.
  These modes are saved in presets too. Numbering accepts negative starts and
  steps for descending sequences across zero; a zero step repeats one number.
  Padding includes the minus sign. Signed numbering is saved in presets.
  **Counter position** places the number before or after the decorated name
  stem, retaining the final extension. After name remains the default for old
  rules; the position is saved in presets. Prefix text can provide a separator
  after a leading counter, for example `03_Photo.jpg`.
  **Counter only** replaces the decorated stem with the number while retaining
  the resulting extension (`Photo.JPG` → `001.JPG`). This mode is saved in
  presets and has no effect while numbering is disabled.
  **Trim spaces** removes Unicode whitespace at both ends of the final name,
  after other transformations. Internal spaces remain intact. This option is
  saved in presets; empty names and collisions still block renaming.
  **Collapse repeats** reduces consecutive identical punctuation, whitespace or
  symbol graphemes to one (`report___final--v22.txt` → `report_final-v22.txt`).
  Letters, digits and different adjacent separators remain intact. It runs on
  the final name before Trim spaces and is saved in presets.
  **Unicode** offers Keep / NFC / NFD / NFKC / NFKD normalization of the final
  complete name. Keep is the default. Compatibility forms also replace characters
  such as ① with 1; resulting invalid names and collisions block renaming.
  The selected form is saved in presets. A filesystem may enforce its own form.
  **Remove prefix / Remove suffix** remove one exact, case-sensitive match from
  the original complete name, before find/replace and extension splitting
  (`backup_photo.jpg.old` → `photo.jpg`). Empty fields do nothing. Both are
  saved in presets and cleared by Reset rule.
  **Limit stem** keeps up to 1–255 complete Unicode graphemes in the final stem,
  retaining the last extension. It runs last, including after decorations and
  normalization; folders use their whole name. The option defaults off, saves
  in presets and resets to an inactive limit of 32. Filesystem byte-length and
  collision checks still apply.

- **Copy / move / delete** on a background thread with a live progress window
  (speed graph, ETA, per-file error list). Copies use native `copyfile` with
  APFS cloning and fall back to a buffered copy; a same-volume move is an
  instant atomic rename.
- **Durable operations**: Fast/Verified/Versioned profiles, typed failures,
  source/destination revalidation, an idempotent operation journal, safe-state
  lockout, and a Recovery Center with Resume, Roll back, Inspect, and repair
  plans. Versioned operations expose Compact/Recent/Archive/Forever retention;
  pruning publishes the retained manifest before deleting expired copies.
- **Adaptive transfers**: resumable buffered and delta checkpoints, fixed or
  measured-threshold FastCDC delta copy, sparse-file preservation, explicit
  clone/rename/delta/buffered telemetry, per-volume concurrency, bandwidth and
  quiet-hour rules, and a generation-aware verified BLAKE3 cache.
- **Transfer queue**: firing a second operation while one runs queues it
  instead of dropping it (F5/F6 stay live during an active transfer just for
  this). The **queue panel** (palette: "Transfer queue") lists every waiting
  job with pause / resume / reorder / cancel; concurrency stays capped at one
  transfer at a time (the progress UI is built around a single active job).
- **Operation history** (palette: "Operation history"): a searchable log of
  completed moves, deletes, and batch renames, each with a jump-back button
  and, while it is still the exact top of the undo stack, a live Undo button.
  Session-lifetime, not persisted across restarts.
- **Safe overwrites**: the confirmation dialog offers Overwrite All, **Keep
  Both**, and Skip. Overwrites stage the new copy and swap it into place, so an
  interrupted copy never destroys the existing file. Copying or moving a path
  into itself is rejected, and a **free-space preflight** warns before a copy
  that will not fit (a clone or same-volume move needs ~0 extra space).
  Visible actions also consume the shared volume capability matrix: Copy needs
  a writable destination, Move needs both panes writable, and local mutations
  are disabled on a read-only active volume with the exact reason shown.
- **Drag and drop** between panels and onto subfolders, routed through the same
  engine as the keyboard. `Cmd+Enter` moves into the highlighted folder and
  `Cmd+Shift+Enter` copies into it; both actions are also available from the
  toolbar overflow menu for single-pointer use.
- **Archive browse / extract**: open ZIP, TAR, tar.gz, tar.bz2, tar.xz and tar.zst
  (including `.tgz`, `.tbz`/`.tbz2`, `.txz`, `.tzst` and `.tar.zstd`) as a
  member browser, multi-select entries, and extract into the other panel.
  Archive **Copy visible / Copy selected** also supports **Names without
  extension** (remove the final dot suffix lexically, including directory names)
  and **Parent folders** (distinct internal folders in displayed order; `.` is
  the archive root). Hidden selected members are included by Copy selected.
  Scroll the archive pane to reach its selection and extraction actions in
  shorter windows; the member list also has its own bounded scroll area.
  Switch between All members and Folders, navigate with breadcrumbs/Up, and
  select a listed folder subtree without losing selections in other folders.
  **Invert visible / Clear visible** toggle or remove only the currently shown
  real members, retaining selections hidden by the filter or another folder.
  Synthetic navigation folders are excluded; **Clear** resets all selections.
  **Shift+click** and **Shift+Up/Down** in the archive filter add a range in
  displayed order. Hidden selections stay selected; navigation folders are
  skipped. A normal click or arrow move sets a new range anchor.
  With the archive filter focused, **PageUp / PageDown** moves the member cursor
  by one list viewport; **Shift+PageUp / Shift+PageDown** adds the traversed
  range to the selection. Page moves stop at the first/last visible row.
  **Save selection / Restore selection** keep one snapshot for the current
  loaded archive, including hidden members. Restore replaces the current set;
  saving an empty set is supported. Closing or reloading the archive resets it.
  **Same extension** selects or deselects visible regular files matching the
  focused file's final suffix without case. Hidden choices remain intact;
  directories, links and synthetic navigation folders are excluded. Files
  without a suffix match one another, including dotfiles such as `.profile`.
  **Same name** selects/deselects visible regular files sharing the focused
  file's name without its final extension, ignoring case. **Same folder** uses
  the exact internal parent path of the focused real member, including visible
  directory/link records. Both keep hidden selections, skip navigation rows and
  do not expand directory subtrees.
  Expand **Select by mask** to add/remove visible matches with an independent
  glob mask, case setting and filename/full-path scope. Empty/invalid masks
  change nothing. This changes selection, keeps hidden choices and saved
  selection memory, and leaves the current view filter alone. Mask settings
  last for this loaded archive; they are separate from saved/reset view filters.
  **Narrow to selection** shows a snapshot of selected members in All members,
  keeping the path filter. Deselecting a row keeps it visible for review;
  narrowing again updates the snapshot. **Remove narrowing** restores the list
  without changing selection. Narrowing an empty selection also removes it.
  **Types** combines regular files, directories, symbolic links and other
  archive records. All types are shown by default. Navigation folders remain
  available for browsing; hidden selected members remain selected.
  **Types → Executable files only** additionally requires a regular member with
  known declared executable status. Unknown status, directories, links and other
  records are excluded. It intersects the type choices; navigation folders stay
  available. Saved filters retain it; Show all types and Reset filters clear it.
  **Save filter / Saved filters** keeps up to 20 query/settings snapshots for
  this loaded archive, with a 16 KiB combined filter-text limit. Recall restores the query,
  Regex/case/exclusion and type settings in the current folder and narrowing,
  preserving sorting and selection. Invalid patterns cannot be saved. Reset
  filters retains saved entries; Clear all saved filters removes the saved snapshots.
  Choose **Remove** beside a saved filter to remove that
  snapshot alone; the active query and filters stay as they are.
  **Search saved filters** narrows the menu by its displayed descriptions,
  ignoring case. This menu search does not change the active archive filter.
  **Clear search** reveals all entries; **Clear all saved filters** removes
  all snapshots, including those hidden by the menu search.
  Closing or reloading the archive discards them.
  **File size** filters declared unpacked regular-file sizes using fd-style
  expressions: `0b` exact, `+1mi` minimum, `-10mi` maximum, or `+1mi -10mi`
  for an inclusive range. Decimal units are `k/m/g/t`; binary units are
  `ki/mi/gi/ti`. Blank disables it. Invalid input shows no rows; navigation
  folders remain available for valid constraints. Size settings are included
  in saved filters and cleared by Reset filters. Hidden selections stay selected.
  **Depth** selects an exact archive-root depth (`2`), minimum (`2..`), maximum
  (`..3`) or inclusive range (`2..4`). Root-level members are depth 1; navigating
  folders does not change that basis. Valid constraints filter real members while
  keeping navigation folders available; invalid input hides all rows with an error.
  Saved filters retain depth, Reset clears it, and hidden selections stay selected.
  **Modified (UTC)** filters declared modification dates: `2026-01-01` matches
  that UTC day; `2026-01-01..` includes dates since its start; `..2026-02-01`
  stops before that day. A range combines both bounds. Unknown timestamps are
  excluded only while active. Saved filters retain dates; Reset clears them.
  **Extensions** accepts alternatives separated by spaces: `pdf jpg tar.gz`.
  Matching uses literal, case-insensitive filename suffixes, with optional leading
  dots; multi-part extensions and dotfiles are supported. It applies to real
  files, folders and links, intersecting the type and other filters. Navigation
  folders stay available. Blank disables it; invalid input shows no rows.
  Saved filters retain extensions; Reset clears them and preserves selections.
  **Exclude glob** independently removes matching names and directory descendants
  while the main query keeps matching desired entries. For example, use `report`
  in the main query and `{*.bak,*.tmp}` here. Matching ignores case; **Full paths**
  compares internal path prefixes. Blank disables it; invalid masks show no rows.
  Saved filters retain the mask and scope; Reset clears both. Selected IDs stay selected.
  **Hide dotfiles** hides members and navigation folders with a dot-prefixed
  component anywhere in their internal path, including hidden-folder descendants.
  It uses archive names, not host attributes or link targets. Saved filters retain
  the setting; Reset clears it. Existing selections remain selected when hidden.
  **Filename only** matches the final member or navigation-folder name instead
  of its full internal path, for both literal and Regex filters. For example,
  `^main\.rs$` matches `src/main.rs` in this mode. Exclusion uses the same scope.
  Full-path matching remains the default; saved filters retain the scope.
  **Exact** compares the entire selected path/name literally, including leading
  and trailing spaces. It treats regex symbols as ordinary characters and is
  mutually exclusive with Regex. An empty query still disables path filtering.
  Exact mode follows case settings and is retained by saved filters.
  **Glob** supports whole-name/path masks (`*`, `?`, `[ab]`, `{rs,txt}`), with
  `**/` for recursive path components and backslash escaping. In full paths,
  `*` stays within one component: use `**/*.rs` or Filename only for all depths.
  Wildcards/classes use globset's byte-oriented semantics (`?` is one UTF-8
  byte); Regex supports Unicode character matching. Glob, Exact and Regex are
  mutually exclusive. Glob participates in saved filters, exclusion and Reset.
  A selection summary counts files, directories, links and other records across
  the whole archive, including hidden choices. Exact unpacked byte totals cover
  regular files only; ZIP also shows their packed bytes. These are declared
  metadata totals, not verified payload sizes or a disk-space estimate.
  Expand **Member details** under the current member's path to inspect its kind,
  declared size, ZIP packed size, UTC modification time, octal Unix permissions
  and executable flag. Missing metadata is shown as unavailable; known zero
  values remain explicit. This view does not change extraction permissions.
  **Back / Forward** revisit folders and All members inside the current archive,
  restoring the cursor when it remains visible. Current filters, sorting and
  selections stay active. History is session-only, capped at 64 saved locations
  and 256 KiB of encoded folder paths; a new navigation clears Forward.
  Expand **Go to folder** to enter an exact internal directory path from the
  archive root, such as `src/nested`. Empty or `/` opens root; `..` is rejected.
  Go/Enter uses the loaded folder index and participates in Back/Forward,
  keeping filters and selection. Paths are case-sensitive and capped at 4096
  characters; this does not open directories on the host filesystem.
  **Suggestions** lists up to 12 indexed folders matching the typed prefix;
  selecting one navigates directly. A trailing `/` limits matches to descendants.
  Matching is case-sensitive, and an overflow hint asks for a longer prefix.
  Sort archive rows by name, extension, size, packed size, compression ratio or date;
  Extension compares the last filename suffix without case; directories,
  extensionless names and dotfiles such as `.profile` use an empty suffix.
  reverse the order or put folders first without losing the selection.
  **Natural names** sorts file2 before file10 (including navigation folders);
  turn it off for ordinary name ordering.
  Archive filters default to a case-insensitive literal substring of the full
  internal path. **Regex**, **Exact**, **Glob** and **Fuzzy** are exclusive modes;
  **Match case** and **Smart case** control case matching. Fuzzy matches letters
  in order with gaps, trims outer spaces and keeps inner spaces literal. It
  preserves the selected sort and is included in saved filters and Reset.
  Uppercase query characters enable Smart case, while manual
  Match case takes priority. Regex keeps whitespace meaningful and accepts inline
  flags. Patterns are capped at 256 characters and use bounded compilation.
  Invalid patterns show an error and no rows; saved selections remain intact.
  **Exclude matches** hides paths matched by the current path/name filter,
  including navigation folders. An empty pattern matches all paths, so excluding
  it hides every row. Invalid patterns still show an error and no rows.
  **Reset filters** clears the query, matching/case/exclusion options, type filters
  and narrowing together. It preserves the current folder, sort, selection,
  saved selection and navigation history, and returns focus to the query field.
  **Copy visible / Copy selected** exports listed member names, internal paths
  or CSV metadata without extracting files. Visible rows follow the displayed
  order and omit synthetic navigation folders; selected rows include choices
  outside the current folder/filter and follow the current sort. CSV includes
  archive path, member index, kind, byte sizes, UTC time and truncation status;
  duplicate member paths remain separate. Output is capped at 4 MiB. Paths
  outside UTF-8 report an error instead of copying a lossy name.
  Text search also visits bounded plain/compressed TAR members. Decoded TAR
  inspection is limited to 128 MiB, including skipped payloads; reaching the
  limit reports partial results. New compression formats use a private temporary
  TAR, capped at 128 MiB, for the existing system-tar extraction path; it is
  removed on success, error or cancellation. Automatic archive
  inspection in search consults trust-gate hooks (J003).
  **Test archive** runs a read-only, cancellable check before extraction: ZIP
  member CRC checks, or TAR headers and the complete compressed stream/trailers.
  It checks all members, independent of selection/filter. Limits: 100,000
  members and 128 MiB decoded data (also bounding TAR logical payloads).
  Cancelled, unsupported, damaged or limited checks never report completion.
  Ordinary TAR has no payload checksum; this check cannot detect every changed
  TAR payload or establish authenticity. The last result is not a live monitor.
- **View as hex** (palette): inspect binary files in 4 KiB pages with absolute
  offsets, hexadecimal bytes and a character column (Unicode BOM or ASCII).
  **Characters** also offers Windows-1251/1252, KOI8-R/U and IBM866. The chosen
  column is included in Copy page; search encoding and raw-byte copying are
  independent. Controls, undefined and invisible spacing bytes appear as dots.
  **UTF-8** shows each character at its leading byte and `›` at continuation
  bytes, including sequences crossing rows or page boundaries. Invalid UTF-8
  uses dots. Copy page preserves that byte-position representation.
  **UTF-16 LE / BE** uses explicit byte order and code units aligned to even
  absolute file offsets, including surrogate pairs and odd-offset jumps.
  Remaining bytes of a valid character show `›`; malformed units use dots.
  **F8** cycles the supported character encodings; **Shift+F8** opens their
  menu. These display-only actions work during reads and preserve query text.
  Open menus suppress switching; viewer F8 keys never reach pane deletion.
  **Auto (Unicode BOM)** selects UTF-8/UTF-16 LE/BE from a signature at byte
  zero of the accepted snapshot, otherwise ASCII. Manual choices override Auto
  and survive Reload. Auto can be restored without rereading; UTF-32 and
  heuristic encoding detection are not supported. BOM bytes are kept visible.
  Previous/Next, First/Last, decimal/hex
  Go to offset, Reload and Copy page work without editing the file.
  **PageUp/PageDown** loads the previous/next 4 KiB block; **Home/End** or
  **Ctrl+Home/End** loads the first/last block. Focused widgets, open menus and
  active work keep ownership of these keys. Repeating a boundary key does not reread it.
  **Bytes per row** selects 1–64 bytes, with quick presets 8/16/32 (default 16).
  With no widget holding keyboard focus, **Alt+Left/Right** changes width by one;
  **Ctrl+Alt+Left/Right** moves to the next lower/higher multiple of 16, bounded
  to 1–64. Busy work and open menus suppress these shortcuts.
  Copy page uses the same layout and byte radix. Changing width uses the loaded
  snapshot, retains absolute byte offsets and survives Reload in this viewer.
  Status also shows the loaded page's start as a percentage of the snapshot
  file size, rounded down to 0.01% with integer arithmetic. EOF is 100.00%; an
  empty file shows n/a. This is position, not background read/search progress.
  **F5** focuses Go to offset and **F7** focuses Search, scrolling the field
  into view. These focus actions preserve input and do not start work. While
  Hex preview is open, unmodified F5/F7 belong to its fields; active reads/searches
  and open menus suppress focus changes. Modified function keys keep their existing routes.
  **Esc / F10** closes Hex preview, cancels its active work and requests focus
  on the current file-panel row. An open menu receives the key first.
  **Ctrl+Left / Ctrl+Right** shifts the page start by one byte, retaining bytes
  per row and recording the new position in history. This requires no focused
  widget, open menu or active work; the start cannot move outside the file.
  **Copy match** exports only the found bytes as HEX, decimal or exact UTF-8
  text from the loaded snapshot. Invalid UTF-8 disables the text action;
  copying does not reread the file or include surrounding page bytes.
  **Clear match** in that menu, or **Ctrl+U** with no focused widget or open
  menu, removes the search highlight and disables match copying. It keeps
  the query, loaded page, bookmarks and next-search position.
  **Ctrl+C / Cmd+C / Ctrl+Insert** copies the selected match as HEX when no
  field, menu or manually selected label text owns copying. Busy or cleared
  matches do not copy; the menu remains available for decimal and UTF-8.
  **Shift+F7 / Space** repeats search forward and **Alt+F7** repeats backward
  when no widget or open menu owns keyboard input. These use the current
  query, encoding and Wrap setting; active work suppresses repeats.
  Enter a whole percentage such as `50%` to jump within the file (`100%` opens
  EOF); percentages use the loaded file size and round down to a byte.
  Prefix **+ / -** for a relative step from the loaded page's first byte:
  `+64`, `-0x10` or `+10%`. Signed percentages use the full file size for the
  step. Overflow and destinations outside the file are rejected before reading;
  the current page stays loaded. Unsigned offsets remain absolute.
  Explicit radix forms also accept `$FF` or `FFh` for hexadecimal and `255m`
  for decimal, including relative forms such as `-$10` and `+16m`. The `m`
  suffix means decimal notation, not megabytes; percentages remain decimal.
  **Back / Forward** revisit successfully loaded positions, including Go,
  page/bookmark jumps and search hits. History holds 64 saved offsets; a new
  position after Back clears Forward. Failed/cancelled reads do not advance it.
  **Ctrl+Z / Alt+Backspace** goes Back; **Ctrl+Shift+Z** goes Forward.
  These shortcuts leave focused widgets, open menus and active work alone.
  Reload starts a new history; search settings and bookmarks stay current.
  Ten numbered session bookmarks save exact page positions: choose a slot,
  **Set here**, **Jump**, or **Clear**. Reload clears all bookmarks.
  **Ctrl+Shift+1–9** saves slots 1–9; **Ctrl+Shift+0** saves slot 10.
  **Ctrl+1–9 / Ctrl+0** jumps to those slots using the guarded page reader.
  These shortcuts are suppressed during work or while a menu is open.
  Switch **HEX / Decimal** to display byte values in base 16 or 10; Copy page
  uses the selected representation. Row offsets remain hexadecimal and byte
  search still takes hexadecimal input. A new viewer defaults to HEX.
  **Copy bytes** exports the loaded page as space-separated HEX or decimal
  bytes without offsets/ASCII columns, or as exact UTF-8 text when valid.
  Invalid UTF-8 disables text copy; no lossy replacement is used. It copies
  the loaded snapshot without rereading the file, up to the 4 KiB page limit.
  Find next/previous search exact hexadecimal byte sequences or **UTF-8 text**
  throughout the file, across page boundaries, with progress and cancellation.
  **Find first / Find last** start from the beginning/end of the file regardless
  of the current page or previous match. Next/previous then continue from that hit.
  With the search field focused, **F3 / Enter** finds next and
  **Shift+F3 / Shift+Enter** finds previous, retaining focus for repeated searches.
  Shortcuts do not queue additional work while a read/search is running.
  **Recent searches** recalls up to 20 submitted queries together with encoding
  and Wrap, without starting a search. Exact duplicates move to the front;
  history text is capped at 16 KiB. The history stays in this viewer session
  and can be cleared separately from the current query.
  **Recent offsets** recalls up to 20 submitted Go expressions without reading
  a page. Press Go to evaluate the recalled expression; relative expressions
  use the current page position. Invalid or busy submissions are excluded.
  This history survives Reload, stays in the open viewer, and can be cleared.
  In either recent-history menu, **Remove** beside an entry deletes only that
  entry without recalling it or changing the page/input. Search entries include
  their encoding and Wrap setting; offset entries use their exact expression.
  UTF-8 search preserves spaces and case, uses at most 256 encoded bytes, and
  also works inside binary files without decoding their entire contents.
  **UTF-16 LE / BE** searches text in the selected byte order, including emoji;
  no BOM is added. The same 256-byte encoded limit applies, and matches may
  begin at any byte offset inside a binary file.
  **Windows-1251 / Windows-1252** searches exact encoded bytes in those codepages,
  preserving spaces and case. Unrepresentable characters fail before a worker
  starts; no replacement bytes, BOM or normalization are introduced. The same
  256-byte cap and forward/backward/wrap search apply.
  **More encodings** adds exact **KOI8-R**, **KOI8-U** and **IBM866** search for
  legacy Cyrillic files, with the same strict encoding and 256-byte limit.
  The menu shows the selected encoding; changing it clears stale match state
  while preserving the query and loaded page.
  **Mixed text/hex** combines double-quoted UTF-8 literals and hexadecimal byte
  constants, such as `"Header" 00 FF`. Constants may use `0x` and must fit one
  byte; tokens require whitespace separators. Quoted literals support `\"`,
  `\\`, `\n`, `\r`, `\t` and `\0`. Input is capped at 1024 characters and the
  encoded pattern at 256 bytes; malformed or incomplete patterns fail before
  searching.
  Optional **Wrap search** continues from the other end after a boundary and
  reports a wrapped match. It defaults off and applies to both directions and
  every input mode. Each candidate address is checked at most once per search;
  matches never join the last bytes of the file to its first bytes.
- **Combined name filters**: `.rs$ && !^test` requires both conditions.
  `&&` binds before `|`, so `.rs$ && !^test | .toml$` also accepts any `.toml`
  ending. Operators must stand alone; ordinary spaces retain fuzzy matching.
  Double quotes protect operators inside literal terms. Empty/incomplete terms
  show no results; positive highlight ranges are merged without overlaps.
- **Quoted filter alternatives**: `"a | b" | .rs$` matches literal text `a | b`
  or a Rust-file ending. Double quotes surround one literal alternative, keeping
  its spaces and operators unchanged. Unclosed or malformed quoted alternatives
  show no results. The leading-apostrophe whole-input form keeps its behavior.
- **Alternative name filters**: `.rs$ | .toml$` shows either ending. A standalone
  `|` separates fuzzy, literal, anchored or excluding alternatives; surrounding
  whitespace is ignored. Empty alternatives show no matches. A leading apostrophe
  makes the whole input literal, including `|`; embedded `x|y` stays ordinary
  fuzzy text. Highlights use the first matching alternative. Saved pane filters
  retain the expression.
- **Excluding name filters**: `!backup` hides names containing `backup`,
  `!^temp` hides names starting with `temp`, and `!.tmp$` hides names ending in
  `.tmp`. Comparisons are literal and case-insensitive. A leading apostrophe
  searches for an actual `!`. Excluded selections stay selected; saved pane
  filters retain the rule. A lone `!` excludes every name.
- **Anchored name filters**: `^report` matches the beginning of a name, `.rs$`
  matches its end, and `^README.md$` matches a whole name. The text between
  markers is literal and case-insensitive, not a regular expression. Use a
  leading apostrophe to search for actual `^` or `$` characters. Highlights and
  saved-search conversion retain the selected boundaries.
- **Literal name filter**: start a pane's filter with `'` to match uninterrupted
  text instead of fuzzy letters: `'ab` matches `ab.txt`, not `a-b.txt`. Matching
  stays case-insensitive. All text after the marker is literal, including spaces
  and `*`; use `''` to find an apostrophe. A lone marker matches everything.
  Highlighting and newly saved pane filters use the same rule, including Unicode
  comparison. New saved filters retain fuzzy, literal and anchored behavior using
  a `filter:` query term; existing `name:` query terms keep their meaning.
  Ordinary fuzzy filters keep their behavior.
- **Clear selection in this folder** (palette): removes this folder's picks,
  including filtered-out entries, while retaining selections in other folders
  and the saved selection-memory buffer. The command is unavailable when this
  folder has no picks, even if another folder does.
- **Clear visible selection** (palette): removes picks only from the current
  filtered view, retaining filtered-out picks and choices in other folders.
  **Toggle current selection in place** changes the cursor entry's selection
  without advancing the cursor; Space retains its existing move-down behavior.
- **Go to next / previous selected entry** (palette): moves the cursor among
  visible selected files and folders in current display order, without changing
  picks. Navigation stops at either end and skips filtered-out selections. It
  reveals the destination, retains the last repeatable command and is unavailable
  when no selected entry remains in that direction.
- **Open other pane’s folder here** (palette): navigates the focused pane to
  the other pane's current folder while retaining focus. This complements
  **Equalize panels** (Cmd+E), which opens this pane's folder in the other pane.
  Both retain the destination's Back/Forward history, skip an already-matching
  folder and reject navigation while a dialog owns the current operation.
- **Focus left pane / Focus right pane** (palette): activates a specific pane
  without changing its tabs, filters or selection. The already-focused pane's
  action is unavailable. Pending dialogs block execution to retain their owner;
  these navigation actions preserve the last command available for repetition.
- **Deselect visible files / Deselect visible folders** (palette): subtracts
  just that kind from the current filtered selection. Other kinds, filtered-out
  picks and selections remembered in other folders stay selected. Each command
  is unavailable when that kind has no selected visible entries.
- **Copy current folder path / Copy current folder as file URL** (palette):
  copies the focused pane's location even in an empty or filtered folder,
  independently of selected files. The URL form percent-encodes spaces and
  special characters; copying retains the current selection and filter.
- **Sort: folders by name first (toggle)** (palette): keeps folders first in
  ascending name order while files follow the chosen column and direction.
  Natural name ordering and case settings apply to folders too. Enabling it
  turns on folders-first; turning folders-first off clears this option. The
  option is saved with pane/tab views and is off by default.
- **Sort: case-sensitive names (toggle)** (palette): uses original Unicode names
  for name/stem sorting and name tie-breaks; ASCII uppercase sorts before
  lowercase. Natural numeric order still applies when enabled. This mode keeps
  accents and does not use locale collation. It is off by default and saved in
  each pane/tab's view settings.
- **Sort by name without extension** (palette): compares file names before their
  final extension, so `a.txt` precedes `a-z.txt`. Directory names keep their dots.
  Natural name ordering and folders-first still apply; equal stems use full
  names in ascending order. Run again to reverse direction. The column heading
  identifies this mode, and tab/session view settings retain it.
- **Selection memory** (palette): **Save selection in memory** remembers this
  folder's explicit picks, including filtered-out picks; **Restore saved
  selection** replaces this folder's selection with the saved paths still in
  its complete listing. Picks in other folders stay intact. Each tab has one
  in-memory buffer, bound to its saved folder; return to that folder to restore
  it. Saving an empty selection deliberately replaces the buffer. The buffer
  follows closed-tab recovery and moves, but is not saved across app restarts
  or copied when duplicating a tab.
- **CSV/TSV table previews**: optionally treat the first record as headings,
  kept above the scrolling rows. Click a column to cycle natural, case-sensitive
  ascending / descending / original order, or choose **Original order**.
  Row numbers retain their source record numbers; equal values keep source order.
  Sorting covers only the bounded preview (500 records, 64 columns, 256 KiB).
  Each pane keeps its own view until the loaded content changes; Source is unchanged.
  **Filter rows** searches within any single cell using a literal substring or
  **Fuzzy** characters in order with gaps. **Match case** and **Smart case** apply
  to filtering; column sorting keeps its own case-sensitive order. Headings stay
  visible, and **Clear filter** restores the sorted rows and focuses the input.
  Queries are limited to 256 characters; outer spaces are trimmed, inner spaces
  are literal. Filtering searches only loaded preview records.
  **Search in** limits matching to a numbered column or all columns;
  **Exclude matches** hides matching rows. Missing cells do not match, and an
  empty query shows all data rows even with exclusion enabled.
  **Regex** supports regular expressions within each cell; **Whole words** uses
  Unicode word boundaries in literal or Regex mode. Both disable Fuzzy.
  Invalid expressions show an error and leave no rows available to copy.
  **Copy visible CSV/TSV** copies the shown data records in their displayed order,
  including record-1 headings when enabled. It uses the file's delimiter, quotes
  separators/quotes/line breaks, preserves ragged rows and cell values, and adds
  no row-number column. This exports parsed preview records with LF terminators,
  rather than the original file bytes; hidden/unloaded records are omitted.
- **Checksum / verify** (palette): hash the selection with the existing
  content-hash, BLAKE3, SHA-256, SHA-384, SHA-512, CRC32 (IEEE), MD5 and SHA-1 in one fresh read pass.
  Compare a single file using an explicit algorithm selector and its required
  digest length (8, 32, 40, 64, 96 or 128 hexadecimal characters). Copy the selected digest,
  a report or a checksum list for the selected algorithm: GNU-format binary SHA/MD5
  lists, or a BLAKE3 list for `b3sum --check`. Lists preserve UTF-8 names with
  escaped backslashes and line breaks, omit failed/skipped/unprocessed files,
  and refuse paths that cannot be represented losslessly.
  CRC32 selection copies an SFV list (`absolute path CRC32`, uppercase checksum,
  LF records). SFV cannot escape filenames: NUL, CR/LF line breaks, backslashes
  and non-UTF-8 paths prevent export instead of changing names. Tabs and trailing
  spaces in names are preserved.
  MD5 and SHA-1 support legacy checksum manifests; SHA-256 remains the default.
  SHA-1 lists recognize GNU records, SHA1/SHA-1 tags and SHA1SUMS/.sha1/.sha1sum names.
  **Relative paths** makes SHA/MD5/BLAKE3/SFV lists portable with the folder. The
  dialog shows the common directory of completed files: save the list there and
  verify from that directory. Names start with `./`, preserving literal `-` and
  `;` prefixes. Nested paths and algorithm-specific escaping are retained; the
  mode resets for a new calculation. Absolute paths remain the default.
- **Create symlink / hard link** in the other panel from the selection.
- **CLI launch**: `commander <left> [right]` opens those folders on startup.
- **Run command output**: captures stdout/stderr/exit instead of fire-and-forget.
- **Navigation lock**, **`.` repeat last command**, **named bookmarks in the
  palette**, and **Sync dry-run** preview.
- **Go to home folder** (`Cmd+Shift+H`) and **Go to disk root** (palette) use
  the active tab's normal navigation history. The opposite pane stays put,
  including with navigation lock enabled. `Cmd+H` still toggles hidden files.
- **Open highlighted folder in other pane** (palette or folder-tab context
  menu): browse the cursor folder in the opposite pane's active tab while
  retaining source focus, selection and filters. The destination's Back history
  retains its previous folder; other tabs stay open, including at tab capacity.
- **Gather into a new subfolder** (`Cmd+Shift+N`): move the selection into a
  freshly-named folder in one undoable step (Finder's New Folder with
  Selection). Undo moves the files back and removes the empty folder; redo
  recreates it before gathering again.
- **Batch-rename studio** (`Cmd+Shift+R`): find/replace (plain or **regex**,
  with `$1`-style capture groups), case, prefix/suffix, numbering, with a live
  preview. Resolvable collisions (swaps, rotations, and the case-only
  `Foo` -> `foo` rename) are applied through a safe temp-staged order.
- **Run-command / open-with bar** (palette): run a shell command on the
  selection with `{paths}` / `{names}` / `{dir}` placeholders expanded and
  shell-quoted, with a live preview; save reusable templates.
  **Import / export templates** exchanges Commander JSON bundles. Paste a
  bundle, **Review JSON**, inspect command text/file types, select entries and
  **Add selected templates**. Existing command lines retain their metadata;
  duplicates are skipped. **Copy all templates as JSON** exports the live list.
  Imports save before replacing the live list and never execute commands or
  grant trust. Exchange is limited to 64 KiB and 128 templates. This format is
  separate from Strata action manifests. The window scrolls when expanded;
  use the explicit Run button while the exchange section is open.
  **Find templates** searches saved names, command text and extension filters
  without case sensitivity; whitespace-separated terms must all match.
  **All templates** also shows entries for other file types. Search preserves
  saved order and Move/Delete target the original stored entries. Choosing a
  row fills the draft; searching and pressing Enter in the search field never
  run it. Search is limited to 128 characters and resets when the bar closes.
  In **Template name and file types**, **Require every selected file to match**
  makes a nonempty extension filter require a nonempty opening selection in
  which every item is a matching non-directory row. Mixed selections and
  folders with matching suffixes are refused. Run stays disabled for an
  incompatible strict template chosen through All templates, with an explanation.
  Save/update and JSON exchange retain the rule. Existing templates default to
  the previous any-match behavior; no extension filter means no restriction.
  **Minimum items** and optional **Maximum items** constrain the complete
  opening selection, including templates without extension filters. Use 1–1
  for a single-item command, or minimum 2 with no maximum for batch work.
  Invalid ranges disable Save and Run. All templates cannot bypass count limits.
  **Apply to: Any / Files / Folders** constrains every opening item by its
  captured directory classification. Files and Folders both reject mixed or
  empty selections, and their rules compose with count/extension settings.
  Any preserves the previous behavior. Use the explicit Run button while the
  template settings are expanded; Enter in those fields does not submit Run.
  **Execution** selects **Whole selection**, **Per item: stop on error**, or
  **Per item: continue**. Per item runs sequentially in the resolved working
  folder; `{paths}` and `{names}` contain just the current entry, while `{dir}`
  and `{dir_other}` keep their opening values. Preview shows the first call.
  Progress and the final report show completed/successful/failed/remaining calls;
  failures name the item and exit/error status. Aggregate stdout/stderr are each
  limited to 1 MiB plus summary/truncation text. Stop or closing the bar cancels
  the current process and prevents remaining calls; completed changes are not
  rolled back. Whole selection remains the default for existing templates.
  **Working folder** selects **Opening folder**, **Other pane**, or **Single
  selected folder**, using the window's captured context. The last option
  requires exactly one directory. The resolved folder is shown before Run;
  opening and target roots must both allow command execution under trust labels.
  Missing/inaccessible targets fail instead of falling back to another folder.
  `{paths}` stays absolute, `{names}` stays as opening basenames, and `{dir}` /
  `{dir_other}` keep their opening values; relative arguments use the chosen CWD.
  New JSON exports use version 6 to retain working-folder policy. Version 1–5 imports
  remain supported for their existing policies; bundles that declare newer
  policies under an older version are rejected.
  Completed output offers **Copy stdout**, **Copy stderr**, **Copy report** and
  **Copy report as JSON**. Reports retain the original template, input paths,
  requested initial working folder, attempted commands and batch outcome even
  after the draft is edited. Copy uses the retained streams, including capture
  truncation notices. Each export is limited to 4 MiB; oversized reports are
  refused without copying a partial report. Metadata reports require UTF-8
  paths; stream copying remains available separately.
  **Find in output** searches the retained stdout/stderr display with Match case,
  Smart case, Whole words and background Regex. Next/Prev or Enter/F3 and their
  Shift variants navigate matches while the search field is focused. Wrap search
  controls cyclic navigation; **Wrap output** controls line layout. Search does
  not change the streams or copied reports. It searches captured text only:
  at most 4096 matches; Regex requires at most 256 KiB of displayed text.
  **Line numbers** adds a separate gutter for logical output lines, including
  empty/trailing lines. **Go to line** (button or Enter in its field) reveals a
  1-based line in the combined display; wrapping does not change its number.
  Numbers include the display's stderr heading and inserted separator. Invalid
  numbers show the valid range and keep the current position. Copying streams
  and reports never includes the gutter.
  **Text size** provides A− / A+ in 25% steps (50–200%) and a 100% reset for
  output text and its line-number gutter. It is independent of application and
  pane zoom, lasts for this result view, and resets for a new run/window.

- **Verify checksum list** (palette): highlight a checksum-list file and use
  **Auto**, or choose SHA-256/384/512, SHA-1, MD5, BLAKE3 or CRC32/SFV, then **Verify list**.
  Auto recognizes common list filenames/extensions or unambiguous complete
  contents; untagged 64-digit hashes need SHA-256/BLAKE3 selection. BSD/GNU tagged
  and OpenSSL-style lists are supported alongside GNU untagged and SFV records.
  Conflicting filename hints require an explicit override; mixed algorithms are
  refused. The detected algorithm is shown with the result. The hash lane
  reports Match, Mismatch or Error per record, supports cancellation and copies
  a verification report. Relative paths use the list's folder; absolute paths
  must stay within it. Symbolic links and parent traversal are refused. UTF-8
  lists are bounded to 1 MiB and 10000 records; GNU escaped filenames and SFV
  comments are supported. Rerun reads current contents again.
  Result filters (**Problems / Mismatches / Errors / Matches**) combine with
  case-insensitive filename search. Full verification totals remain visible;
  options persist across reruns. **Copy visible report** exports only shown
  completed rows with full/partial totals and filter context, up to 1 MiB.
  **Copy verification report** continues to copy the full result.

### Comparing and selecting

- **Duplicate finder**: keep the shortest path, oldest or newest known modified
  copy in each group. Date ties and all-unknown dates retain the first listed
  copy. **Keep all groups** clears the proposed Trash list; **Apply policy**
  reapplies the selected rule after manual choices. Policy actions replace
  per-group choices; the existing Trash action remains separate.
  **Copy CSV plan** exports every result in group order with `group`, `action`
  (`keep` / `trash` / `skip`), absolute `path` and `size_bytes`. Quoting preserves
  commas, quotes and multiline names. It copies the current proposal, including
  skipped groups, with a 1 MiB output limit and lossless UTF-8 paths.
  **Copy JSON report** adds applied root/scope, size/name/depth/minimum-copy
  settings, current keep/trash/skip decisions, view/original group identities
  and partial-scan status. It exports the retained proposal without new reads,
  using the same 1 MiB and lossless-path limits. Draft settings are omitted.
  **Sort groups** offers potential savings, file size or copy count (largest
  first), plus the original scan order. Potential savings assumes one retained
  copy in every group, including skipped groups. Keeper/skip choices stay with
  their original groups; CSV follows the displayed group order.
  **Include other panel folder** searches the two folders captured when the
  tool opened, with fresh root-only or recursive enumeration. Groups may span
  folders or remain within either one; labels identify Folder 1 / Folder 2.
  Overlapping paths and hard-link identities count once. Rescan stays bound to
  the captured roots even if the panels navigate elsewhere; JSON names both.
  **Only groups spanning folders** retains groups with distinct physical copies
  in different captured roots. Nested roots use the most specific root; aliases
  alone do not qualify. The choice is retained while two-folder mode is off;
  JSON records it when that mode is active.
  **Include subfolders** restarts with a background traversal of the captured
  folder, including hidden directories. It excludes symbolic links, empty files
  and repeated physical hard links (when file IDs are available). Recursive
  rescans enumerate again, discovering added files and updated metadata.
  Read/changed-file failures show a partial-scan warning. Scope changes and
  **Rescan** retain the chosen rule and sorting while rebuilding manual choices;
  the default unchecked scope uses the existing pane listing.
  **Min bytes / Max bytes** restrict candidate sizes before hashing in either
  scope. Bounds are inclusive whole byte counts; blank fields omit a bound.
  **Apply size limits** rescans with the range, while the applied-size caption
  identifies the current results. Invalid ranges cannot be applied. **Reset
  size limits** restores all nonempty files. Scope/rescan retain applied limits.
  **Groups with at least N copies** stages a minimum of two or more distinct
  identical files. **Apply minimum** rescans; **Reset minimum** restores two.
  Small size buckets are skipped before hashing, and confirmed content groups
  must also meet the threshold. Invalid drafts leave current results unchanged.
  This selects result groups and does not change the one-copy keeper policy.
- Read-only **text diff** (`Cmd+D`), directory
  **synchronise** sheet (`Cmd+Shift+S`), and a disk-usage **treemap**
  (`Cmd+Shift+M`).
  Text diff groups adjacent inserted/deleted rows into difference blocks.
  **First/Previous/Next/Last difference** reveal and highlight a block, with a
  counter and 1-based line positions for the **− file** and **+ file** operands
  named in the header. Navigation stops at the ends; First/Last can recenter
  after manual scrolling. Clicking a navigation button activates diff keyboard
  control: **Alt+Up/Down**, **Alt+Home/End**; **Tab/Esc** returns to files.
  Source files remain read-only and row rendering stays virtualized.
  **Compact diff** hides unchanged stretches outside **Context lines** (0–50,
  default 3). Nearby contexts merge; hidden-row markers show their exact count
  and expand that stretch on click. Difference navigation keeps original source
  positions. Changing context/mode clears expansions and recenters the selected
  block; turning Compact diff off restores the complete retained comparison.
  **Ignore trailing spaces** ignores ASCII spaces/tabs only at line ends, preserving
  leading/interior spacing, original displayed text and separate ending policies.
  **Ignore space changes** compares each ASCII space/tab run as one space;
  **Ignore case** compares Unicode lowercase characters. Both recompute the
  loaded snapshot and survive **Reload compared files**; line endings remain
  significant, and shared rows display the original − operand text.
  **Ignore all spaces** compares text without ASCII spaces/tabs, including
  absent spacing (`a + b` matches `a+b`). It overrides **Ignore space changes**
  while enabled, combines with Ignore case, and preserves original display text,
  line boundaries and CRLF/LF/final-newline differences. Swap and Reload retain it.
  **Ignore CRLF/LF differences** optionally matches Windows and Unix line
  endings, independently of case and spacing. Missing final newlines and CR
  characters inside text remain significant. It retains sources, supports immediate
  recomparison and is preserved by Swap/Reload; no files are rewritten.
  **Expand tabs for comparison** matches tabs with spaces up to the next
  eight-character stop while keeping other spacing significant. Stops count
  Unicode characters rather than screen-cell width. Space-ignore modes override
  this retained setting; case and ending policies compose with it.
  **Swap compared sides** reverses the captured − / + paths and loaded text,
  retaining comparison/search/view settings and rebuilding row coordinates.
  **Ctrl+U** does the same while diff keyboard navigation owns input; text
  fields keep their editing shortcuts. Reload follows the reversed paths;
  swapping uses the retained snapshot and leaves file panels in place.
  **Highlight changes within lines** marks changed characters in paired rows
  with background and underline, leaving selectable text unchanged. Unpaired
  rows are marked in full. Long/complex pairs use a reported simplified span.
  Synchronize's **Find plan rows** searches names by all entered words,
  ignoring case. Review scopes show Planned, Skipped, Conflicts, One-sided,
  Paired files, Matching metadata or Folder pairs; **Reset view** restores all
  rows. Matching metadata does not prove byte identity, and folder pairs are
  not recursively compared. The view shows hidden planned-action counts:
  filtering never excludes actions from Sync or the All direction buttons.
  **Compare file contents** reads paired regular files afresh in a cancellable
  background scan. Equal bytes default to Skip even when dates differ; different
  bytes are detected even when size and date match. Same/Different/Unverified
  contents scopes expose the results. Unreadable, changed or symbolic-link pairs
  remain unverified and cannot authorize a copy in this mode. Folder pairs stay
  shallow. Switching the mode rescans; reports include per-row content results.
  Row direction edits still target the original plan entry. The sync form
  scrolls within its pane so controls remain reachable in smaller windows.
  **Copy plan CSV / JSON** exports **Full plan** (default) or **Shown rows**
  after scanning finishes. Reports retain edited directions, original row
  numbers and paths, target folders, captured roots, policy, filter context and
  full/exported action counts. They describe the retained comparison snapshot,
  not a fresh filesystem check or an executable/importable sync plan. Exports
  require lossless UTF-8 paths and are limited to 1 MiB; errors copy no prefix.
  **Skip shown**, **Restore shown defaults**, and **Reverse shown copies** edit
  only the rows currently shown by the review filter. Targets are captured
  before edits can remove rows from Planned/Skipped. Defaults use the current
  policy and retained comparison statuses; they do not rescan. Reverse leaves
  skipped rows, conflicts and unavailable opposite sources unchanged. The
  buttons report how many rows changed and never start synchronization.
  **Sort rows** offers Plan order, natural case-insensitive Name, Status and
  Direction with **Descending**. Sorting changes the review projection only;
  the original plan and Apply order stay intact. Reset view restores Plan order.
  Shown-row exports follow the sorted view with original row numbers; full-plan
  exports retain original order. Both formats include the view's sort settings.
  Duplicate finder's **Name filters** section accepts one basename glob per
  line in **Include names** and **Exclude names**, with optional case matching.
  Includes are ORed and exclusions win. **Apply name filters** restarts hashing
  in the current scope; edits alone leave existing results unchanged. Invalid
  masks disable Apply. **Reset name filters** restores all names. Filtering
  composes with size limits and subfolder scanning before content reads.
  With **Include subfolders**, **Subfolder levels** bounds traversal: 0 reads
  root files only, 1 also reads immediate subfolders, and empty is unlimited.
  **Apply depth** restarts with the validated draft; **Reset depth** removes the
  bound. Invalid drafts leave the applied bound and current results unchanged.
  Disk usage offers **Logical size**, **File allocation** and **File count**. Switching re-scans
  the captured folder and rebuilds the map using the selected metric. Allocation
  counts regular-file blocks once per hard-link identity, excluding directory/link
  storage; it does not represent reclaimable space. File count counts non-directory
  entries, including empty files, symbolic links and each hard-link alias.
  Unknown or overflowing totals
  are marked incomplete. JSON/CSV include the metric and completeness metadata.
  Disk usage's **Tree** mode filters loaded directory rows by relative path,
  with optional **Fuzzy**, **Match case** and **Clear**. Matching rows retain
  their ancestors as `(context)`; byte/file totals remain full subtree totals.
  Results are cached, including empty searches, and filtering adds no disk
  reads. Filter settings survive Map/Tree switches and Rescan; a newly completed
  snapshot rebuilds the view. Clicking a row retains its original path identity.
  **Share of Root / Parent** shows percentage bars from full snapshot totals;
  filtering never changes the denominator. Parent means the nearest represented
  ancestor in the compressed tree, falling back to the root. Empty totals show
  an em dash, and partial scans retain their warnings.
  Branch buttons and **Collapse all / Expand all** hide or reveal descendants
  without navigating or rescanning. Search temporarily reveals matching branches;
  clearing the query restores saved collapse choices, including after Rescan.
  Visible CSV follows the collapsed projection and marks closed rows.
  **Copy scan JSON** exports retained root totals, map items, compressed
  directory measurements and incomplete-scan flags independently of filtering.
  **Copy visible CSV** exports shown tree rows in order, including ancestor
  context, query/options and partial-scan flags. Nested subtree totals must not
  be summed. Both use lossless UTF-8 paths and a 1 MiB output cap, without more
  disk reads; copying is disabled during rescans and after worker failure.
- **Compare mode** tints rows by how they differ from the other panel, with
  relative-size occupancy bars. Same-named folders and file/folder collisions
  have explicit states; directories are never called identical merely because
  their metadata-sized fingerprints happen to match.
- **Cross-pane selection**: select files only in this panel, differing from the
  other, identical to the other, or same-named (palette).
- **Selection algebra**: select-by-mask (`Cmd+G`), and stash/union/intersect/
  subtract/symmetric-difference of selections.
  The command palette can select or deselect visible files with the highlighted
  file's name ignoring its last extension (`report.pdf`, `report.docx`, `report`).
  Matching ignores case; folders and unrelated or filtered-out picks remain.
  **Go to first/last visible file** moves the cursor past folders according to
  the current sort and filter, retaining selections and the repeat-command target.
  Both actions are available in the command palette when files are visible.
  **Invert visible file selection / Invert visible folder selection** flip only
  the chosen kind in the current view. The other kind and filtered-out selections
  remain intact. Both are available in the command palette.
  Masks accept commas or semicolons, including mixed separators
  (`*.jpg;*.png,!*raw*`). Subtraction still wins. Use Exact names for literal
  filenames containing these separators.
  Optional **Match case** distinguishes uppercase/lowercase in masks, including
  extension shorthand and exclusions. It starts off each time the dialog opens;
  Exact names remains case-sensitive independently of this option.
  **Ignore accents** compares masks, names and extensions after canonical
  decomposition and removal of combining marks, expanding common Latin
  ligatures (`café` → `cafe`, `cœur` → `coeur`). It works with Match case and
  exclusions, defaults off for each dialog and never renames files. Exact names
  disables this comparison option.
  **Treat `*.*` as `*`** lets the exact
  `*.*` term match extensionless names too, including in exclusions. Other masks
  retain their usual meaning. This optional compatibility rule starts off with
  each dialog and is disabled for Exact names.
  `Cmd+G` also offers **Exact names (one per line)** for pasted filename lists:
  literal, case-sensitive, additive, and limited to the current filtered view.
  Lists are bounded to 64 KiB / 4096 names; invalid paths block application.
  Enter adds a line; Cmd+Enter or Select applies it.
  **Apply to** limits mask/list changes to files, folders, or both, with the
  live count following that choice. Entries of the other kind keep their marks.
- **Selection groups** (palette): add visible files or visible folders to the
  current selection, or deselect visible files sharing the cursor's extension.
  Existing selections outside the current filter are retained; folder names never
  participate in extension subtraction.
- **Marked-files set** (`M` to toggle), distinct from the interactive
  selection and never cleared by select-all/invert/clear-selection, with its
  own union/intersect/subtract/symmetric-difference against the selection
  (palette).
- **Drop-stack shelf**: gather files across folders (`Cmd+Shift+A`) and drain
  them into one destination (`Cmd+Shift+V`).

- **Change permissions** (command palette, Unix): edit owner/group/other
  read/write/execute bits or an octal mode with a before/after preview. Applies
  only to selected objects, including selections from other folders; no recursion
  or Undo. Symlinks and objects without a readable protected handle are rejected.

- **Change file timestamps** (command palette, Unix): preview and explicitly
  set modified/accessed times on selected files and real directories. Use a
  UTC date or an RFC 3339 time with timezone; blank fields preserve current
  values. **Use current time** fills both drafts; **Keep existing times** clears
  them. Apply checks captured identities and edited-field conflicts on a worker;
  Reload refreshes the same objects and clears drafts. Links are not followed;
  folder contents and unselected items are untouched. Stop retains applied
  changes; no Undo or creation-time editing. Hard-link names share timestamps.

### Viewing and the rest

- **Basic source syntax colors** for Rust, C/C++, Python and TOML, with a
  **Wrap lines** toggle. Syntax coloring is capped at 64 KiB and disabled under
  Increase Contrast; the ordinary bounded text preview remains available.
  Source/text previews also offer logical line numbers and **Go to line**,
  including wrapped lines; the gutter stays outside selectable source text.
- **Find in preview** supports literal text and an optional **Regex** mode,
  with Match case, Whole words, Prev/Next and Enter/Shift+Enter navigation.
  F3 / Shift+F3 also move to the next/previous match while the search field is
  focused; outside that field F3 keeps its ordinary preview command.
  **Wrap search** defaults on. Turn it off to stop at the first/last collected
  match; unavailable Prev/Next buttons are disabled, and keyboard navigation
  follows the same boundaries. The choice retains the current result.
  Optional **Smart case** ignores case for lowercase queries and matches case
  when the query contains a Unicode uppercase character. Match case overrides
  it. Regex syntax is included in this check; inline regex flags still apply.
  Smart case defaults off and is remembered per pane for the session.
  **Clear** empties the preview search, removes highlights and cancels any
  pending regex job while retaining search options. Focus returns to the query
  field so a new search can be typed immediately.
  Whole words uses Unicode regex word boundaries (underscore and combining
  marks remain parts of words), in both literal and regex mode. Regex indexing
  runs in the background with cancellation and explicit syntax errors; searches
  cover displayed text (including formatted documents and the current PDF page).
- **Formatted JSON preview** preserves key order, duplicate keys, number spellings
  and string escapes while adding indentation. Switch to **Source** for loaded
  original text; both views support find, line numbers and text sizing. Formatting
  is capped at 256 KiB input, 128 nested containers and 1 MiB output; errors fall
  back to source with an explanation. No external tools are required.
- **Text size in previews**: A− / A+ adjust text from 50–200%; 100% resets it.
  Plain text, syntax colors, formatted documents and PDF page text reflow at the
  selected size. Each pane keeps its own session setting; CSV/TSV table view and
  image/PDF raster zoom use their separate controls.
- **Copy displayed text** copies the entire loaded text in the current preview
  representation, including content outside the viewport. Formatted JSON and
  document previews copy rendered text; Source copies the loaded source. Empty
  previews disable the button. CSV/TSV table mode uses Source for this action;
  PDF page text retains its separate copy button.

- **Office document text**: DOCX and ODT previews show paragraphs, line breaks,
  tabs and table cell text with the existing find/copy controls. Text only:
  images, styles, headers, comments and document layout are not rendered.
  Limits: 16 MiB package, 4 MiB main XML and 256 KiB extracted text. Encrypted
  or malformed documents report an error; no embedded resources are opened.
  XLSX and ODS previews show named sheets and tab-separated cell values in
  workbook order, with find/copy support. Formulas use cached values or appear
  as literal text when no result is stored. Number/date styles and charts are
  not rendered. XML reading is limited to 4 MiB across the entire workbook;
  ODS repetitions preserve internal gaps and omit trailing empty cells/rows.
  PPTX and ODP previews show slide names and text in presentation order, with
  paragraph breaks, table text and the same find/copy controls. Speaker notes,
  images, charts, animation and slide layout are not rendered. Up to 512 slides
  fit within the same aggregate XML and output limits.
- **EPUB book text**: previews follow the book's spine order, with chapter
  headings and static HTML text, including entities, paragraphs and tables.
  Find and copy use the extracted text. Scripts/styles and embedded resources
  are not loaded. Encrypted chapters and unsupported spine content report an
  error. Limits: 16 MiB package, 4 MiB aggregate XML/HTML, 512 chapters and
  256 KiB output. Only UTF-8 HTML/XHTML chapter resources are supported.
- **FB2 book text**: previews preserve the book title, body order, section
  paragraphs, verse lines and table text with find/copy support. UTF-8 and
  UTF-16 are supported, plus declared Windows-1251/1252, KOI8-R/U and ISO-8859-5.
  Images remain placeholders and binary payloads are omitted. Input/decoded XML
  are limited to 4 MiB and extracted text to 256 KiB. **.fb2.zip** previews read
  FB2 books directly from a bounded ZIP (16 MiB, 4096 entries), without
  extracting files. For collections, **Book** selects the displayed/copied member.
  At most 32 books, 4 MiB combined XML and 256 KiB combined text/names are loaded;
  oversized, malformed or damaged collections report an error.
- **PDF pages on macOS**: Previous/Next, direct page entry, Fit and 25–400%
  zoom of the fixed raster. Reads/renders on preview workers; source limit 64 MiB,
  raster long edge 2048 pixels, two native jobs at once. **Page text** lazily
  extracts the current page for searching and copying (256 KiB cap; no OCR).
  Interactive forms and links use **Open externally**.
  Expand **Search entire PDF** for literal search across extractable text on
  every page, with Match case, progress and cancellation. Select a matching
  page to open its text and highlight the query. Limits: 4096 pages, 256 KiB
  extracted text per page; high match counts are labelled `4096+`. No OCR.
  **Regex**, **Whole words** and **Smart case** also apply across the document;
  selecting a result transfers these options to the page find toolbar.
  Smart case ignores case unless the entered query contains Unicode uppercase
  characters (including regex syntax); Match case overrides it and inline regex
  flags still apply. It defaults off. Invalid patterns
  report an error. Empty/whitespace-only pages are skipped.
- **RTF text preview**: paragraphs, table cells, Unicode escapes and declared
  document/font encodings feed the existing find, selection and copy controls.
  Field results remain text; instructions, pictures, objects, metadata,
  headers/footers, notes, hidden and deleted text are omitted. No converter or
  external resources are used. Limits: 4 MiB input, 256 KiB output, 128 groups
  and 200,000 tokens. Unsupported encodings, HTML-encapsulated RTF and malformed
  documents report an error. This is text extraction, without page layout/styles.
- **Interactive image preview**: zoom 25–800% relative to Fit, drag/scroll to
  pan, and double-click or use Fit to reset. Larger zoom requests sharper bounded
  decodes through the existing cache. Rotate left/right by 90° or use **Rotate 180°**, mirror horizontally
  or vertically, and reset orientation without changing the source file.
  Transparent pixels appear over a checkerboard, with White/Black background options.
  **Enlarge small images** lets Fit fill the preview while preserving aspect
  ratio. It defaults off and affects display only; Fit/reset retains the choice,
  while a different file starts with enlargement off.
  **Center image** defaults on; disable it to align smaller images to the upper
  left without changing zoom, orientation or background. Alignment is local to
  the pane and current file; changing it resets pan.
- **Unicode text previews**: automatically decode UTF-16LE/BE and UTF-32LE/BE
  with a BOM, alongside UTF-8. Existing search, line numbers and table/document
  previews work on decoded text; malformed Unicode gets an explicit error.
- **Whitespace markers** in plain/source and JSON text previews: enable
  Whitespace to show spaces as dots and tabs as arrows. Markers follow the
  displayed glyphs without altering text, search offsets or copied content.
  The choice defaults off and is independent in each pane for the session.
- **Formatted Markdown and static HTML** previews, with a Source toggle,
  bounded parsing and no external resource loading or embedded execution.
- **95 bundled palettes** from Strata, selected in View and settings and
  remembered across restarts; system Increase Contrast retains priority.
  See [asset notices](THIRD_PARTY_LICENSES.md).

- **Preview** of images (via ImageIO, including RAW/HEIC) and text in the
  opposite panel, with look-ahead caching. Image/video decoding targets the
  physical preview viewport, applies orientation, and runs through typed
  native/standard fallbacks behind a hard timeout and four-slot limit. Decoded
  images retain dimension and 256 MiB allocation caps; failures are explicit
  and retryable rather than infinite spinners. Text preview is debounced and
  read on an isolated one-worker executor with cancellation, file-identity
  revalidation, a 256 KiB limit, and explicit timeout/error states.
- **Relative dates** in the Modified column (Finder/Things style), with the
  absolute timestamp on hover.
- **Status bar and selection summary**: each panel's footer shows item count and
  total size, and with nothing selected the folder's largest and oldest item;
  selecting swaps in a summary of the selection (count, size, average, kinds,
  largest, oldest).
- **Density tiers** (`Cmd+Shift+D`), a one-shot **focus mode**, rich
  **path-to-clipboard** (`Cmd+Shift+C` and palette variants).
  **Copy name without extension** (palette) copies visible selected names, or the cursor
  name when nothing is selected: `archive.tar.gz` becomes `archive.tar` and
  `.env` stays `.env`. This lexical rule also applies to folder names with dots.
  Multiple names use separate lines, with embedded line controls escaped.
- **Accessible adaptive UI**: active pane, keyboard focus, cursor, selection,
  marks, differences, errors, and disabled state use separate color plus
  shape/text cues. File rows expose named columns and state to assistive
  technology. System high-contrast and reduced-motion preferences are
  honored; text scales from 80% to 200%, with compact toolbars and a bottom
  Operations panel on constrained widths. The bottom key bar is generated
  from the focused pane's available actions, and compact file names preserve
  regular/compound extensions while exposing the full name on hover.
- **Developer diagnostics**: the gear panel shows workers, queued I/O, cache
  bytes/jobs/failures, frame and cancellation percentiles, startup phases,
  persistence recovery, preview-provider fallback/timeouts, watcher
  native/polling policy, merged event batches/reconnects, and CI budgets.
  It can export a capability report or a salted, redacted support bundle and
  exposes bounded rollout/kill controls for optional index, preview, and
  external-provider paths.
- **Native context menu**: Open With, Quick Look, Get Info, Duplicate,
  Compress, Copy Path, Show in Finder, Tags, Share, Move to Trash.
- **Resilient session/config persistence** (panel paths, layout, view toggles,
  bookmarks, smart folders, templates, and collections): a malformed record
  is skipped without discarding valid siblings. Includes a **light / dark
  theme** following the system appearance.

## Repository feature transfers

The [transfer ledger](docs/research/feature-transfers-2026-10-27.md) records
implemented ideas from the 99-repository cohort, source references, validation
and remaining work. The command bar now includes reviewed command recipes plus
editable saved-template names and file-extension filters.

## Strata comparison

The [implementation ledger](docs/strata-parity.md) records transferred features,
existing equivalents, platform differences and remaining work against the pinned
Strata source. Full feature parity is not yet complete.

## Roadmap

Shipped from earlier design rounds: pinned favorites, saved searches,
selection sets, the operation-queue engine, focus mode, compare/diff selection,
relative dates, gather-into-folder, the run-command bar, contextual empty
states (truly empty vs filtered-to-nothing vs permission-denied, each with a
one-click recovery), the transfer-queue panel, vim-style chords, the
marked-files set, per-folder view memory, regex batch-rename, and operation
receipts.

A few scope decisions from the last round, so they don't read as oversights:

- The queue panel's concurrency is fixed at one transfer at a time; the
  progress UI and `active_transfer` state are built around a single job; true
  parallel transfers would need that to become a collection.
- Per-folder view memory and operation receipts are session-lifetime only
  (not written to disk), unlike the current directory's own settings, which
  the session file already persists.
- Operation receipts cover moves, deletes, and batch renames (the operations
  that already raise a completion signal); plain copies aren't logged, since
  they have no undo counterpart in this app.
- Vim chords claim `g`, `s`, `j`, `k` away from type-ahead when they're used
  as chord leaders/motions; mid-search (e.g. typing "backjack") they still
  fall through as ordinary search characters.

The architecture and the prioritised plan for what's next live in
[architecture.md](architecture.md) and [recommendation.md](recommendation.md).
The review material is split by trust level: [audit.md](audit.md) is the
verified defect ranking, [recommendation.md](recommendation.md) now contains a
compact Top-500 cleanup/design backlog, and [backlog.md](backlog.md) keeps the
full 621-item raw sweep. [research.md](research.md) is the external evidence
layer: a fixed, relevant cohort of 100 high-star repositories, 30 primary
papers/standards, and 100 deduplicated proposals. The whole cohort was
revalidated on 2026-07-18 with 100 reachable and zero archived projects. Its
implementation ledger records G001-G050 as the first shipped research
milestone and G051-G100 as the second; H001-H012 and I001-I010 record the two
comparative hardening slices. J001-J010 is shipped (see `research.md`'s Phase 4
ledger): color-managed previews, version-store quota visibility, trust labels,
machine-pressure admission, encrypted support bundles, assistive recovery
timeline notes, change provenance, workspace profiles, conflict rules, and
decoder circuit breakers.

## Review backlog

The 2026-07-09 pass added a compact Top-500 list to
[recommendation.md](recommendation.md): 79 bugs,
194 problems, 115 improvements, and
112 suggestions, all preserving the original
`backlog.md` numbering and locations. [architecture.md](architecture.md) now
mirrors the same work as ten small SOLID/DRY reading slices, so the project can
be understood and refactored one bounded context at a time.

The 2026-07-11 three-pass refresh revalidated all 500 numbers, kept that file
as the single source of truth, and closed the next high-risk interaction
cluster: sync rows now retain stable source paths and directory snapshots,
Batch Rename retains its opening panel/directory/selection, drag-and-drop
requires a visible target and cancels elsewhere, and text diff rejects
quadratic work before allocating its matrix. The native macOS Get Info action
from the earlier pass also remains protected by AppleScript-literal escaping.
The continuation pass adds undo/redo for F2 rename, fixes conflict policies
that can reduce an over-budget transfer, and keeps treemap titles bound to the
directory snapshot they visualize.
The safety pass adds poison-tolerant worker/UI locks, checked thumbnail
allocation, cursor/filter invariants, queue-aware toolbar gating, and complete
Gather folder undo/redo.
The external-research pass samples exactly 100 active repositories across file
managers, editors, search, transfer/backup, storage, Rust desktop foundations,
and keyboard-first tools. Its design conclusion is deliberately conservative:
keep paths and dual-pane browsing visible, enrich them with contextual search,
protect the foreground loop with cancellation and budgets, and define file
operations around recovery and end-to-end completion.
The two implementation milestones now cover all `G001-G100`: the final slice
adds live CI performance probes, percentile telemetry, empirical benchmark
trees, startup phases, developer diagnostics, redacted support exports,
runtime provider controls, KLM workflow budgets, and colocated operation ADRs.
The 2026-07-18 comparative refresh then closed twelve concrete gaps: typed
file/folder comparison, fail-closed conflict policy, item-level settings
recovery, shared command availability with disabled reasons, fully background
and bounded image decoding with retryable failures, and observable watcher
recovery with mandatory reconciliation after gaps or reconnects.
Watcher startup now performs mount probing, canonicalization and the initial
subscription scan in background workers. Navigation releases a lightweight
lease; backend shutdown also stays off the UI thread. Installation admission is
bounded, late results remain tied to their directory epoch, and subscription
completion requests reconciliation for changes made during startup.
Watcher configuration disables following child symlinks on supporting backends
(including polling), avoiding link-cycle restarts and observation of linked
external trees. Link entries themselves and real in-scope files still generate
changes. This does not change file-operation or search symlink policies.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Tab` | Switch active panel |
| `Cmd+T` | New folder tab |
| `Cmd+Shift+T` | Reopen the most recently closed folder tab |
| `Cmd+Shift+W` | Close active folder tab |
| `Cmd+Shift+[` / `Cmd+Shift+]` | Previous / next folder tab |
| `↑` / `↓` | Move cursor |
| `Shift+↑` / `Shift+↓` | Extend selection |
| `Shift+Home` / `Shift+End` / `Shift+PageUp` / `Shift+PageDown` | Extend selection to the top, bottom or by a page |
| `Home` / `End` | Jump to first / last row |
| `PageUp` / `PageDown` | Move by one page |
| `Enter` | Open file / enter folder (`..` row goes up) |
| `Backspace` | Go up one folder (cursor lands on the folder you left) |
| `Cmd+[` / `Cmd+]` | History back / forward |
| `Cmd+1`..`9` | Jump to bookmark slot |
| `Cmd+Shift+1`..`9` | Assign active folder to bookmark slot |
| `Space` | Toggle selection |
| `M` | Toggle mark |
| `j` / `k` | Move cursor down / up (`5j` moves 5 rows) |
| `g g` | Jump to first row |
| `s s` | Reverse sort |
| `Cmd+A` | Select all |
| `Cmd+Shift+I` | Invert selection |
| `Cmd+G` | Select by mask |
| `F2` / `Cmd+R` | Rename |
| `Cmd+Shift+R` | Batch rename |
| `F5` | Copy to the other panel |
| `F6` | Move to the other panel |
| `Cmd+Enter` | Move selection into the highlighted folder |
| `Cmd+Shift+Enter` | Copy selection into the highlighted folder |
| `F7` | New folder |
| `Cmd+Shift+N` | New folder with selection |
| `F8` / `Delete` | Move to Trash |
| `Cmd+Shift+A` / `Cmd+Shift+V` | Add to shelf / drain shelf here |
| `Cmd+D` | Diff selected pair |
| `Cmd+Shift+S` | Synchronize panels |
| `Cmd+Shift+M` | Disk-usage treemap |
| `Cmd+F` | Find files |
| `Cmd+Shift+C` | Copy path |
| `Cmd+L` | Go to path |
| `Cmd+P` | Recent folders |
| `Cmd+E` / `Cmd+U` | Equalize / swap panels |
| `Cmd+I` | Get Info |
| `Cmd+K` | Command palette |
| `Cmd+H` | Toggle hidden files |
| `Cmd+Shift+D` | Cycle list density |
| `F3` | Toggle preview |
| `Cmd+Z` / `Cmd+Shift+Z` | Undo / redo |

More commands (run command on selection, cross-pane diff selection, copy-name /
parent / file-URL / shell / relative path, **copy the listing as text / CSV /
Markdown** (the selection if any, else the whole folder), selection stash and
marked-set algebra, **select clutter files** like `.DS_Store`,
**select the 10 largest**, **files like the cursor**, or **empty files**,
duplicates, saved searches, bookmark this folder, **sort by extension / kind**,
**reverse sort**, the **sort toggles** for folders-first and natural-vs-A-Z
ordering, the **transfer queue** panel, and **operation history**) are
available from the command palette (`Cmd+K`). Names sort naturally (`file2`
before `file10`) by default.

## Build and run

```sh
cargo run --release
```

Requires Rust nightly 1.100 (`rust-toolchain.toml` pins
`nightly-2026-09-11`) and macOS (the app links AppKit / AVFoundation /
ImageIO). See [`CHANGELOG.md`](CHANGELOG.md) for the current package version
and release notes.

### macOS distribution (signing and notarization)

Commander is a native macOS binary. Before shipping outside your own machine:

1. **Sign** the release binary (and any `.app` bundle wrapper) with a Developer
   ID Application certificate, for example
   `codesign --force --options runtime --sign "Developer ID Application: …" path/to/commander`.
2. **Notarize** with Apple (`notarytool submit` against the signed artifact, then
   `stapler staple` for a local ticket) so Gatekeeper accepts downloads.
3. Keep the signed identity, Team ID, and notarization credentials out of the
   repo; CI should inject them from a secrets store. Hardened Runtime is
   required for notarization; grant only the entitlements the app actually uses.

Local `cargo run` builds are unsigned and fine for development. The native
release QA path verifies a running binary's code signature when present; it does
not replace the Developer ID + notarization steps above.

## Development

```sh
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

The file-manager logic lives in a UI-independent core (`workspace`,
`workspace/transfer_queue`, `panel`, `transfer`, `opqueue`, `scan`, `command`,
`compare`, `ui_request`, `workload`, `ports`, `provider_runtime`, `fs_util`,
`rename`, `sync`, `undo`, ...) that is unit-tested without a GUI. `Workspace`
emits typed FIFO `UiRequest` values; `app/update.rs` owns the single per-frame
dispatcher and the `app` module remains the egui adapter. Main-thread AppKit
context-menu behavior and background workload admission are injected through
narrow handles rather than reached through UI-global state.

### Architecture checkpoint

The ordered SOLID/DRY pass completed these ownership boundaries:

- `TransferQueueController`, `DeleteController`, and `SpaceProbeController`
  own their asynchronous lifecycle state; `Workspace` applies typed outcomes.
- `UndoCenter` exclusively owns the undo/redo timeline and replay
  reservations. Async settlement is bound to the center owner, timeline
  revision, history entry, and transfer operation; interrupted replays remain
  locked until the matching recovery is resumed or rolled back.
- `transfer::executor::TransferExecutor` owns the transactional lifecycle from
  preflight through terminal publication. Native/clone, delta, sparse, and
  buffered staging run through replaceable backend ports that cannot place the
  final destination, delete the source, or settle the operation journal.
- `UiState` owns transient input and all dialog/modal buffers while
  `UiRequestQueue` preserves non-modal and modal FIFO ordering.
- `ListingState` owns rows, checked revision, and filter-cache invalidation;
  `ViewState` owns private `ViewConfig` plus bounded per-folder memory.
  Selection, watcher, size index, and pure sorting are separate panel owners.
- `operation_journal` validates stable path identity, legal state transitions,
  restart/migration proofs, and rollback/recovery behavior under injected
  side-effect failures.
- Clipboard, opener, Trash, free-space, and native context-menu behavior use
  typed ports. AppKit selectors return invocation-bound intents and perform no
  filesystem or process effects while the menu is tracking.
- `persistence::Persist` is the shared object-safe byte-store boundary.
  Bookmarks and session state use an injected instance; feature flags and the
  version manifest use the same versioned envelope, bounded no-follow reads,
  generation/revision checks, and fail-closed recovery rules. Recovered
  bookmark sources are quarantined before an explicit upgrade can replace
  them.
- The large workspace integration suite lives in `workspace/tests.rs`;
  pathname parsing/validation lives in its own typed module.
- `path_probe::PathProbeController` debounces `Cmd+L` input, binds every result
  to the exact dialog/input generation, and runs the injected filesystem
  `metadata` probe through a dedicated two-worker workload lane. A dialog owns
  at most two admitted probes and never more than one for its current binding;
  further edits collapse into one latest-wins candidate while older slots
  retire, without growing scheduler freshness state. The dialog keeps stable
  one-line status geometry and a polite accessibility live region.

The full serial suite currently passes 979 tests with three intentional
manual/performance harnesses ignored. Native release QA now records automated
checks and blocks until its permission-bound human attestation is complete.
The next high-value architecture work is moving `PanelState::navigate_to`
listing publication off the UI thread and closing the successful path-probe
TOCTOU window. Durable undo history, path-identity-bound replay, migration of
the operation journal and content index to versioned stores, cross-process
persistence CAS, descriptor-relative filesystem effects, and reconciliation
of the last placement-to-journal crash window remain later schema migrations
or OS-hardening work.

The same checkpoint reduced the locked dependency graph from 559 to 516 crates
by enabling only the image decoders Commander uses. The checked-in
[`deny.toml`](deny.toml) and CI gate for pull requests, main-branch pushes, and
a weekly refresh report zero known vulnerabilities and explicitly accept four
unmaintained advisories in Linux-only upstream UI paths: `RUSTSEC-2026-0192`
for `ttf-parser` in Wayland/winit, plus `RUSTSEC-2026-0370`,
`RUSTSEC-2026-0371`, and `RUSTSEC-2026-0372` for the pre-zbus-6 accessibility
stack. The owner is `@themoretheless`. All four were reviewed on 2026-10-21,
have no safe upgrade in the current eframe/accesskit graph, must be reviewed
again by 2026-11-18, and hard-expire at 00:00 UTC on 2026-12-02. The same review
upgraded `zip` to the patched 9.x line after `RUSTSEC-2026-0367`,
`RUSTSEC-2026-0368`, and `RUSTSEC-2026-0369` were published. CI verifies the
SHA-256 of pinned `cargo-deny` version `0.20.2` before executing it. Duplicate
crate groups remain a warning and tracked dependency debt. Run the same
full-lockfile policy locally with:

```sh
cargo deny --all-features --locked check advisories bans licenses sources
```

### Native visual and release QA

The `visual-qa` feature runs the real native eframe/Glow framebuffer path
against a temporary deterministic workspace. It does not load user session or
storage data, and every native effect is a recording fake. CI runs all twenty-two
scenarios as independent strict matrix jobs:

```sh
cargo run --features visual-qa -- --visual-qa desktop_base --output target/visual-qa
cargo run --features visual-qa -- --visual-qa icons_grid --output target/visual-qa
cargo run --features visual-qa -- --visual-qa image_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa pdf_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa permissions --output target/visual-qa
cargo run --features visual-qa -- --visual-qa rename_presets --output target/visual-qa
cargo run --features visual-qa -- --visual-qa rename_target --output target/visual-qa
cargo run --features visual-qa -- --visual-qa rename_options --output target/visual-qa
cargo run --features visual-qa -- --visual-qa name_list --output target/visual-qa
cargo run --features visual-qa -- --visual-qa checksums --output target/visual-qa
cargo run --features visual-qa -- --visual-qa hex_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa hex_search --output target/visual-qa
cargo run --features visual-qa -- --visual-qa folder_tabs --output target/visual-qa
cargo run --features visual-qa -- --visual-qa search_results --output target/visual-qa
cargo run --features visual-qa -- --visual-qa code_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa unicode_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa regex_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa json_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa archive_browser --output target/visual-qa
cargo run --features visual-qa -- --visual-qa minimum_window --output target/visual-qa
cargo run --features visual-qa -- --visual-qa zoom_200_accessible --output target/visual-qa
cargo run --features visual-qa -- --visual-qa confirmation_owner --output target/visual-qa
cargo run --features visual-qa -- --visual-qa pane_tool --output target/visual-qa
```

Each scenario writes `manifest.json` and `capabilities.json`; a successful
capture also writes `frame.png`. Checks cover framebuffer content, exact
logical viewport size, in-viewport pane/row/dialog geometry, pane separation,
modal ownership, disabled modal background, a pane tool that leaves input live,
painted glyph pixels, and strict native-effect counts. The JSON preview case
requires exactly one recording clipboard write with the displayed-text payload;
all other cases require zero calls. The real clipboard stays untouched.
The 200% scenario opens an `1800x1000` native window so
the application receives the intended `900x500` logical workspace at 2x text
zoom. `--allow-skip` is diagnostic only; CI omits it, so unsupported capture,
timeout, and validation failure are red.

Native release evidence is a separate fail-closed path:

```sh
cargo run --features visual-qa -- --native-release-qa diagnostic --output target/native-release-qa
cargo run --features visual-qa -- --native-release-qa strict --output target/native-release-qa --attestation path/to/attestation.json
```

Diagnostic mode never opens a TCC prompt. `build.rs` embeds the source commit
and dirty bit while watching Git metadata plus every tracked package file.
Runtime evidence ignores `COMMANDER_QA_COMMIT`/`GITHUB_SHA`, verifies that
embedded identity against the exact clean source checkout, and hashes the
current executable with BLAKE3. On macOS it additionally compares the running
process's kernel CDHash with the strictly verified on-disk code signature, so a
replaced `current_exe` path cannot pass as the loaded binary. It also records
the macOS build, existing WindowServer, Accessibility, Screen Recording, and
VoiceOver capability state, full `NSScreen` topology, and a canonical
order-independent fingerprint. It renders the declarative menu into an actual
`NSMenu`, then recursively compares all 22 items and separators: titles, stable
accessibility identifiers, full labels, enabled/state/submenu/shortcut
metadata, targets, selectors, and represented objects. The same pure placement
function used by production verifies that the full top-left-anchored menu
rectangle fits the `visibleFrame` at the center and four corners of every
detected display, including negative coordinates and mixed backing scales.
Oversized menus fail instead of being reported as unclipped.

Speech quality, VoiceOver task navigation, separate AppKit popup pixels,
Escape focus return, and real mixed-display behavior remain permission-bound
human checks. Diagnostic output includes an exact-subject attestation template.
The output directory is private (`0700`), fixed-name JSON files are atomically
replaced through a held no-follow directory descriptor, and files are `0600`;
symlink, non-regular, oversized, corrupt, and unknown-field input fails closed.
Evidence schema v4 and attestation schema v2 bind strict mode, commit,
executable digest, topology, case set, reviewer, notes, and timestamp. The
checked-in structural references are
[`qa/native-release-attestation.schema.json`](qa/native-release-attestation.schema.json)
and
[`qa/native-release-attestation.template.json`](qa/native-release-attestation.template.json).
Strict mode accepts only a clean exact commit/binary/topology binding, all
native capabilities, all automated checks, and all five human cases passed
within seven days. Denied, blocked, `not_run`, stale, mismatched, malformed, or
missing evidence exits nonzero.

On the 2026-07-28 implementation host, actual 22-item NSMenu introspection and
all 15 full-rectangle placement probes passed on three displays (1x at negative
x, 2x main, and 2x above). Accessibility and Screen Recording were denied and
VoiceOver was not running, so evidence remained non-passing and strict mode
returned nonzero. No permission prompt was shown.

The production WGPU presentation path remains in the normal application. It is
not used for automated readback: on the tested Metal host, eframe 0.35 queued
`ViewportCommand::Screenshot` but did not deliver `Event::Screenshot` without
external device polling, while polling outside the renderer's event-loop
ownership could hang. Such a timeout is classified as a capture failure, never
as an unsupported capability.

Manual release check:

- Complete the primary two-pane journey with VoiceOver.
- Invoke the focused row's AccessKit `ShowContextMenu` action with
  VoiceOver-Shift-M, including a row in the inactive pane, then navigate the
  resulting AppKit menu without a pointer.
- Verify separate popup pixels and all submenus at display edges.
- Dismiss with Escape and verify focus returns to the exact row.
- Repeat placement on the attested mixed 1x/2x topology.

The architecture and the refactoring plan are documented in
[architecture.md](architecture.md) and [recommendation.md](recommendation.md).

## License

MIT. See [LICENSE](LICENSE).

**Create relative symlink in other panel** in the command palette creates links
for the active selection (or cursor) using targets relative to the other pane.
The links continue to work when their shared directory tree moves without
changing its internal structure. Existing names are preserved with numbered
link names on collision. The absolute-symlink and hard-link commands remain
available separately.

**Follow symbolic link** in the command palette reveals the highlighted link's
resolved target in its parent folder. It follows relative targets and link
chains, shows hidden targets, and reports broken links without navigating away.
Use Back to return to the original folder.

**Copy symbolic-link target** copies the target recorded in the highlighted
link, including a relative or dangling target. **Copy resolved symbolic-link
target** copies its existing absolute destination through any link chain.
Both keep the panels in place; failed resolution leaves the clipboard alone.

The palette can **Add visible symbolic links to selection**, **Remove visible
symbolic links from selection**, or **Invert visible symbolic-link selection**.
These actions include links to folders and dangling links, leave hidden picks
and nonlinks alone, and use the current listing without another disk scan.

Get Info shows a symbolic link's target and its captured **Link status**:
exists, missing, or unavailable with the observed error. Missing/unavailable
size is shown as unavailable. These details come from the loaded directory
snapshot, so opening the card does not reread the link; refresh the listing to
observe an externally changed target.

Get Info also separates a regular file's logical **Size** from **Allocated size**
(the captured Unix block count, with exact bytes). Sparse files may allocate much
less than their length. Unknown allocation stays unavailable; directories and
symbolic links say Not measured. Refresh the listing to update these facts.
Allocation is not exclusive ownership or a prediction of freed space: hard links
and shared filesystem extents can retain blocks after deletion.

**Sort by allocated size** in the palette orders the active pane by captured
regular-file allocation. Invoking it again reverses the known values; unknown
values stay after known ones (folder-pinning preferences still apply). The Size
column becomes **Allocated** and shows that metric, with unavailable observations
explicit. The column header reverses this mode; **Sort by size** restores logical
sizes. Cursor, selections, tab/directory view memory and saved view settings retain
their existing behavior. Totals and transfer sizes continue to use logical bytes.

**Copy listing as JSON** exports the active pane's retained listing with folder,
query, quick-filter facets, sorting and selection context. It copies visible rows,
or visible selected rows when a selection exists, and reports hidden selections
separately. Exact paths, regular-file logical/allocated byte counts and listed UTC
times accompany the current display order. Reports are limited to 1 MiB; invalid,
lossy or oversized output leaves the clipboard unchanged. This does not rescan files.

**Select 10 largest allocated files** adds the ten visible regular files with the
largest observed allocated sizes to the selection. Unknown values, directories
and symbolic links are skipped; existing picks, including hidden picks, remain.
Zero is a known value and can be included when fewer than ten larger candidates
exist. The command is unavailable when no visible allocation facts are known.
The existing **Select 10 largest files** continues to use logical size. Neither
selection estimates space freed by deleting shared or hard-linked storage.

Get Info now includes the captured **Filesystem ID** (device + inode where
available) and regular-file **Hard links** count, including the current name.
The ID of a symbolic link is labeled Link object; its target's link count is not
shown as its own. Directory link counts are not presented as file aliases.
Unknown facts remain unavailable. Refresh the listing and reopen Get Info to
observe changes; inode numbers can be reused after deletion. Long values wrap
and the card body scrolls while its Close control remains available.
Listing JSON includes nullable device_id, file_id and hard_links facts as well.

Get Info also shows **Owner UID** and **Group GID** from the retained Unix
listing metadata, including directories. Symbolic links explicitly show their
own IDs with a **Link object:** prefix, including when the target is missing.
These are numeric IDs; account names are not resolved. Opening Info adds no
filesystem or account database lookup. Missing/non-Unix facts say Unavailable.
Refresh and reopen the card to observe later ownership changes.
Listing JSON v1 exports nullable `owner_uid` and `owner_gid` for every observed
lexical object, with exact integer values and null unknowns. These IDs refer
to their source system and do not establish effective access or ACL ownership.

Use **Sort by accessed** or **Sort by created** from the command palette to
order the active pane by the listed object's access or creation time. Running
the same command again reverses its order. The date column follows the active
metric (Accessed/Created); **Sort by modified** restores Modified. Unknown dates
stay last in both new orders, with existing folder policies and ascending name
and path ties. Cursor, selection and saved view configuration are retained.
New access/creation observations use lexical metadata, including a link's own
dates. Hover the date for the absolute listed value; Get Info adds both dates
and marks link-own values. Listing JSON adds nullable `accessed_utc` and
`created_utc` fields at observed subsecond precision. Creation time remains
unknown when the filesystem does not support it. Access time can be delayed
or disabled by mount policies; metadata is a listing snapshot, not an audit log.
No file dates are changed by sorting; reading file contents elsewhere can
change future access-time observations.

In Find, choose **Report (JSON)** under **Copy as**. **Copy JSON report**
exports visible results in displayed order; **Copy selected** exports the visible
selection. Hidden rows are omitted. A completed query with zero visible rows
can still export its context. The versioned report records the applied query,
root, match mode, source, observed completion/partial coverage, counters,
pivot and export scope. Editing or failing a new query does not relabel retained
results: the report uses their last applied context and flags a differing draft.
Streaming, cancelled/interrupted, capped or skipped coverage remains explicit.

Each row records a native location or an archive/member/index tuple, provider
size/time observations and match explanations with Unicode-character ranges.
UTF-8 paths use `{encoding:"utf8", text:...}`; invalid UTF-8 Unix paths retain
exact lowercase hexadecimal bytes, and Windows unpaired-surrogate paths retain
UTF-16 units. Display names are labels, not lossless filesystem identifiers.
The report is one Commander JSON document (not ripgrep's JSON Lines schema).
Exports are bounded to 1 MiB; refusal shows an error and leaves the clipboard
untouched. Export a smaller selection if a report exceeds the limit. No content
or filesystem reread occurs, and a report is not an executable operation plan.

Find can filter physical allocation separately from logical size:
`allocated:>1MB`, `allocated:=0` or `allocated:1KB..8KB`. Operators, units,
AND composition and saved queries follow existing `size:` rules. For example,
`allocated:=0 size:>1MB` finds large sparse files with known zero allocation.
Only observed regular-file allocation qualifies; directories and links are
excluded. Missing regular-file allocation counts as skipped, including archive
payloads and old indexes. Rebuild old indexes to capture Unix allocation.
Explanations show the observed byte count; search JSON adds nullable
`allocated_bytes`. Shared/hard-linked blocks do not predict freed space.

Find also supports immediate-parent filtering: `parent:"/absolute/folder"`
includes direct children only, independently of Exact/Fuzzy/Regex and case flags.
Use the lexical path shown by the result; relative paths and `..` are rejected.
For archive members, use `parent:"/absolute/bundle.zip!/nested"` (or the archive
path plus `!` for root members). This combines with names, types and contents,
and works with retained index paths without extra filesystem reads.

**Sort by inode** in the command palette orders captured inode numbers (device
breaks numeric ties), with unknown identities last and existing folder grouping.
Activate it again to reverse the order. The Inode column replaces the active
date column while this sort is selected; its hover shows the device and full
inode. Icon view adds an Inode label. Logical sizes and Modified accessibility
metadata remain available. Symlinks use their own inode; equal inode numbers
on different devices do not imply a shared object. No filesystem reread occurs.

**Sort by metadata changed** orders captured Unix status-change time (`ctime`),
with unknown values last, and toggles direction on repeated activation. The
active date column shows Changed; Get Info shows Metadata changed; listing
JSON includes nullable `changed_utc` with subsecond precision. This is separate
from creation and content-modification time: permissions, ownership and other
metadata changes may update it. Symlinks use their own observed change time.
Non-Unix change-time observations are unavailable. These displays and sorting
reuse listing metadata without new filesystem reads.

**Select hard-link aliases of cursor** adds the highlighted regular file and
its visible aliases to selection using captured device/inode pairs. Another
visible path with the same complete identity is required. Ordinary copies,
symbolic links, directories and unknown identities are excluded. Existing and
hidden selections remain intact. The command does no filesystem or content
reads, and does not search for aliases outside the visible listing.

**Find hard-link aliases of cursor** starts recursive Find in the source pane's
folder tree, including the highlighted regular file and its aliases. It uses
captured identity and starts with archives off. The query fields `device:D` and
`inode:N` also work manually, accept decimal u64 values and `!` exclusions, and
combine with existing name/type/content filters. Unknown components remain
skipped, including archive members when archives are enabled. Existing indexed
volume/file IDs are reused; metadata-only cached matches need no source-file
reread. Inodes are scoped by device and may be reused: results are observations,
not authorization for file operations or a global alias inventory.

Duplicate Finder's **Name filters** offers **Match extensions**, initially
case-sensitive, and **Ignore ASCII extension case**. Apply restarts the scan
and rebuilds keep choices; drafts do not retarget existing plans. The mode
partitions content-verified groups by final extension, reapplies minimum-copy
and cross-root rules, and appears in JSON's applied name-filter context. Name
case remains independent. Existing one-representative-per-hard-linked-object
policy is retained; this mode does not promise fewer reads or reclaimed space.

Duplicate Finder can additionally group content-identical files by full name, name
without the last extension, or immediate parent folder name, with optional ASCII
case folding. Stage these under Name filters and Apply; extension constraints
combine with the selected component. The applied policy is shown in the summary
and JSON report. See [qualification](docs/qualification/duplicate-name-components-2026-11-02/README.md).

Duplicate Finder's Keep choices also support literal name A–Z/Z–A, shortest/longest
native-byte name and shallowest/deepest lexical path. Ties retain the first listed
copy; Apply policy replaces manual decisions and Keep all groups skips deletion.
See [qualification](docs/qualification/duplicate-keeper-ranking-2026-11-02/README.md).

Duplicate Finder can match the complete path relative to each captured scan root
(Group by → Same relative path), with optional ASCII case folding. This compares
corresponding content-identical files across folder trees; overlapping paths use
the most specific root. See [qualification](docs/qualification/duplicate-relative-paths-2026-11-02/README.md).

Find supports observed regular-file hard-link counts: `links:>1`, `links:2`,
`links:=0`, or `links:2..4`. Unknown/archive counts remain skipped; cached values
are retained observations. Find JSON exposes nullable `hard_links`.
See [qualification](docs/qualification/hard-link-count-search-2026-11-02/README.md).

**Sort by hard-link count** in the command palette selects a **Links** column
and orders observed regular-file link counts numerically; repeat to reverse.
Icon view shows **Hard links**. Counts include names outside the current folder.
Directories and symbolic links have no regular-file count; unavailable values
stay last in either direction, within the existing folder grouping. Sorting
uses captured observations and preserves cursor and hidden selections.

Find also supports **accessed-after:** and **accessed-before:** with strict
access-time boundaries, independently of modified-time after:/before:. For example,
`accessed-after:2026-01-01 accessed-before:2026-02-01` uses UTC date boundaries;
RFC 3339 times with offsets and fractional seconds use the shared date parser.
New indexes retain exact access observations; legacy caches retain their original
second precision. Missing/invalid access times and unknown archive-member times
are reported as skipped rather than borrowed from the archive container.

**created-after:** and **created-before:** filter observed creation/birth time
independently of modification and access time, using strict UTC dates or RFC 3339
boundaries. New indexes preserve exact creation timestamps; old indexes or
filesystems without that observation report unknown/skipped values. Find JSON
adds nullable **created_utc** for native rows. Archive-container dates are never
substituted for a member's missing creation time.

**changed-after:** and **changed-before:** filter Unix metadata/status-change
time (ctime), including permission/link/ownership changes, independently of
content modification or creation time. They share strict UTC/RFC 3339 boundaries,
cache support and saved queries; unsupported/old/invalid observations stay
unknown/skipped. Find JSON includes nullable native **changed_utc**.

Find's **Group by** row offers List, Modified, Accessed, **Created** and
**Metadata changed**. Creation/status groups use captured observations; unknown
or archive-member dates stay in Unknown. Switching groups preserves the
highlighted result by location and retains hidden/selected rows. JSON reports
record the chosen group mode and follow the displayed order without rerunning
the query.
