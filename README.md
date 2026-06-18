# Commander

A dual-pane file manager for macOS, built in Rust with [egui](https://github.com/emilk/egui).

Two side-by-side panels, keyboard-first navigation, and native macOS
integration (Quick Look, Finder tags, share sheet, APFS clone copies).

**Recent major additions (from designer iterations comparing to Total Commander, VS Code, Path Finder, etc.): per-panel tabs, basic bookmarks, git status glyphs, performance improvements.**

## Features

- **Dual panels** with an active-panel marker; `Tab` switches sides.
- **Per-panel tabs** (multiple locations per side, like Total Commander): Cmd+T new/duplicate, Cmd+W close, Ctrl+Tab / Ctrl+Shift+Tab cycle, +/× in bar, drag-reorder planned, persistence across restarts. Never zero tabs.
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
  opposite panel, with look-ahead caching. (Resizable non-replacing preview in progress per designer specs.)
- **Git status glyphs** in file list (M/A/? etc with color tints, low-weight, additive to existing stripes; computed on refresh with debounce for perf).
- **Bookmarks / favorites** (basic functional: palette commands BeginBookmarks/AssignCurrentToBookmark, jump to last, assign current; full UI/picker/hotkeys/persistence in progress).
- **Native context menu**: Open With, Quick Look, Get Info, Duplicate,
  Compress, Copy Path, Reveal in Finder, Tags, Share, Move to Trash.
- **Light / dark theme** following the system appearance.
- Advanced tools: batch rename, recursive find with facets/smart folders, duplicates finder, dir sync, diff, disk treemap, shelf, undo/redo, density modes, compare mode, command palette (Cmd+K with usage ranking).

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
| `Cmd+T` | New / duplicate tab (on active side) |
| `Cmd+W` | Close current tab (never zeros side) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab on active side |
| `Cmd+K` | Command palette |
| `Cmd+P` | Recent folders |
| `Cmd+Shift+C` | Copy path |
| `Cmd+E` | Equalize panels |
| `Cmd+U` | Swap panels |
| `Cmd+Z` / `Cmd+Shift+Z` | Undo / Redo |

Names sort naturally (`file2` before `file10`).

Tabs and bookmarks draw from classic commanders (Total Commander, Double Commander, FAR) and modern editors (VS Code tabs/palette/git, Path Finder favorites/preview).

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

Performance work ongoing (git status debounced; benchmarks added for refresh/git paths showing ~2.5x wins; more fixes from audit: allocations, per-frame work, multi-tab scaling).

## License

MIT. See [LICENSE](LICENSE).
