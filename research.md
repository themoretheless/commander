# External Research

This document records the external-design pass requested after the internal
Top-500 review. It is deliberately separate from Track F in
[recommendation.md](recommendation.md): Track F remains an exact 500-item,
code-grounded inventory, while this file contains externally inspired product
and engineering hypotheses.

## Method and limits

- Original snapshot date: **2026-07-14** (Asia/Tbilisi). Star values in the
  100-row table are the original point-in-time GraphQL counts, not live badges.
  Every repository was revalidated through the GitHub REST API on
  **2026-07-18**; all 100 were reachable and none was archived.
- Inclusion rule: a public, non-archived GitHub repository with at least
  **1,000 stars**, and a transferable lesson for a keyboard-first desktop file
  manager. Stars are a popularity signal, not proof of quality.
- The sample contains exactly **100 unique repositories** in seven strata:
  file managers (20), editors (15), search/launchers (15), transfer/backup
  systems (20), storage tools (10), Rust desktop foundations (10), and
  keyboard-first workflow tools (10).
- This is a relevance-curated Top 100 for Commander, not GitHub's global
  all-category leaderboard. The strata prevent editor popularity alone from
  crowding out filesystem, transfer, recovery, and desktop-runtime evidence.
- GitHub GraphQL supplied the original repository identity, archive status,
  star count, and update metadata; REST supplied the full-cohort refresh. The
  README/feature contracts of 24 representative projects were then read more
  closely. Scientific claims below link to the paper, author copy, publisher,
  standards body, or project documentation.
- This is comparative design research, not a license to copy source or visual
  identity. Any implementation must be designed for Commander's macOS/egui
  constraints and tested against its own workloads.

### 2026-07-18 cohort refresh

The refresh retained all 100 entries with no replacements, duplicates, or
archived projects. These are the ten highest current star counts inside the
fixed cohort; the original per-row values below remain frozen so the research
snapshot stays reproducible.

| Rank | Repository | Refreshed stars |
| ---: | --- | ---: |
| 1 | [microsoft/vscode](https://github.com/microsoft/vscode) | 187,623 |
| 2 | [tauri-apps/tauri](https://github.com/tauri-apps/tauri) | 109,169 |
| 3 | [neovim/neovim](https://github.com/neovim/neovim) | 101,197 |
| 4 | [zed-industries/zed](https://github.com/zed-industries/zed) | 87,149 |
| 5 | [syncthing/syncthing](https://github.com/syncthing/syncthing) | 86,557 |
| 6 | [localsend/localsend](https://github.com/localsend/localsend) | 85,434 |
| 7 | [junegunn/fzf](https://github.com/junegunn/fzf) | 81,804 |
| 8 | [jesseduffield/lazygit](https://github.com/jesseduffield/lazygit) | 80,470 |
| 9 | [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) | 66,265 |
| 10 | [meilisearch/meilisearch](https://github.com/meilisearch/meilisearch) | 58,629 |

## 100-repository sample

### File managers and explorers (1-20)

| # | Repository | Stars | Transferable lesson |
| ---: | --- | ---: | --- |
| 1 | [files-community/Files](https://github.com/files-community/Files) | 44,288 | Treat tabs, preview, operations, and shell integration as one coherent desktop workflow. |
| 2 | [sxyazi/yazi](https://github.com/sxyazi/yazi) | 40,261 | Use nonblocking I/O, priority-aware tasks, cancellation, and provider-based previews. |
| 3 | [spacedriveapp/spacedrive](https://github.com/spacedriveapp/spacedrive) | 38,569 | Separate physical paths from indexed identity and virtual cross-source views. |
| 4 | [filebrowser/filebrowser](https://github.com/filebrowser/filebrowser) | 35,524 | Put filesystem operations behind a backend boundary that can represent remote roots. |
| 5 | [jarun/nnn](https://github.com/jarun/nnn) | 21,705 | Keep startup and navigation tiny; let pickers and language-agnostic plugins add breadth. |
| 6 | [yorukot/superfile](https://github.com/yorukot/superfile) | 18,718 | A dense multi-pane surface can still have clear hierarchy and live operation feedback. |
| 7 | [ranger/ranger](https://github.com/ranger/ranger) | 17,293 | Model preview as a fallback chain of format-specific providers. |
| 8 | [gokcehan/lf](https://github.com/gokcehan/lf) | 9,377 | Decouple the interactive client from long-lived state and configurable commands. |
| 9 | [TagStudioDev/TagStudio](https://github.com/TagStudioDev/TagStudio) | 7,029 | Add virtual collections and tags without requiring files to move. |
| 10 | [aleksey-hoffman/sigma-file-manager](https://github.com/aleksey-hoffman/sigma-file-manager) | 6,416 | Save task-oriented workspaces instead of treating layout as disposable chrome. |
| 11 | [kimlimjustin/xplorer](https://github.com/kimlimjustin/xplorer) | 5,619 | Make layout and preview capability extensible without exposing core mutations. |
| 12 | [sayanarijit/xplr](https://github.com/sayanarijit/xplr) | 4,776 | Represent keymaps and actions as composable data, not scattered conditionals. |
| 13 | [doublecmd/doublecmd](https://github.com/doublecmd/doublecmd) | 4,322 | Preserve dual-pane symmetry and make background operations first-class. |
| 14 | [kamiyaa/joshuto](https://github.com/kamiyaa/joshuto) | 3,720 | Keep preview and filesystem work asynchronous in a Rust-native core. |
| 15 | [antonmedv/walk](https://github.com/antonmedv/walk) | 3,625 | Make fuzzy navigation a terminal-to-GUI handoff, not a separate information silo. |
| 16 | [derceg/explorerplusplus](https://github.com/derceg/explorerplusplus) | 3,228 | Native shell behavior and low overhead remain differentiators for desktop tools. |
| 17 | [vifm/vifm](https://github.com/vifm/vifm) | 3,221 | Compose two-pane commands with modal, repeatable keyboard grammar. |
| 18 | [FarGroup/FarManager](https://github.com/FarGroup/FarManager) | 2,186 | Archives and remote sources can appear through a virtual-filesystem contract. |
| 19 | [zhanghai/MaterialFiles](https://github.com/zhanghai/MaterialFiles) | 8,564 | Surface storage scope, permissions, and operation progress as understandable states. |
| 20 | [Canop/broot](https://github.com/Canop/broot) | 12,819 | Compress large trees into an interruptible overview that preserves hierarchy. |

### Editors and interaction systems (21-35)

| # | Repository | Stars | Transferable lesson |
| ---: | --- | ---: | --- |
| 21 | [microsoft/vscode](https://github.com/microsoft/vscode) | 187,504 | Use a command registry and isolate lazily activated extensions from the UI loop. |
| 22 | [neovim/neovim](https://github.com/neovim/neovim) | 101,112 | Keep the core independently addressable through commands/events and a UI protocol. |
| 23 | [zed-industries/zed](https://github.com/zed-industries/zed) | 86,923 | Give background work immutable snapshots so the foreground remains responsive. |
| 24 | [helix-editor/helix](https://github.com/helix-editor/helix) | 45,394 | Make the affected selection visible before applying an operation. |
| 25 | [vim/vim](https://github.com/vim/vim) | 40,621 | Build a small grammar of motions, operators, counts, and repeat instead of shortcut sprawl. |
| 26 | [lapce/lapce](https://github.com/lapce/lapce) | 38,650 | Separate high-performance document/core state from rendering and plugins. |
| 27 | [microsoft/edit](https://github.com/microsoft/edit) | 14,383 | A small tool can prioritize predictable defaults and immediate response over breadth. |
| 28 | [micro-editor/micro](https://github.com/micro-editor/micro) | 29,019 | Pair keyboard efficiency with discoverable menus and contextual help. |
| 29 | [mawww/kakoune](https://github.com/mawww/kakoune) | 10,984 | Selection-first interaction makes the scope of destructive commands legible. |
| 30 | [lite-xl/lite-xl](https://github.com/lite-xl/lite-xl) | 6,258 | Keep the trusted core small and move optional behavior behind a narrow plugin API. |
| 31 | [martanne/vis](https://github.com/martanne/vis) | 4,652 | Structural regular expressions can power composable bulk transformations. |
| 32 | [CodeEditApp/CodeEdit](https://github.com/CodeEditApp/CodeEdit) | 22,951 | Honor native macOS focus, menus, windows, and workspace expectations. |
| 33 | [emacs-mirror/emacs](https://github.com/emacs-mirror/emacs) | 5,109 | Commands should be introspectable and self-documenting, not only invocable. |
| 34 | [geany/geany](https://github.com/geany/geany) | 3,664 | Fast startup and an optional plugin layer can coexist with a conventional GUI. |
| 35 | [microsoft/monaco-editor](https://github.com/microsoft/monaco-editor) | 46,331 | Separate the data model from views and virtualize only where scale requires it. |

### Search, fuzzy matching, and launchers (36-50)

| # | Repository | Stars | Transferable lesson |
| ---: | --- | ---: | --- |
| 36 | [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) | 66,088 | Respect ignore semantics and choose parallel/mmap strategies from measured workloads. |
| 37 | [sharkdp/fd](https://github.com/sharkdp/fd) | 43,733 | Friendly defaults and predictable filtering beat exposing every low-level switch. |
| 38 | [junegunn/fzf](https://github.com/junegunn/fzf) | 81,697 | One streaming fuzzy picker can serve files, history, commands, and custom providers. |
| 39 | [skim-rs/skim](https://github.com/skim-rs/skim) | 6,883 | Stream candidates incrementally instead of waiting for a complete result vector. |
| 40 | [helix-editor/nucleo](https://github.com/helix-editor/nucleo) | 1,468 | Use a matcher designed for cancellation and interactive rescoring. |
| 41 | [jhawthorn/fzy](https://github.com/jhawthorn/fzy) | 3,276 | Keep fuzzy scoring deterministic enough to explain and regression-test. |
| 42 | [alexpasmantier/television](https://github.com/alexpasmantier/television) | 6,087 | Define search channels as providers with independent preview and execution. |
| 43 | [meilisearch/meilisearch](https://github.com/meilisearch/meilisearch) | 58,556 | Make typo tolerance and ranking rules explicit product behavior. |
| 44 | [quickwit-oss/tantivy](https://github.com/quickwit-oss/tantivy) | 15,545 | An embedded segmented index can support facets and immutable reader snapshots. |
| 45 | [typesense/typesense](https://github.com/typesense/typesense) | 26,301 | Optimize result quality for instant, typo-tolerant interaction rather than batch search. |
| 46 | [davatorium/rofi](https://github.com/davatorium/rofi) | 16,251 | A small mode/provider interface keeps a launcher extensible and coherent. |
| 47 | [Flow-Launcher/Flow.Launcher](https://github.com/Flow-Launcher/Flow.Launcher) | 15,161 | Normalize asynchronous plugin results into one ranked result model. |
| 48 | [albertlauncher/albert](https://github.com/albertlauncher/albert) | 7,959 | Route queries to typed handlers instead of letting plugins own the whole UI. |
| 49 | [Ulauncher/Ulauncher](https://github.com/Ulauncher/Ulauncher) | 4,483 | User keywords can make recurring workflows explicit and memorable. |
| 50 | [raycast/extensions](https://github.com/raycast/extensions) | 7,610 | Manifest-declared commands and preferences make an extension surface auditable. |

### Transfer, synchronization, backup, and archives (51-70)

| # | Repository | Stars | Transferable lesson |
| ---: | --- | ---: | --- |
| 51 | [rclone/rclone](https://github.com/rclone/rclone) | 58,328 | Model backend capabilities and put circuit breakers around uncertain synchronization. |
| 52 | [syncthing/syncthing](https://github.com/syncthing/syncthing) | 86,340 | Preserve conflict copies, version replaced files, and verify folder health. |
| 53 | [restic/restic](https://github.com/restic/restic) | 34,993 | Commit immutable, content-addressed snapshots only after referenced data exists. |
| 54 | [borgbackup/borg](https://github.com/borgbackup/borg) | 13,508 | Combine checkpointing, content-defined chunks, compression, and authenticated data. |
| 55 | [kopia/kopia](https://github.com/kopia/kopia) | 13,653 | Treat policies, snapshots, verification, and recovery as one lifecycle. |
| 56 | [duplicati/duplicati](https://github.com/duplicati/duplicati) | 14,743 | Expose retention and versioning in a GUI without hiding recovery consequences. |
| 57 | [bup/bup](https://github.com/bup/bup) | 7,327 | Reuse pack/index structures for efficient global deduplication. |
| 58 | [rustic-rs/rustic](https://github.com/rustic-rs/rustic) | 3,131 | A typed Rust core can keep repository and policy logic UI-independent. |
| 59 | [libarchive/libarchive](https://github.com/libarchive/libarchive) | 3,554 | Put diverse archive formats behind a capability API and treat parsers as hostile input. |
| 60 | [peazip/PeaZip](https://github.com/peazip/PeaZip) | 7,630 | Browse and selectively extract archives before committing an operation. |
| 61 | [M2Team/NanaZip](https://github.com/M2Team/NanaZip) | 14,792 | Native shell integration matters as much as the compression engine. |
| 62 | [magic-wormhole/magic-wormhole](https://github.com/magic-wormhole/magic-wormhole) | 22,709 | Human-verifiable codes can establish a secure ad-hoc transfer. |
| 63 | [schollz/croc](https://github.com/schollz/croc) | 35,520 | Resume and multiplex encrypted transfers instead of restarting large payloads. |
| 64 | [localsend/localsend](https://github.com/localsend/localsend) | 85,228 | Local discovery can make cross-device transfer feel like a native file operation. |
| 65 | [LANDrop/LANDrop](https://github.com/LANDrop/LANDrop) | 5,899 | Keep LAN transfer setup minimal and make destination identity visible. |
| 66 | [haiwen/seafile](https://github.com/haiwen/seafile) | 14,967 | Libraries, file history, and block synchronization form a useful user-level model. |
| 67 | [RsyncProject/rsync](https://github.com/RsyncProject/rsync) | 5,003 | Delta transfer, dry-run, and itemized change output are complementary features. |
| 68 | [tus/tusd](https://github.com/tus/tusd) | 3,823 | Persist transfer offsets so interruption does not invalidate completed work. |
| 69 | [ipfs/kubo](https://github.com/ipfs/kubo) | 17,081 | Content identity can provide deduplication and integrity, but needs a path-facing UX. |
| 70 | [seaweedfs/seaweedfs](https://github.com/seaweedfs/seaweedfs) | 33,469 | Design metadata access separately for the many-small-files case. |

### Storage, disk usage, and filesystem events (71-80)

| # | Repository | Stars | Transferable lesson |
| ---: | --- | ---: | --- |
| 71 | [muesli/duf](https://github.com/muesli/duf) | 15,192 | Present volume capacity and filesystem facts as a compact, scannable summary. |
| 72 | [bootandy/dust](https://github.com/bootandy/dust) | 11,985 | Preserve hierarchy while visualizing where space is consumed. |
| 73 | [Byron/dua-cli](https://github.com/Byron/dua-cli) | 6,024 | Separate expensive scanning from deliberate, reviewable cleanup. |
| 74 | [dundee/gdu](https://github.com/dundee/gdu) | 5,814 | Parallel scans still need responsive cancellation and incremental navigation. |
| 75 | [qarmin/czkawka](https://github.com/qarmin/czkawka) | 32,071 | Cache scan work and distinguish duplicate, similar, empty, and broken-file modes. |
| 76 | [littlefs-project/littlefs](https://github.com/littlefs-project/littlefs) | 6,800 | Design state transitions for interruption and bounded resources from the start. |
| 77 | [rfjakob/gocryptfs](https://github.com/rfjakob/gocryptfs) | 4,526 | Filename limits, reverse mappings, and path edge cases deserve explicit tests. |
| 78 | [trapexit/mergerfs](https://github.com/trapexit/mergerfs) | 5,729 | Selection and placement policies should depend on per-volume capabilities. |
| 79 | [watchexec/watchexec](https://github.com/watchexec/watchexec) | 7,063 | Coalesce noisy filesystem events and make debounce behavior observable. |
| 80 | [notify-rs/notify](https://github.com/notify-rs/notify) | 3,411 | Watcher guarantees differ by backend; overflow and disconnect need a recovery path. |

### Rust desktop foundations (81-90)

| # | Repository | Stars | Transferable lesson |
| ---: | --- | ---: | --- |
| 81 | [emilk/egui](https://github.com/emilk/egui) | 29,634 | Stable widget IDs and a strict frame budget are correctness concerns in immediate mode. |
| 82 | [iced-rs/iced](https://github.com/iced-rs/iced) | 30,965 | Model state updates and asynchronous commands as separate, typed concepts. |
| 83 | [slint-ui/slint](https://github.com/slint-ui/slint) | 23,192 | Keep presentation declarative and feed it bounded data models. |
| 84 | [tauri-apps/tauri](https://github.com/tauri-apps/tauri) | 109,014 | Keep a small trusted core and grant optional capabilities explicitly. |
| 85 | [linebender/xilem](https://github.com/linebender/xilem) | 5,445 | Typed view composition can make state ownership visible at compile time. |
| 86 | [linebender/druid](https://github.com/linebender/druid) | 9,705 | Data-first state and lenses clarify which component can mutate which value. |
| 87 | [longbridge/gpui-component](https://github.com/longbridge/gpui-component) | 12,063 | Reusable desktop controls should support dense data, keyboard focus, and themes. |
| 88 | [pop-os/cosmic-epoch](https://github.com/pop-os/cosmic-epoch) | 6,419 | Accessibility, theming, and system integration are architectural, not polish tasks. |
| 89 | [rust-windowing/winit](https://github.com/rust-windowing/winit) | 6,047 | Keep platform event-loop behavior at a narrow boundary. |
| 90 | [tokio-rs/tracing](https://github.com/tokio-rs/tracing) | 6,775 | Carry structured spans across asynchronous work instead of reconstructing failures. |

### Keyboard-first workflow tools (91-100)

| # | Repository | Stars | Transferable lesson |
| ---: | --- | ---: | --- |
| 91 | [ajeetdsouza/zoxide](https://github.com/ajeetdsouza/zoxide) | 38,010 | Rank destinations by frecency, not chronology alone. |
| 92 | [atuinsh/atuin](https://github.com/atuinsh/atuin) | 30,540 | Search history with contextual fields and stable identity. |
| 93 | [nushell/nushell](https://github.com/nushell/nushell) | 39,987 | Treat file listings and command output as typed records. |
| 94 | [fish-shell/fish-shell](https://github.com/fish-shell/fish-shell) | 33,844 | Suggestions and completions can teach the interface while preserving speed. |
| 95 | [jesseduffield/lazygit](https://github.com/jesseduffield/lazygit) | 80,336 | Contextual key help and operation-specific panes reduce command memorization. |
| 96 | [jesseduffield/lazydocker](https://github.com/jesseduffield/lazydocker) | 52,019 | Dense multi-resource views work when focus and available actions stay obvious. |
| 97 | [zellij-org/zellij](https://github.com/zellij-org/zellij) | 34,275 | Persist layouts/sessions and show the current keyboard mode continuously. |
| 98 | [ratatui/ratatui](https://github.com/ratatui/ratatui) | 21,660 | Deterministic rendering and narrow widget boundaries make interaction testable. |
| 99 | [charmbracelet/bubbletea](https://github.com/charmbracelet/bubbletea) | 43,731 | A message/update/command loop makes effects explicit and replayable. |
| 100 | [eza-community/eza](https://github.com/eza-community/eza) | 22,606 | Rich metadata should remain optional, aligned, and fast to scan. |

## Evidence index

### Research papers and standards

| ID | Source | What it changes for Commander |
| --- | --- | --- |
| S01 | [Boardman and Sasse, cross-tool PIM study](https://discovery.ucl.ac.uk/id/eprint/13438/) | People use varied strategies; one rigid organization model is a poor fit. |
| S02 | [Dumais et al., Stuff I've Seen](https://www.microsoft.com/en-us/research/?p=145459) | Time, people, type, thumbnails, and prior context are strong re-finding cues. |
| S03 | [Teevan et al., The Perfect Search Engine Is Not Enough](https://doi.org/10.1145/985692.985745) | Users often orient through known context instead of issuing one perfect query. |
| S04 | [Cutrell et al., Fast, Flexible Filtering with Phlat](https://www.microsoft.com/en-us/research/wp-content/uploads/2006/04/phlat-color-camera-readyv0.31.pdf) | Search, browse, facets, and tags should share one interaction model. |
| S05 | [Barreau and Nardi, Finding and Reminding](https://homepages.cwi.nl/~steven/sigchi/bulletin/1995.3/barreau.html) | Folder location is both a retrieval cue and a reminder; search must not erase it. |
| S06 | [Bergman et al., folder structure and navigation](https://doi.org/10.1002/asi.21415) | Retrieval cost depends on both depth and folder size; shallow overview matters. |
| S07 | [Dinneen and Julien, What's in People's Digital File Collections?](https://arxiv.org/abs/2402.06421) | Test corpora must represent different personal, work, and IT collections. |
| S08 | [Gifford et al., Semantic File Systems](https://www.sigmod.org/publications/dblp/db/conf/sosp/sosp91.html) | Attribute-derived virtual directories are useful, but should complement paths. |
| S09 | [Miller, Response Time in Man-Computer Conversational Transactions](https://doi.org/10.1145/1476589.1476628) | Interaction needs explicit latency budgets and visible progress beyond them. |
| S10 | [Card, Moran, and Newell, Keystroke-Level Model](https://doi.org/10.1145/358886.358895) | Common workflows can be compared by operators, not aesthetic preference alone. |
| S11 | [Shneiderman, Direct Manipulation](https://www.cs.umd.edu/~ben/publications.html) | Keep objects visible and actions incremental, rapid, and reversible. |
| S12 | [Accot and Zhai, Beyond Fitts' Law](https://research.google/pubs/beyond-fitts-law-models-for-trajectory-based-hci-tasks/) | Narrow drag corridors and cascading pointer paths impose measurable cost. |
| S13 | [Abowd and Dix, Giving Undo Attention](https://pure.cardiffmet.ac.uk/en/publications/giving-undo-attention/) | Undo is a user intention and recovery contract, not merely stack mechanics. |
| S14 | [Berlage, Selective Undo](https://doi.org/10.1145/196699.196721) | An old action is undoable only when it still has a meaningful interpretation. |
| S15 | [Patterson et al., Recovery-Oriented Computing](https://www2.eecs.berkeley.edu/Pubs/TechRpts/2002/5574.html) | Design for diagnosis and recovery speed, not only lower failure frequency. |
| S16 | [Saltzer, Reed, and Clark, End-to-End Arguments](https://www.cs.cmu.edu/~15712/papers/saltzer84.pdf) | A transfer is correct only when the application verifies the final destination. |
| S17 | [Tridgell and Mackerras, The rsync Algorithm](https://rsync.samba.org/tech_report/) | Delta transfer pays off when source and destination are similar and I/O is remote. |
| S18 | [Muthitacharoen et al., LBFS](https://sosp.org/2001/papers/mazieres.pdf) | Content reuse can reduce bandwidth by an order of magnitude on suitable workloads. |
| S19 | [Quinlan and Dorward, Venti](https://www.usenix.org/conference/fast-02/venti-new-approach-archival-data-storage) | Content-addressed, immutable blocks simplify integrity, caching, and snapshots. |
| S20 | [Xia et al., FastCDC](https://www.usenix.org/conference/atc16/technical-sessions/presentation/xia) | Chunking needs a measured CPU/dedup tradeoff and a size threshold. |
| S21 | [Meyer and Bolosky, A Study of Practical Deduplication](https://www.usenix.org/events/fast11/tech/techAbstracts.html) | Whole-file dedup captures much of the benefit with far less complexity. |
| S22 | [Agrawal et al., Five-Year File-System Metadata Study](https://www.usenix.org/conference/fast-07/five-year-study-file-system-metadata) | Real trees mix many small directories with a heavy tail of huge files. |
| S23 | [Pinheiro et al., Failure Trends in Disk Drives](https://www.usenix.org/event/fast07/tech/full_papers/pinheiro/pinheiro.pdf) | Hardware health signals are imperfect; verify data instead of trusting one proxy. |
| S24 | [Pillai et al., Crash-Consistent Applications](https://www.usenix.org/conference/osdi14/technical-sessions/presentation/pillai) | Rename/write ordering assumptions vary across filesystems and must be tested. |
| S25 | [Dean and Barroso, The Tail at Scale](https://research.google/pubs/the-tail-at-scale/) | p95/p99 latency, not averages, determines whether an interactive system feels stuck. |
| S26 | [Dinneen and Julien, The Ubiquitous Digital File](https://arxiv.org/abs/2109.09668) | File management remains under-supported; retrieval, organization, and sharing interact. |
| S27 | [Apple HIG: Drag and Drop](https://developer.apple.com/design/human-interface-guidelines/drag-and-drop) | Show valid targets, offer alternatives, and prefer undo for accidental drops. |
| S28 | [Apple HIG: Progress Indicators](https://developer.apple.com/design/human-interface-guidelines/progress-indicators) | Progress must move, stay truthful, retain position, and permit cancellation when safe. |
| S29 | [W3C WCAG 2.2](https://www.w3.org/TR/WCAG22/) | Focus visibility, drag alternatives, and target sizes are testable constraints. |
| S30 | [WAI-ARIA Grid Pattern](https://www.w3.org/WAI/ARIA/apg/patterns/grid/) | Dense tabular navigation needs a coherent focus and keyboard model. |

### Representative engineering documents

| ID | Source | Reusable pattern |
| --- | --- | --- |
| E01 | [Yazi feature contract](https://yazi-rs.github.io/) | Async I/O, priority scheduling, cancellation, and preloading. |
| E02 | [Zed: low-latency syntax-aware editing](https://zed.dev/blog/syntax-aware-editing) | Copy-on-write snapshots let background work proceed without blocking UI reads. |
| E03 | [VS Code extension host](https://code.visualstudio.com/api/advanced-topics/extension-host) | Lazy activation and process boundaries protect startup and UI latency. |
| E04 | [rclone bisync safety and recovery](https://rclone.org/bisync/) | Access checks, delete budgets, safe-state lockout, snapshots, and conflict copies. |
| E05 | [restic repository format](https://restic.readthedocs.io/en/v0.18.0/design.html) | Write ordering and immutable references define a recoverable commit. |
| E06 | [Syncthing file versioning](https://docs.syncthing.net/users/versioning?version=v2.0.0) | Replaced/deleted remote versions can be retained by policy. |
| E07 | [fzf](https://github.com/junegunn/fzf) | One streaming picker can combine query grammar, preview, and execution. |
| E08 | [Tantivy](https://github.com/quickwit-oss/tantivy) | Embedded facets and reader snapshots avoid a separate search service. |
| E09 | [egui](https://github.com/emilk/egui) | Immediate-mode identity and per-frame work need deliberate ownership. |
| E10 | [Iced](https://github.com/iced-rs/iced) | Typed messages separate state transitions from asynchronous commands. |
| E11 | [tracing](https://github.com/tokio-rs/tracing) | Structured operation spans survive thread and async boundaries. |
| E12 | [tus protocol server](https://github.com/tus/tusd) | Persisted offsets make interrupted transfer resumable. |
| E13 | [VS Code when-clause contexts](https://code.visualstudio.com/api/references/when-clause-contexts) | One context policy can drive command, menu, and keybinding availability. |
| E14 | [notify `Event`](https://docs.rs/notify/latest/notify/struct.Event.html) | `need_rescan` means incremental watcher state is no longer trustworthy. |
| E15 | [watchexec](https://watchexec.github.io/docs/) | Event batching and coalescing belong between noisy backends and consumers. |
| E16 | [rclone backend overview](https://rclone.org/overview/) | Operations must be selected from backend capabilities, not assumed globally. |

## Synthesis

The research rejects two tempting extremes. Commander should not become a
search-only semantic database: location and shallow browsing remain powerful
memory cues (S01-S06). It also should not remain a synchronous path browser:
the leading tools stream work, cancel obsolete generations, preserve immutable
snapshots, and treat every long operation as a recoverable state machine
(E01-E06).

Five design constraints follow:

1. **Keep paths visible while enriching them.** Facets, tags, time pivots, and
   virtual collections augment the dual-pane model; they do not replace it.
2. **Protect the foreground loop.** Listing, indexing, decoding, hashing, and
   filesystem probes receive budgets, priorities, cancellation, and stale-result
   rejection.
3. **Define completion end to end.** A successful syscall is not a successful
   user operation until the destination and operation manifest agree.
4. **Design every destructive action for recovery.** Undo eligibility,
   conflict copies, checkpoints, journals, and safe-state lockouts are parts of
   one recovery model.
5. **Keep the surface quiet and operational.** Stable geometry, dense rows,
   distinct focus/selection, truthful progress, and keyboard/pointer parity
   matter more than decorative novelty.

## Ideas already present and not counted again

The source review strongly supports the following existing Track D/E ideas,
but they are not disguised as new findings below: post-copy checksum
verification, the append-only crash journal, Sync/Delete dry-run, archive
browsing/extraction, capped preview workers, virtualized file rows, headless
egui tests, structured logging, persisted schema versions, a slow-volume
indicator, shallow/opt-out remote watchers, and generative `PanelState` tests.

## 100 new research-backed proposals

These entries began as hypotheses. Their wording and identifiers remain stable
while the implementation ledger below records delivery. `S` means small, `M`
medium, and `L` large relative effort.

### G001-G010: finding and re-finding

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G001 | Rank `Cmd+P` destinations by frecency while retaining a chronological mode. | repo 91; S02 | S |
| G002 | Preserve facets and breadcrumbs while moving between browse and search results. | S03-S05 | M |
| G003 | Represent active filters as editable query chips with one canonical query model. | S04; repos 43-45 | M |
| G004 | Add replayable query history with result-count and duration metadata. | repo 92; S02 | S |
| G005 | Keep result ordering stable by file identity when a query refreshes. | S02-S03 | M |
| G006 | Add a time pivot that groups a result set by modified/accessed period without moving files. | S02; repo 3 | M |
| G007 | Offer `Why matched?` details for fuzzy/content results and their score components. | repos 41, 43 | S |
| G008 | Add named project collections spanning roots as virtual, non-owning views. | S01, S08; repos 3, 9 | M |
| G009 | Add an interruptible compressed-tree overview for very large roots. | S06; repo 20 | M |
| G010 | Restore cursor, scroll anchor, and last focused child per directory, not only view settings. | S05-S06 | S |

### G011-G020: search and large-directory scale

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G011 | Stream first search results before the full walk completes. | repos 38-40; S09 | M |
| G012 | Give every query a generation token and cancel superseded scans on each edit. | repo 40; E01 | S |
| G013 | Schedule visible-row metadata and previews ahead of offscreen work. | E01; S25 | M |
| G014 | Make content indexing optional, root-scoped, idle-only, and inspectable. | S01-S05; E08 | L |
| G015 | Show index coverage, freshness, excluded roots, and last error next to indexed search. | E08; S15 | M |
| G016 | After archive browsing lands, expose archive members to the same search-provider contract. | repos 18, 59-61 | M |
| G017 | Support one documented query grammar for `path:`, `type:`, `size:`, `date:`, and `content:`. | repos 20, 36, 38 | M |
| G018 | Use a segmented Exact/Fuzzy/Regex mode with one stable result layout. | repos 5, 20, 38 | S |
| G019 | Add deterministic typo-tolerant ranking with golden fixtures for filenames. | repos 41, 43-45 | M |
| G020 | Benchmark search on shallow, deep, many-small, huge-file, and mixed personal corpora. | S07, S22, S25 | M |

### G021-G030: operation integrity

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G021 | Let users choose an operation durability profile: Fast, Verified, or Versioned. | S16, S19; repos 53-55 | M |
| G022 | Add an excessive-delete/change circuit breaker to synchronization plans. | E04 | S |
| G023 | Verify a configured health marker before applying changes to a remote/sync root. | E04; repo 52 | S |
| G024 | Fingerprint filters and comparison settings so stale sync baselines fail closed. | E04 | S |
| G025 | Abort before mutation when an implausible fraction of existing files appears changed. | E04 | S |
| G026 | Re-stat sources before final placement and requeue files modified during transfer. | E04; S24 | M |
| G027 | Detect destination changes between conflict scan and commit with an identity/version check. | S14, S24 | M |
| G028 | Classify failures as retryable, blocked, user-decision, or integrity-uncertain. | S15; E04 | S |
| G029 | Define graceful stop as finish-current-unit, checkpoint, then stop accepting work. | E04, E12 | M |
| G030 | Enter a visible safe state after uncertain failure and require review before replay. | S15; E04 | M |

### G031-G040: recovery, versioning, and undo

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G031 | Build a Recovery Center over the planned journal, with Resume, Roll back, and Inspect. | S15 | L |
| G032 | Add policy-based local versions for files replaced or deleted by non-local operations. | E06; repos 52, 55-56 | M |
| G033 | Compute selective-undo eligibility from current filesystem state, not stack position alone. | S13-S14 | L |
| G034 | Preview an undo's paths, conflicts, and irreversible gaps before executing it. | S13-S14 | M |
| G035 | Explain exactly why redo was invalidated and which later action caused it. | S13 | S |
| G036 | Let related queued jobs share an explicit transaction/group boundary in history. | S13; repo 95 | M |
| G037 | When rollback is partial, generate a concrete repair plan from completed steps. | S15 | M |
| G038 | Discover orphan staging/quarantine paths at startup and offer safe cleanup. | S15, S24 | M |
| G039 | Retry only failed manifest entries while proving completed entries unchanged. | S16; repos 51, 53 | M |
| G040 | Assign idempotency keys to operation attempts so recovery cannot duplicate an effect. | S15-S16 | M |

### G041-G050: transfer and sync performance

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G041 | Persist chunk/offset progress for resumable large-file and remote copies. | E12; repos 63, 68 | L |
| G042 | Offer delta copy when a large similar destination exists on a slow link. | S17-S18 | L |
| G043 | Enable content-defined chunking only above measured size/latency thresholds. | S20-S21 | L |
| G044 | Cache a typed capability profile per volume/backend with a short TTL. | repos 51, 78 | M |
| G045 | Tune transfer concurrency per volume from recent p95 latency and error rate. | S25; E01 | M |
| G046 | Reserve I/O budget for foreground listing/preview while background scans run. | E01; S09, S25 | M |
| G047 | Detect and preserve sparse-file layout through copy and verification. | S22; repo 51 | M |
| G048 | Record which fast path won: clone, rename, sparse, delta, or buffered copy. | S15; E11 | S |
| G049 | Add per-volume bandwidth limits and quiet hours for remote operations. | repos 51-52 | M |
| G050 | Cache verified hashes by stable identity, size, mtime, and filesystem generation. | S16, S21 | M |

### G051-G060: filesystem reality

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G051 | Preview Unicode normalization changes and persist the chosen collision policy. | E04; repo 77 | M |
| G052 | Add a portability audit for names invalid on common destination filesystems. | repos 51, 77 | M |
| G053 | Make symlink traversal a visible per-operation policy with a safe default. | repos 36, 51, 77 | S |
| G054 | Pause on mount disconnect and retain state until a bounded reconnect timeout. | S15; repos 51-52 | M |
| G055 | Key caches and sync baselines by volume identity so remounts cannot reuse stale state. | S24; repo 80 | M |
| G056 | Detect watcher overflow/gaps and replace incremental state with a full generation scan. | repos 79-80 | M |
| G057 | Introduce a path identity value carrying path, file ID, volume ID, and observed version. | S24; repo 52 | M |
| G058 | Classify roots as local-fast, local-slow, removable, or remote and expose the reason. | repos 51, 71 | S |
| G059 | Build a preflight capability matrix for read, write, rename, clone, trash, and xattrs. | repos 51, 78 | M |
| G060 | Run operation tests on case-sensitive/insensitive APFS, read-only, SMB/NFS, and disconnect fixtures. | S24; repos 51, 80 | L |

### G061-G070: operation UX

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G061 | Merge queue, history, errors, and recovery into one unframed Operations Center with tabs. | repos 2, 13, 55 | M |
| G062 | Show explicit Scan, Plan, Transfer, Verify, and Finalize phases with phase-aware ETA. | S28; repos 51, 55 | S |
| G063 | Transition unknown to known progress without changing the indicator's footprint. | S28 | S |
| G064 | Label cancellation by consequence: Stop after current file, Cancel pending, or Roll back. | S13, S28 | S |
| G065 | Show why a job is paused and the exact condition required to resume. | S15; E04 | S |
| G066 | Keep failed-operation notifications actionable until View, Retry, Undo, or Dismiss. | S15; repo 95 | M |
| G067 | Auto-scroll a valid drop container near its edges with bounded velocity. | S12, S27 | M |
| G068 | Add a keyboard command equivalent to dropping onto the highlighted subfolder. | S10, S27, S29 | S |
| G069 | Announce valid destination, move/copy effect, and rejection reason during drag. | S27, S29 | M |
| G070 | Freeze the submitted selection and destination summary into the running operation row. | S11, S13 | S |

### G071-G080: visual design and accessibility

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G071 | Use separate visual channels for active pane, keyboard focus, cursor, selection, and marks. | S29-S30 | M |
| G072 | Audit compact controls against a 24x24 logical-point baseline or equivalent spacing, documenting native exceptions. | S29 | S |
| G073 | Pair every semantic color with icon, shape, text, or pattern. | S29 | S |
| G074 | Expose file rows as named columns plus selection/expanded state to assistive tech. | S29-S30 | L |
| G075 | Add automated focus-order tests for panels, toolbars, dialogs, and operation views. | S29-S30; repo 98 | M |
| G076 | Respect reduced-motion settings for progress, drag feedback, and transitions. | S29 | S |
| G077 | Add high-contrast snapshots that cover focus, selection, diff, and disabled controls. | S29 | M |
| G078 | Test 200% text scale with stable row/control geometry and no overlap. | S29 | M |
| G079 | Ensure toast/dialog layers never obscure the current focus indicator or active error. | S29 | S |
| G080 | Maintain keyboard and single-pointer alternatives for every drag-only workflow. | S27, S29 | S |

### G081-G090: architecture and verification

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G081 | Define narrow `PreviewProvider`, `SearchProvider`, `FileSystemProvider`, and `Hasher` ports. | repos 7, 42, 51; E03 | L |
| G082 | Isolate optional third-party preview/index providers from the UI process. | E03; repo 84 | L |
| G083 | Lazily activate providers by root and file capability, with startup budgets. | E03 | M |
| G084 | Introduce one workload scheduler with priority, cancellation, quotas, and backpressure. | E01; S25 | L |
| G085 | Standardize immutable task snapshots and generation-based stale-result rejection. | E02; E08 | M |
| G086 | Express transfer/recovery as a serializable state machine with explicit terminal states. | S15; E04-E05 | L |
| G087 | Add deterministic scheduler tests that simulate latency, cancellation, and disconnect. | E02; repo 98 | M |
| G088 | Inject failures before and after every filesystem side effect in operation tests. | S15, S24 | L |
| G089 | Add a crash-kill harness that restarts at every journal transition and checks invariants. | S15, S24 | L |
| G090 | Model-check small copy/move/sync conflict state spaces against no-loss invariants. | S13-S16 | L |

### G091-G100: measurement and maintainability

| ID | Proposal | Basis | Effort |
| --- | --- | --- | ---: |
| G091 | Define CI budgets for startup, first listing, filter response, and operation-dialog latency. | S09, S25 | M |
| G092 | Track p50/p95/p99, cancellation latency, and stale-result count rather than averages alone. | S25 | M |
| G093 | Generate benchmark trees from empirical depth, fan-out, size, and type distributions. | S07, S22 | M |
| G094 | Instrument startup as named phases and fail a benchmark on unexplained regression. | E03; S25 | M |
| G095 | Add a developer budget panel for workers, queued I/O, cache bytes, and frame time. | E01, E09, E11 | M |
| G096 | Export a capability diagnostic that explains fast paths, fallbacks, and unavailable features. | repos 51, 71; E11 | S |
| G097 | Produce a redacted support bundle with operation spans, versions, and volume capabilities. | S15; E11 | M |
| G098 | Put risky index/preview providers behind runtime kill switches and bounded rollout flags. | E03 | M |
| G099 | Add Keystroke-Level Model fixtures for ten core workflows and reject avoidable operator growth. | S10 | S |
| G100 | Record architecture decisions for operation invariants, ownership, and failure policy beside code. | S15-S16, S24 | S |

## Implementation ledger

| Range | Status | Primary evidence |
| --- | --- | --- |
| G001-G010 | Implemented and tested | Frecency/history navigation, canonical filters, collections, compressed tree, and per-directory focus restoration |
| G011-G020 | Implemented and tested | Streaming cancellable search, stable identities/ranking, providers, inspectable content index, and bounded ZIP search |
| G021-G030 | Implemented and tested | Operation profiles, typed failures, revalidation, safe staging/commit, sync guard, and uncertain-failure safe state |
| G031-G040 | Implemented and tested | Durable idempotent journal, recovery center, versions, selective undo, repair, orphan cleanup, and manifest retry |
| G041-G050 | Implemented, tested, and reviewed in three passes | Resumable buffered/delta copy, adaptive volume policy, I/O budgets, sparse preservation, fast-path telemetry, and verified hash cache |
| G051-G060 | Implemented and tested | Filesystem identity/capability policy, remount checks, normalization boundaries, and safe refusal reasons |
| G061-G070 | Implemented and tested | Unified Operations Center, phase-aware progress/ETA, typed pause/cancel consequences, failure inbox, keyboard and drag transfer UX |
| G071-G080 | Implemented and tested | Separate visual channels, compact-control audit, assistive row semantics, focus tests, reduced motion, high contrast, 200% responsive geometry, protected overlays, and drag alternatives |
| G081-G090 | Implemented and tested | Narrow provider/filesystem/hash ports, process isolation and lazy activation, one quota scheduler, immutable task generations, serializable transition machines, deterministic fault/crash verification, and no-loss model checks |
| G091-G100 | Implemented, tested, and reviewed in three passes | Live CI budgets, bounded percentile telemetry, empirical fixtures, startup phases, developer diagnostics, capability/support exports, atomic kill switches, KLM workflow guards, and colocated architecture decisions |

All 100 proposals are now implemented. The ledger describes shipped ownership
and verification evidence rather than a future roadmap.

## 2026-07-18 comparative implementation pass

The refreshed cohort was evaluated against the current code rather than used
to generate another speculative feature pile. Twelve missing or incomplete
contracts were selected because they improve correctness, recovery, foreground
latency, or action clarity without widening the product surface.

| ID | Implemented delta | Primary evidence |
| --- | --- | --- |
| H001 | Compare file kind as part of the cross-pane fingerprint. | `compare`, `sync` |
| H002 | Represent folder pairs and file/folder collisions explicitly. | `compare`, compare UI |
| H003 | Make conditional conflict policies fail closed on type uncertainty. | `conflict`, `sync` |
| H004 | Recover valid siblings from partially damaged JSON stores. | `persistence`, config stores, `session` |
| H005 | Aggregate path-free persistence recovery diagnostics and surface them once. | `persistence`, developer diagnostics |
| H006 | Compute command availability from immutable workspace context snapshots. | `command`, `workspace` |
| H007 | Reuse availability in wide/compact toolbars and the command palette. | app toolbar and palette adapters |
| H008 | Show disabled reasons and make palette Enter choose the first enabled match. | command palette UI |
| H009 | Move all image decoding off the egui frame and stream fallback input. | `image_cache` |
| H010 | Bound decoded image buffers/dimensions and expose stable failure plus retry states. | `image_cache`, preview UI |
| H011 | Count watcher gaps, backend/start failures, reconnects, and reconciliations without paths. | `watcher_health`, support bundle schema 2 |
| H012 | Back off failed watchers and require a full reconciliation after gaps or reconnects. | `panel`, `watcher_health` |

The implementation deliberately preserves several distinctions learned from
the cohort: directories are never declared byte-identical from size/mtime;
type conflicts never silently inherit a file policy; one damaged persisted
record does not erase valid siblings; an unavailable action has one policy and
one explanation; preview failure is a stable retryable state rather than an
infinite spinner; and a watcher that failed to subscribe is never reported as
active.

### Next ten evidence-backed candidates

These remain proposals, ordered by expected value versus coupling. They are
not counted as implemented.

| Rank | Candidate | Why it remains separate |
| ---: | --- | --- |
| 1 | Decode-time image downsampling to the visible preview size. | The new cap prevents runaway allocation, but full-resolution decode can still waste CPU and memory. |
| 2 | A typed preview-provider fallback chain with timeout and health. | Current native/fallback paths are bounded, but provider selection is not yet inspectable per format. |
| 3 | Explain unavailable direct keyboard commands with the same reason as the palette. | Palette and toolbars are consistent; shortcut feedback still needs a low-noise policy. |
| 4 | Batch/coalesce watcher events before reconciliation and expose batch telemetry. | Recovery is correct now; event storms can still create avoidable listing work. |
| 5 | Select watcher depth and polling fallback from volume/backend capability. | One local policy cannot fit APFS, removable media, SMB, and NFS equally well. |
| 6 | Preserve filename extensions when compact rows truncate long names. | Dense editors and file managers keep the most decision-relevant suffix visible. |
| 7 | Show focused-pane contextual key help generated from command availability. | The registry now has enough policy data; the remaining work is a restrained UI treatment. |
| 8 | Replace the growing context struct with typed composable command predicates. | Useful only when more command dimensions appear; premature today, likely valuable later. |
| 9 | Gate operations by a cached per-volume capability matrix. | Transfer policy models capabilities, but all visible actions do not yet consume one shared matrix. |
| 10 | Expose conflict-copy/version retention as an operation policy. | Durability/versioning exists underneath; the user-facing recovery contract can be more explicit. |

## Original promotion candidates

Before implementation began, the best near-term value/risk ratio was **G001,
G004, G010, G012, G022-G025,
G028, G035, G048, G062-G065, G068, G072-G073, G079, G096, G099, and G100**.
This historical shortlist is retained to explain the initial sequencing. The
first milestone ultimately implemented all of `G001-G050`; the second
implemented all of `G051-G100`, keeping the architecture-sized `G057` and
`G084-G089` work aligned with their Track A owners.
