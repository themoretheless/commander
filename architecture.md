# Architecture

Commander is a dual-pane macOS file manager written in Rust with
[egui](https://github.com/emilk/egui). This document describes how the code is
organised today, the structural debt that has accumulated, and the target shape
the refactoring is moving toward. It is kept in sync with [README.md](README.md)
(user-facing capabilities), [recommendation.md](recommendation.md) (the
prioritised plan of what to do next), and [audit.md](audit.md) (the ranked list
of concrete defects).

## Guiding principle

> The file-manager logic lives in a UI-independent, unit-tested core; the `app`
> module is a thin egui layer over it.

That split is real and worth protecting: ~40 small modules and 373 GUI-free
tests sit under a thin presentation layer. The debt is concentrated in two
oversized core types and in how the core signals the UI.

## Module map (current, on `master`)

### Core (UI-independent, unit-tested)

Grouped by the bounded context each module really belongs to:

- **Navigation / panel state**: `panel` (the `PanelState` god object: entries,
  cursor, selection, sort, filter, history, watcher, dir-size index),
  `jumplist`, `crumbs`, `scan`.
- **Workspace / coordination**: `workspace` (the second god object: two panels,
  transfer queue, undo, pending ops, the dialog-intent flag bus, compare/sync
  glue, drop handling), `command` (the `Command` enum + key mapping).
- **Selection / comparison**: `compare` (cross-pane classification + selection
  set logic, extracted from `workspace`), `selset`, `selection_summary`,
  `query`, `fuzzy`.
- **File operations**: `transfer`, `opqueue` (a pause/resume/reorder/concurrency
  queue engine, wired but not yet surfaced in the UI), `native_copy`,
  `conflict`, `rename`, `rename_order`, `fs_util`, `undo`, `dedup`, `sync`,
  `shelf`, `treemap`.
- **Presentation-independent helpers**: `listing_export`, `reldate`,
  `file_color`, `clipboard`, `cmdtemplate`, `bookmarks`, `smart_folder`,
  `session`, `density`, `focus_mode`, `quick_actions`, `textdiff`.

### UI adapter (`app/`, egui)

`app/mod.rs` owns the `App` struct (presentation state: theme, zoom, image
cache, tree widget, and ~20 transient dialog buffers). Per-frame orchestration
lives in `app/update.rs`; input translation in `app/keys.rs`; one file per
dialog/sheet (`confirm_dialog`, `batch_rename_dialog`, `sync_dialog`,
`find_dialog`, `palette_dialog`, ...); row rendering in `app/file_list.rs` and
`app/render.rs`; native macOS menu in `native_menu`.

### Size hot-spots

| File | Lines | Note |
| --- | --- | --- |
| `src/workspace.rs` | ~2,980 | God object; ~1,300 lines are its test module |
| `src/panel.rs` | ~2,545 | God object; `PanelState` mixes 4 concerns |
| `src/transfer.rs` | ~1,380 | Cohesive; large but single-purpose |
| `src/app/update.rs` | ~1,000 | Per-frame hub; drains the flag bus |
| `src/app/confirm_dialog.rs` | ~780 | One dialog |

## The core <-> UI boundary today

Three mechanisms connect the core to the shell, in descending order of how much
coupling they create:

1. **Command dispatch (clean).** `app/keys.rs` translates egui events into
   toolkit-independent `KeyPress`es, `command::map_keys` maps them to
   `Command`s, and `Workspace::execute(Command)` runs them. This direction is
   healthy.

2. **The `*_request` flag bus (the main debt).** `Workspace` carries ~20 fields
   such as `mask_request: bool`, `sync_request: bool`, `treemap_request: bool`,
   `clipboard_request: Option<PathStyle>`, `clipboard_text_request:
   Option<(String, String)>`. `execute` raises a dialog intent by setting a
   flag; each `show_*_dialog` in `app/` drains it with `mem::take`. The flag is
   **edge-triggered** (also meaning "this is the opening frame, grab focus") and
   the bus is **bidirectional** (the UI sets some flags directly too). The field
   names literally encode the UI's dialog catalogue, so the domain depends on
   the presentation.

3. **Public-field mutation (encapsulation leak).** `PanelState` exposes 25+
   `pub` fields; the UI mutates `selected`, `cursor`, `sort_col`, `facets`,
   `show_hidden` directly. No invariant (cursor in range, `selected` subset of
   entries, sort order consistent with the listing) can be guaranteed.

## Known structural debt

- **Two god objects.** `Workspace` has ~10 responsibilities (panels, transfer
  queue + pump, undo, pending-op confirm, compare/sync glue, batch rename,
  duplicate finding, treemap, drop). `PanelState` interleaves four: data
  (`entries`), view config (`sort_col`/`sort_order`/`folders_first`/
  `natural_name_sort`/`show_hidden`), view state (`cursor`/`selected`/
  `search_query`), and async plumbing (`Arc<Mutex<HashMap>>` dir indices,
  `AtomicBool` refresh flag, fs watcher).
- **The flag bus** (mechanism #2 above) is a hand-rolled, untyped event queue
  smeared across ~20 fields with no single drain point.
- **Leaky ports.** The only injected capability is `opener: Box<dyn Fn(&Path)>`.
  Clipboard, Trash, persistence and free-space probing are called inline from
  the core, so the domain is not testable without real side-effects.
- **Async intermixed with view state** on `PanelState`, which prevents the
  panel from being cloned or snapshot-tested. The [audit](audit.md) found
  concrete bugs in exactly this plumbing: a clear/spawn race in the dir-size
  index (#5), a redundant nested rayon `install()` (#6), and a stale watcher
  callback after navigation (#40). Extracting a `DirIndex` / `BackgroundScan`
  owner fixes all three at once.

## Target architecture

One organising principle resolves most of the debt:

> A `Command` produces `Effect`s. The core never names a dialog, and
> side-effects go through ports.

```
input -> Command -> Workspace::execute(cmd) -> pushes Effect(s)
Effect = Open(Dialog) | Clipboard{text,label} | Toast(OpOutcome) | ...
app/ drains Effects once, owns every dialog's state, and holds the ports.
```

Target shape:

- **Effect bus.** Replace the ~20 `*_request` fields with one typed
  `effects: Vec<Effect>` queue, pushed by both `execute` and the UI, drained in
  one place. The dialog-open `Effect` carries a "just opened" marker so the
  focus edge-trigger is preserved.
- **`UiState`.** Group the ~20 transient dialog buffers out of `App` into a
  dedicated state struct, shrinking the `App` god object.
- **`ViewConfig` value object.** Bundle the five sort/filter/hidden fields and
  give it `sort_entries`; the panel and `session` hold it instead of loose
  fields. This is also the substrate for per-folder view memory.
- **Services out of `Workspace`.** `TransferCenter` (queue + pump + poll +
  cancel + dismiss) and `UndoCenter` (stack + perform); `compare` is already
  extracted.
- **Ports.** Define `Clipboard`, `Trash`, `Persist` traits and inject them like
  `opener`, so the core names capabilities, not concrete crates, and returns a
  structured `OpOutcome` the UI renders uniformly.
- **Bounded contexts as modules.** Navigation, Selection, Comparison (done),
  Transfer/ops, View, Persistence each own their types and tests; no core file
  exceeds ~600 lines.

## Validation: the `refactor/god-removal-ui-state` spike

A divergent branch, `refactor/god-removal-ui-state`, has already prototyped this
direction and more: a full Effect bus (the `Requests` struct removed, effects
processed in one `process_effects`), a `UiState` extraction, `Pane` /
`CommandHandler` traits with a plugin registry, a tokio runtime with
`spawn_blocking`, a virtualised file list, plus net-new features (tabs, git
status, tags, notes, configurable columns). It sits on a stale base (`master` is
~42 commits ahead, the branch ~13), so a direct merge is conflict-heavy.

**Decision (see [recommendation.md](recommendation.md)): `master` stays the
mainline; the spike is treated as a proven reference, and its ideas are
re-landed onto `master` in small, reviewable steps** rather than merged
wholesale. The Effect-bus and `UiState` designs above are exactly what the spike
validates.

## Invariants and testing

The pure core is covered by 373 GUI-free tests. The one area the tests do **not**
exercise is egui-frame behaviour: the dialog focus edge-trigger driven by the
flag bus is invisible to the test suite, which is why the Effect-bus migration
must be verified manually in the running app, not just by `cargo test`.
