# Strata comparison and implementation ledger

Reference: [lgse/strata](https://github.com/lgse/strata), inspected at commit
`ac5ab4f22d86798a40d7ad30eb3528d56eecf137` on 2026-10-27.
This is a feature comparison, not a declaration of complete parity. Commander
uses egui/macOS; Strata uses GTK/GIO/GStreamer/Linux. Existing Commander
behavior is identified from its source, not inferred solely from its README.

| Strata capability | Commander status and owning source |
| --- | --- |
| Sortable list, dual panes, bookmarks, history, filter | Existing: `panel`, `workspace`, `bookmarks`, `app/render` |
| Icons grid | Added: virtualized grid in `app/file_list`; pane controls, View menu and command palette; selection, context menus, drag/drop, keyboard scrolling and mode persistence reuse the existing owners |
| Image thumbnails in Icons | Added through bounded `image_cache` workers; active pane warms thumbnails while no full preview is open. The shared cache prioritizes full preview when one is open; inactive panes can show generic icons |
| Miller columns | **Open**: needs an independent, cancellable listing owner for each column and explicit action/focus ownership |
| Fuzzy recursive filename/path search | Existing: streaming `search`, `query`, `app/find_dialog`; the default is folder scoped |
| Global search across Home and local mounts | **Open**: collections already combine roots, but automatic mount discovery, complete/incomplete indexing status and Settings budgets do not match Strata |
| Native formatted Markdown | Added: bounded CommonMark formatting with headings, lists, inline styles, code, task markers and basic table text; Formatted/Source switch. Links stay inert, images show alt text, no syntax highlighting |
| Static HTML | Added: bounded tokenizer-based formatting, entities and basic blocks/styles. No script execution, stylesheet application, resource requests, live links or embedded images. Not a web renderer |
| Text/source/image/RAW preview | Existing bounded worker pipelines: `image_cache`, `panel/preview`, native ImageIO; formatted document layout is cached per pane |
| PDF/document preview | **Partial**: native raster PDF page browsing on macOS, Previous/Next/direct page entry, Fit/zoom and bounded CSV/TSV tables with Source toggle are implemented. Page text now offers selection, copying and literal search within the current page. Whole-document search, OCR, password entry, DOCX, ODT and RTF remain open. Other hosts report unsupported native PDF rendering and offer external opening |
| Audio now-playing/artwork/spectrum | **Open** |
| Video playback, storyboard, scrubbing, ambient lighting, handoff position | **Open**; existing still-frame decoder/Quick Look is not playback parity |
| Rich media/EXIF/GPS properties | **Open**; existing `InfoCard` covers ordinary filesystem metadata |
| 3D model and route previews | **Open**, identified in Strata `services/model_preview.rs` and `services/route_preview.rs` |
| Background/cancellable filesystem work | Existing workload scheduler, watcher reconciliation, operation queue and recovery; synchronous navigation publication remains known architecture debt |
| Create/rename/copy/move/trash/permanent delete | Existing transactional engine; preserve it rather than replacing with Strata's Linux implementation |
| SMB/SFTP URI browsing, authentication, host-key decisions | **Open**: local mounted paths work as filesystem paths, URI-native providers are not implemented |
| Storage properties, display labels, mount/eject/format controls | **Open** as a unified Devices UI; existing volume capabilities govern operations |
| Compact/airy density | Existing `density`, per-pane controls |
| 95 bundled themes | Added from Strata catalog with upstream notices; mapped to egui semantic colors, restored across restarts. Increase Contrast overrides palettes |
| Custom theme editing/import and live Omarchy theme following | **Open**; Omarchy integration requires a Linux-specific equivalent, not changes to macOS configuration |
| Signed in-app update checks/download/install/release channels | **Open**; requires Commander's own release manifest, trust/signing keys and macOS packaging, not Strata release URLs |
| User custom actions | Partial: existing saved command templates, extension matching, quoting, trust gate, output and cancellation. Strata action manifest import/export, script editor, runtime selection, per-item policies and context-menu placement remain **open** |
| Integration catalog and Send to providers | **Open**; existing `provider_runtime` is an admission policy, not an installable integration catalog |
| Keyboard reference (F1) | Added from the shared command catalog, showing availability reasons; participates in existing modal keyboard/Escape routing |
| 10xer chrome-free mode and full Yazi-style bindings | Partial: existing focus mode and j/k/gg/ss; Strata's complete mode remains **open** |
| Clipboard cut/copy/paste and persistent clipboard status | **Open**; current F5/F6 transfer, shelf and path-to-clipboard commands have different semantics |
| Linux portal file chooser and encrypted-volume unlock setup | **Platform-specific, open**; do not install Linux services into a macOS app |
| Twelve UI languages | **Open**; Commander currently has English presentation strings |
| Theme-colored file type icon catalog | **Open** for the full Lucide/Catppuccin association set; existing vector category icons remain |
| Archive browsing, jobs and diagnostics | Existing `archive`, operation center, support bundles and diagnostics; detailed parity needs workflow-by-workflow validation |

## Boundaries

No upstream executable, plugin, updater, service or script was installed or run.
No default file manager, system chooser, mount, filesystem label or desktop
configuration was changed. Source is adapted rather than importing GTK or
GStreamer into the egui application. The theme assets are the only copied
upstream assets in this change; [notices](../THIRD_PARTY_LICENSES.md) preserve
both Strata and Tinted Theming licenses.

## Verification

The initial common regression pass after the first view/document changes passed
2184 tests, with 3 intentional harnesses ignored. Final verification and native
capture outcomes are recorded below after all changes. Headless tests do not
establish native keyboard/VoiceOver or media/provider parity.

Detailed [verification evidence](qualification/strata-2026-10-27/README.md) includes
the initial failures, scoped fixes, successful native captures and remaining limits.
