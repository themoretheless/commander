# Repository feature transfer qualification — 2026-10-27

Scope and source references: [implementation ledger](../../research/feature-transfers-2026-10-27.md).

| Check | Result |
| --- | --- |
| Full serial suite after tabs/source integration | 2200 passed, 3 intentionally ignored, no failures |
| Later scoped command checks | 84 passed |
| Template persistence/expansion checks | 21 passed |
| Recipe metadata/filter checks after final validation fix | 2 passed |
| Tab/session checks after active-index conversion hardening | 10 passed |
| Preview checks including Unicode budget and high-contrast controls | 59 passed |
| Native-harness checks including updated 8-scenario matrix | 3 passed |
| Final Clippy, all targets/features with warnings denied | Passed |
| Formatting and diff whitespace checks | Passed |

The full suite predates the later recipe/template work; it is not represented
as a full rerun of the final tree. Relevant scoped checks cover the subsequent
changes. Counts overlap and must not be added together as unique test coverage.
No dependency was added or changed in this batch.

## Native captures

Each screenshot has an accompanying manifest and capabilities report. All
three scenes report **14/14 checks passed** with zero fake native effect calls.
Screenshots were visually reviewed. Fixtures are disposable; user configuration
and user files are not the scene subject.

- [Folder tabs](folder_tabs/frame.png): three tabs in the left pane, third
  selected. Setup must successfully create both additional tabs; readiness
  checks count and active index before capture.
- [Source preview](code_preview/frame.png): an actual loaded Rust file, with
  Wrap lines and Syntax colors controls. Capture waits for text publication.
- [Minimum window](minimum_window/frame.png): 900×500 logical window with the
  sidebar and the new per-pane tab strips visible.

The first diagnostic folder-tabs scene only displayed the tab strip because
its fixture setup had not created extra tabs. It was not accepted as multi-tab
qualification. Setup and readiness were strengthened, then the three-tab scene
was rerun and visually inspected. An initial Clippy run also found a nested
conditional in the new template editor; it was collapsed and final Clippy passed.

```sh
cargo test --all-targets --all-features -- --test-threads=1
cargo test --all-targets --all-features command -- --test-threads=1
cargo test --all-targets --all-features cmdtemplate -- --test-threads=1
cargo test --all-targets --all-features command_recipes -- --test-threads=1
cargo test --all-targets --all-features tabs -- --test-threads=1
cargo test --all-targets --all-features preview -- --test-threads=1
cargo test --all-targets --all-features visual_qa -- --test-threads=1
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --features visual-qa -- --visual-qa folder_tabs --output target/visual-qa-repository-features
cargo run --features visual-qa -- --visual-qa code_preview --output target/visual-qa-repository-features
cargo run --features visual-qa -- --visual-qa minimum_window --output target/visual-qa-repository-features
```

The GUI captures establish these fixture layouts and preview readiness. They
do not certify VoiceOver workflows, hardware performance, installed-release
behavior or the unimplemented features of the 99 upstream repositories.
