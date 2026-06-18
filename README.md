# Commander

A dual-pane file manager for macOS, built in Rust with [egui](https://github.com/emilk/egui).

Two side-by-side panels, keyboard-first navigation, and native macOS
integration (Quick Look, Finder tags, share sheet, APFS clone copies).

## Features

- **Dual panels** with an active-panel marker; `Tab` switches sides.
- **Copy / move / delete** on a background thread with a live progress
  window (speed graph, ETA, per-file error list). Copies use native
  `copyfile` with APFS cloning and fall back to a buffered copy.
- **Safe overwrites**: the confirmation dialog offers Overwrite All,
  **Keep Both**, and Skip. Overwrites stage the new copy and swap it into
  place, so an interrupted copy never destroys the existing file. Copying
  or moving a path into itself is rejected.
- **Drag and drop** between panels (and onto subfolders), routed through
  the same engine as the keyboard, so cross-volume moves and conflicts are
  handled.
- **Folder tree sidebar**, breadcrumb path bar, and a per-panel filter box.
- **Preview** of images (via ImageIO, including RAW/HEIC) and text in the
  opposite panel, with look-ahead caching.
- **Native context menu**: Open With, Quick Look, Get Info, Duplicate,
  Compress, Copy Path, Reveal in Finder, Tags, Share, Move to Trash.
- **Light / dark theme** following the system appearance.

## Design backlog

Top ideas borrowed from polished editors and file tools:

1. **Pinned places**: favorites and project roots above the folder tree.
2. **Saved searches**: reusable filters like "Large media this week".
3. **Selection sets**: name and recall a temporary selection.
4. **Diff drawer**: a dedicated compare summary before copy/move.
5. **Operation queue**: stacked transfers with pause, resume, and reorder.
6. **Inspector tabs**: Info, Preview, Versions, and Permissions in one panel.
7. **Shortcut editor**: searchable keybinding map with conflict warnings.
8. **Command aliases**: user-defined palette aliases for repeat workflows.
9. **Workspace profiles**: saved two-panel layouts per project/task.
10. **Inline action rail**: row-level quick actions on hover for common tasks.

Next 10 design ideas:

1. **Keyboard Steer mode**: accessible pick-up, arrow-key move target, drop/cancel.
2. **Command previews**: palette rows show the affected panel, count, or target.
3. **Per-folder view memory**: remember density, sort, and filters per location.
4. **Action review drawer**: one compact place to inspect pending copy/move/delete.
5. **Compare summary badges**: newer, different, and unique counts before selecting.
6. **Inline conflict suggestions**: rename/keep-both names previewed before transfer.
7. **Workspace switcher**: named two-pane setups with shelf and smart-folder context.
8. **Activity timeline**: searchable operation history with jump-back affordances.
9. **Quick scopes**: restrict commands and filters to panel, selection, or shelf.
10. **Inspector lenses**: swap the preview pane between Info, Diff, Media, and Usage.

More editor-grade ideas:

1. **Palette macros**: record a short chain of commands and rerun it by name.
2. **Transfer dry run**: preview resulting names, conflicts, bytes, and skips.
3. **Split preview**: pin preview left/right/top/bottom instead of only opposite pane.
4. **Local command history**: show the last few commands for the current folder.
5. **Contextual empty states**: folder-specific suggestions for denied, empty, or filtered views.
6. **Selection algebra**: union, subtract, intersect with mask, compare, and shelf.
7. **Per-kind columns**: image dimensions, media duration, archive contents, text line count.
8. **Focus mode**: temporarily hide toolbars/status UI for dense keyboard work.
9. **Reviewable undo stack**: browse reversible actions before choosing Undo/Redo.
10. **Pane roles**: label panels as Source, Target, Archive, Review, or Scratch.

Fresh 10 product-design ideas:

1. **Scope action rail**: bottom-bar actions adapt to selection, filters, and shelf.
2. **Project lanes**: split the shelf into named buckets like Review, Ship, Archive.
3. **Conflict rehearsal**: run a simulated copy/move and pin the proposed decisions.
4. **Finder tag lens**: filter, group, and batch-edit macOS tags from the panel header.
5. **Breadcrumb command zones**: each crumb exposes copy path, open sibling, and pin.
6. **Search handoff**: turn any active filter into a saved smart folder in one click.
7. **Transfer receipts**: every operation leaves a compact, searchable receipt.
8. **Peek compare**: hold a modifier to preview why a compared row is tinted.
9. **Selection recipes**: save mask/facet/compare combinations as reusable selectors.
10. **Keyboard command tray**: show the next likely command from recent local context.

Next implementation ideas:

1. **Filter handoff**: promote the current panel filter into a saved smart folder.
2. **Compare tint explanations**: row hover tells why an item is unique or different.
3. **Shelf lanes lite**: tag staged files as Copy, Review, Archive, or Later.
4. **Pinned filter presets**: put saved searches beside the facet chips.
5. **Receipts drawer**: list the last copy/move/delete outcomes with undo state.
6. **Conflict rehearsal row**: preview keep-both names before opening the transfer dialog.
7. **Sibling crumb menu**: jump to neighboring folders from each breadcrumb segment.
8. **One-shot focus mode**: hide chrome until the next pointer movement.
9. **Compare quick select**: chips for Unique, Different, and Newer in compare mode.
10. **Command next-best hint**: status bar suggests one likely follow-up action.

Next 10 interaction ideas:

1. **Saved-search chips**: surface the top smart folders next to filter facets.
2. **Shelf lane labels**: mark staged items as Copy, Review, Archive, or Later.
3. **Receipt center**: browse recent operation outcomes and jump to affected paths.
4. **Undo timeline**: inspect reversible moves/renames before applying undo.
5. **Crumb sibling menu**: open neighboring folders from a breadcrumb segment.
6. **Conflict dry run**: preview collision decisions before starting a transfer.
7. **Panel role badges**: label panes as Source, Target, Review, or Scratch.
8. **Command follow-up ranking**: bias palette results toward the current context.
9. **Selection recipe pins**: save and rerun compare/mask/facet selections.
10. **Hover inspector lens**: show compact metadata beside the cursor row.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Tab` | Switch active panel |
| `↑` / `↓` | Move cursor |
| `Shift+↑` / `Shift+↓` | Extend selection |
| `Home` / `End` | Jump to first / last row |
| `PageUp` / `PageDown` | Move by one page |
| `Enter` | Open file / enter folder (`..` row goes up) |
| `Backspace` | Go up one folder (cursor lands on the folder you left) |
| `Space` | Toggle selection |
| `F3` | Toggle preview |
| `F5` | Copy to the other panel |
| `F6` | Move to the other panel |
| `F7` | New folder |
| `F8` / `Delete` | Move to Trash |
| `Cmd+A` | Select all |
| `Cmd+H` | Toggle hidden files |

Names sort naturally (`file2` before `file10`).

## Build and run

```sh
cargo run --release
```

Requires a recent stable Rust toolchain and macOS (the app links
AppKit / AVFoundation / ImageIO).

## Development

```sh
cargo test                       # unit tests (UI-independent core)
cargo clippy --all-targets       # lints (the repo is clippy-clean)
cargo fmt --check                # formatting
```

The file-manager logic lives in a UI-independent core (`workspace`,
`panel`, `transfer`, `scan`, `command`, `fs_util`) that is unit-tested
without a GUI; the `app` module is a thin egui layer over it.

## License

MIT. See [LICENSE](LICENSE).
