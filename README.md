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
