use crate::app::{App, VisualQaSeed};
use crate::ports::{
    ClipboardOutcome, ClipboardPort, ContextMenuAction, ContextMenuPort, ContextMenuResult,
    FreeSpacePort, NativeFailure, OpenOutcome, OpenRequest, OpenerPort, SpacePrecision,
    SpaceProbeOutcome, TrashItemOutcome, TrashPort, TrashTarget, VolumeRelation,
};
use crate::theme::ThemeMode;
use egui::{ColorImage, Rect};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const QA_SCHEMA: u32 = 1;
const TIMEOUT: Duration = Duration::from_secs(10);
const CAPTURE_RETRY_AFTER: Duration = Duration::from_secs(2);
const MAX_CAPTURE_ATTEMPTS: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    DesktopBase,
    IconsGrid,
    ImagePreview,
    PdfPreview,
    Checksums,
    HexPreview,
    HexSearch,
    Permissions,
    RenamePresets,
    RenameTarget,
    RenameOptions,
    NameList,
    FolderTabs,
    CodePreview,
    UnicodePreview,
    RegexPreview,
    JsonPreview,
    ArchiveBrowser,
    MinimumWindow,
    Zoom200Accessible,
    ConfirmationOwner,
    /// Disk usage open as a pane tool: it replaces the right pane while the
    /// left pane stays live and nothing traps input.
    PaneTool,
}

impl Scenario {
    const ALL: [Self; 22] = [
        Self::DesktopBase,
        Self::IconsGrid,
        Self::ImagePreview,
        Self::PdfPreview,
        Self::Checksums,
        Self::HexPreview,
        Self::HexSearch,
        Self::Permissions,
        Self::RenamePresets,
        Self::RenameTarget,
        Self::RenameOptions,
        Self::NameList,
        Self::FolderTabs,
        Self::CodePreview,
        Self::UnicodePreview,
        Self::RegexPreview,
        Self::JsonPreview,
        Self::ArchiveBrowser,
        Self::MinimumWindow,
        Self::Zoom200Accessible,
        Self::ConfirmationOwner,
        Self::PaneTool,
    ];

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == value)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::DesktopBase => "desktop_base",
            Self::IconsGrid => "icons_grid",
            Self::ImagePreview => "image_preview",
            Self::PdfPreview => "pdf_preview",
            Self::Checksums => "checksums",
            Self::HexPreview => "hex_preview",
            Self::HexSearch => "hex_search",
            Self::Permissions => "permissions",
            Self::RenamePresets => "rename_presets",
            Self::RenameTarget => "rename_target",
            Self::RenameOptions => "rename_options",
            Self::NameList => "name_list",
            Self::FolderTabs => "folder_tabs",
            Self::CodePreview => "code_preview",
            Self::UnicodePreview => "unicode_preview",
            Self::RegexPreview => "regex_preview",
            Self::JsonPreview => "json_preview",
            Self::ArchiveBrowser => "archive_browser",
            Self::MinimumWindow => "minimum_window",
            Self::Zoom200Accessible => "zoom_200_accessible",
            Self::ConfirmationOwner => "confirmation_owner",
            Self::PaneTool => "pane_tool",
        }
    }

    const fn viewport(self) -> [f32; 2] {
        match self {
            Self::MinimumWindow | Self::Zoom200Accessible => [900.0, 500.0],
            Self::DesktopBase
            | Self::IconsGrid
            | Self::ImagePreview
            | Self::PdfPreview
            | Self::Checksums
            | Self::HexPreview
            | Self::HexSearch
            | Self::Permissions
            | Self::RenamePresets
            | Self::RenameTarget
            | Self::RenameOptions
            | Self::NameList
            | Self::FolderTabs
            | Self::CodePreview
            | Self::UnicodePreview
            | Self::RegexPreview
            | Self::JsonPreview
            | Self::ArchiveBrowser
            | Self::ConfirmationOwner
            | Self::PaneTool => [1280.0, 760.0],
        }
    }

    const fn native_window_size(self) -> [f32; 2] {
        let viewport = self.viewport();
        [viewport[0] * self.ui_scale(), viewport[1] * self.ui_scale()]
    }

    const fn ui_scale(self) -> f32 {
        match self {
            Self::Zoom200Accessible => 2.0,
            _ => 1.0,
        }
    }

    const fn theme(self) -> ThemeMode {
        match self {
            Self::MinimumWindow | Self::Zoom200Accessible => ThemeMode::Light,
            Self::DesktopBase
            | Self::IconsGrid
            | Self::ImagePreview
            | Self::PdfPreview
            | Self::Checksums
            | Self::HexPreview
            | Self::HexSearch
            | Self::Permissions
            | Self::RenamePresets
            | Self::RenameTarget
            | Self::RenameOptions
            | Self::NameList
            | Self::FolderTabs
            | Self::CodePreview
            | Self::UnicodePreview
            | Self::RegexPreview
            | Self::JsonPreview
            | Self::ArchiveBrowser
            | Self::ConfirmationOwner
            | Self::PaneTool => ThemeMode::Dark,
        }
    }

    const fn preferences(self) -> crate::accessibility::Preferences {
        crate::accessibility::Preferences {
            reduced_motion: matches!(self, Self::Zoom200Accessible),
            high_contrast: matches!(self, Self::Zoom200Accessible),
        }
    }

    const fn show_tree(self) -> bool {
        matches!(self, Self::MinimumWindow)
    }

    const fn modal_owner(self) -> Option<&'static str> {
        match self {
            Self::ConfirmationOwner => Some("Confirmation"),
            Self::Permissions => Some("Permissions"),
            Self::NameList => Some("Mask"),
            Self::RenamePresets | Self::RenameTarget | Self::RenameOptions => Some("BatchRename"),
            _ => None,
        }
    }

    const fn needs_confirmation(self) -> bool {
        matches!(self, Self::ConfirmationOwner)
    }

    const fn opens_pane_tool(self) -> bool {
        matches!(self, Self::PaneTool)
    }
}

struct Request {
    scenario: Scenario,
    output_root: PathBuf,
    allow_skip: bool,
}

impl Request {
    fn parse() -> Result<Option<Self>, String> {
        let mut args = std::env::args().skip(1);
        let Some(first) = args.next() else {
            return Ok(None);
        };
        if first != "--visual-qa" {
            return Ok(None);
        }
        let scenario = args
            .next()
            .and_then(|value| Scenario::parse(&value))
            .ok_or_else(|| {
                format!(
                    "--visual-qa requires one of: {}",
                    Scenario::ALL
                        .iter()
                        .map(|scenario| scenario.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let mut output_root = PathBuf::from("target/visual-qa");
        let mut allow_skip = false;
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--output" => {
                    output_root = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| "--output requires a path".to_string())?;
                }
                "--allow-skip" => allow_skip = true,
                other => return Err(format!("unknown visual QA argument: {other}")),
            }
        }
        Ok(Some(Self {
            scenario,
            output_root,
            allow_skip,
        }))
    }
}

pub fn maybe_run() -> Option<eframe::Result<()>> {
    if std::env::args().nth(1).as_deref() == Some("--native-release-qa") {
        let request = crate::native_release_qa::NativeQaRequest::parse(std::env::args().skip(2));
        return Some(match request.and_then(crate::native_release_qa::run) {
            Ok(()) => Ok(()),
            Err(error) => Err(app_error(error)),
        });
    }
    match Request::parse() {
        Ok(Some(request)) => Some(run(request)),
        Ok(None) => None,
        Err(error) => Some(Err(app_error(error))),
    }
}

fn run(request: Request) -> eframe::Result<()> {
    let scenario_dir = request.output_root.join(request.scenario.name());
    if scenario_dir.exists() {
        std::fs::remove_dir_all(&scenario_dir).map_err(|error| {
            app_error(format!(
                "could not reset stale QA artifacts {}: {error}",
                scenario_dir.display()
            ))
        })?;
    }
    std::fs::create_dir_all(&scenario_dir)
        .map_err(|error| app_error(format!("could not create QA artifact directory: {error}")))?;
    write_capabilities(
        &scenario_dir,
        CapabilityStatus::Attempted,
        "capture_attempted",
        "starting native eframe/Glow framebuffer capture",
        None,
    )
    .map_err(app_error)?;
    write_unfinished_manifest(&scenario_dir, request.scenario, "attempted", None)
        .map_err(app_error)?;

    if (request.scenario == Scenario::Permissions && !cfg!(unix))
        || (request.scenario == Scenario::PdfPreview && !cfg!(target_os = "macos"))
    {
        let detail = "This native preview scenario is unavailable on this host";
        write_capabilities(
            &scenario_dir,
            CapabilityStatus::Unsupported,
            "unsupported_platform",
            detail,
            None,
        )
        .map_err(app_error)?;
        write_unfinished_manifest(&scenario_dir, request.scenario, "skipped", Some(detail))
            .map_err(app_error)?;
        return if request.allow_skip {
            Ok(())
        } else {
            Err(app_error(detail))
        };
    }

    let fixture = Fixture::create(request.scenario).map_err(app_error)?;
    crate::fs_util::install_storage_root_override(fixture.root.join("state"))
        .map_err(|_| app_error("visual QA storage override was already installed"))?;

    let viewport = request.scenario.native_window_size();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("Commander Visual QA: {}", request.scenario.name()))
            .with_inner_size(viewport)
            .with_min_inner_size(viewport)
            .with_max_inner_size(viewport)
            .with_resizable(false)
            .with_titlebar_shown(false)
            .with_fullsize_content_view(true),
        persist_window: false,
        run_and_return: true,
        centered: true,
        ..Default::default()
    };
    let result_slot = Arc::new(Mutex::new(None));
    let app_result = Arc::clone(&result_slot);
    let scenario_dir_for_app = scenario_dir.clone();
    let scenario = request.scenario;
    let allow_skip = request.allow_skip;
    let run_result = eframe::run_native(
        "Commander Visual QA",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let app = VisualQaApp::new(
                cc,
                scenario,
                fixture,
                scenario_dir_for_app,
                app_result,
                allow_skip,
            )?;
            Ok(Box::new(app))
        }),
    );

    if let Err(error) = run_result {
        let detail = error.to_string();
        let capability_missing = capability_is_unavailable(&error);
        let status = if capability_missing {
            CapabilityStatus::Unsupported
        } else {
            CapabilityStatus::Failed
        };
        let failure_kind = if capability_missing {
            "unsupported_backend"
        } else {
            "eframe_runtime_failure"
        };
        write_capabilities(&scenario_dir, status, failure_kind, &detail, None)
            .map_err(app_error)?;
        write_unfinished_manifest(
            &scenario_dir,
            request.scenario,
            if capability_missing {
                "skipped"
            } else {
                "failed"
            },
            Some(&detail),
        )
        .map_err(app_error)?;
        if capability_missing && request.allow_skip {
            return Ok(());
        }
        return Err(error);
    }

    match result_slot
        .lock()
        .map_err(|_| app_error("visual QA result lock was poisoned"))?
        .take()
    {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(app_error(error)),
        None => Err(app_error(
            "visual QA window closed before a screenshot result was recorded",
        )),
    }
}

fn app_error(message: impl Into<String>) -> eframe::Error {
    eframe::Error::AppCreation(Box::new(std::io::Error::other(message.into())))
}

#[derive(Debug)]
enum VisualQaCreationError {
    UnsupportedBackend(String),
    Setup(String),
}

impl VisualQaCreationError {
    const fn unsupported(&self) -> bool {
        matches!(self, Self::UnsupportedBackend(_))
    }
}

impl fmt::Display for VisualQaCreationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedBackend(message) | Self::Setup(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for VisualQaCreationError {}

fn capability_is_unavailable(error: &eframe::Error) -> bool {
    match error {
        eframe::Error::AppCreation(source) => source
            .downcast_ref::<VisualQaCreationError>()
            .is_some_and(VisualQaCreationError::unsupported),
        eframe::Error::NoGlutinConfigs(_, _) => true,
        #[allow(unreachable_patterns)]
        _ => false,
    }
}

struct Fixture {
    root: PathBuf,
    left: PathBuf,
    right: PathBuf,
}

impl Fixture {
    fn create(scenario: Scenario) -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!(
            "commander-visual-qa-{}-{}",
            std::process::id(),
            scenario.name()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root)
                .map_err(|error| format!("could not reset owned QA fixture: {error}"))?;
        }
        let left = root.join("left");
        let right = root.join("right");
        std::fs::create_dir_all(left.join("Design"))
            .and_then(|()| std::fs::create_dir_all(right.join("Archive")))
            .map_err(|error| format!("could not create QA fixture: {error}"))?;

        let long_name = "quarterly-design-review-with-a-deliberately-long-filename-for-layout.txt";
        for (path, contents) in [
            (left.join("README.txt"), "Commander visual QA fixture\n"),
            (left.join(long_name), "long name\n"),
            (
                left.join("photo.png"),
                "not decoded in the base scenarios\n",
            ),
            (left.join("notes.md"), "# Notes\n- deterministic\n"),
            (right.join("invoice.pdf"), "fixture\n"),
            (right.join("release.zip"), "fixture\n"),
            (right.join("source.rs"), "fn main() {}\n"),
        ] {
            std::fs::write(path, contents)
                .map_err(|error| format!("could not write QA fixture: {error}"))?;
        }
        if matches!(scenario, Scenario::IconsGrid | Scenario::ImagePreview) {
            let (width, height) = if scenario == Scenario::ImagePreview {
                (96, 72)
            } else {
                (64, 48)
            };
            image::RgbaImage::from_fn(width, height, |x, y| {
                let alpha = if scenario == Scenario::ImagePreview && x < width / 3 {
                    0
                } else if scenario == Scenario::ImagePreview && x < width / 2 {
                    128
                } else {
                    255
                };
                image::Rgba([
                    ((x * 255) / width) as u8,
                    ((y * 255) / height) as u8,
                    160,
                    alpha,
                ])
            })
            .save(left.join("photo.png"))
            .map_err(|error| format!("could not create thumbnail fixture: {error}"))?;
        }
        if scenario == Scenario::HexSearch {
            let mut data = vec![0x11; 131104];
            for offset in [4094, 65534] {
                data[offset..offset + 4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
            }
            std::fs::write(left.join("bytes.bin"), data).map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::HexPreview {
            std::fs::write(
                left.join("bytes.bin"),
                (0..8197).map(|index| index as u8).collect::<Vec<_>>(),
            )
            .map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::FolderTabs {
            std::fs::write(
                left.join("Design/inside.txt"),
                "Folder opened in opposite pane",
            )
            .map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::RenameOptions {
            std::fs::write(left.join("TXT-TXT.txt"), "original bytes")
                .map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::RenameTarget {
            std::fs::write(left.join("item.txt"), "original bytes")
                .map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::Checksums {
            std::fs::write(left.join("checksum.txt"), "abc").map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::PdfPreview {
            std::fs::write(left.join("preview.pdf"), crate::pdf_preview::fixture())
                .map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::JsonPreview {
            std::fs::write(left.join("preview.json"), r#"{"z":9007199254740993123456789,"message":"café / ёж","nested":{"message":"second"},"z":-0.00e+007}"#).map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::RegexPreview {
            std::fs::write(
                left.join("regex.txt"),
                "item-12\nITEM-34\nxitem-56\n[abc]\n",
            )
            .map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::UnicodePreview {
            let mut bytes = vec![0xfe, 0xff];
            for unit in "UTF-16 preview\nmessage: café / ёж\nmessage: deuxième\n".encode_utf16()
            {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            std::fs::write(left.join("unicode.txt"), bytes).map_err(|error| error.to_string())?;
        }
        if scenario == Scenario::CodePreview {
            let code = r#"// Rust source preview: logical line numbers stay aligned when this intentionally long comment wraps. café / ёж.

fn main() {
    let message = "Hello, Commander";
    println!("{message}");
}
"#;
            let code = code.replace("    println!", "\tprintln!") + "// commander\n";
            std::fs::write(left.join("preview.rs"), code)
                .map_err(|error| format!("could not create code fixture: {error}"))?;
        }
        if scenario == Scenario::ArchiveBrowser {
            let create_archive = || -> std::io::Result<()> {
                let file = std::fs::File::create(left.join("sample.tar"))?;
                let mut builder = tar::Builder::new(file);
                for (name, bytes) in [
                    (
                        "docs/notes.txt",
                        b"Archive content search needle\n".as_slice(),
                    ),
                    ("src/main.rs", b"// A larger file deliberately sorts before docs when ordered by size.\nfn main() {}\n".as_slice()),
                    ("src/nested/helper.rs", b"// helper\n".as_slice()),
                    ("src/file10.rs", b"same".as_slice()),
                    ("src/file2.rs", b"same".as_slice()),
                    ("src/folder10/readme", b"x".as_slice()),
                    ("src/folder2/readme", b"x".as_slice()),
                ] {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(bytes.len() as u64);
                    header.set_mode(0o644);
                    header.set_mtime(37);
                    header.set_cksum();
                    builder.append_data(&mut header, name, bytes)?;
                }
                builder.finish()
            };
            create_archive().map_err(|error| format!("could not create TAR fixture: {error}"))?;
        }
        if scenario == Scenario::Permissions {
            let path = left.join("permissions.txt");
            std::fs::write(&path, "permission fixture\n").map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o640))
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(Self { root, left, right })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct EffectCounters {
    context_menu: AtomicUsize,
    deferred_menu: AtomicUsize,
    clipboard: AtomicUsize,
    clipboard_text: std::sync::Mutex<Option<String>>,
    opener: AtomicUsize,
    trash: AtomicUsize,
    free_space: AtomicUsize,
}

impl EffectCounters {
    fn snapshot(&self) -> NativeEffectCalls {
        NativeEffectCalls {
            context_menu: self.context_menu.load(Ordering::Relaxed),
            deferred_menu: self.deferred_menu.load(Ordering::Relaxed),
            clipboard: self.clipboard.load(Ordering::Relaxed),
            opener: self.opener.load(Ordering::Relaxed),
            trash: self.trash.load(Ordering::Relaxed),
            free_space: self.free_space.load(Ordering::Relaxed),
        }
    }
}

struct FakeContextMenu(Arc<EffectCounters>);

impl ContextMenuPort for FakeContextMenu {
    fn show_context_menu(
        &self,
        _invocation: &crate::ports::ContextMenuInvocation,
    ) -> ContextMenuResult {
        self.0.context_menu.fetch_add(1, Ordering::Relaxed);
        ContextMenuResult::Dismissed
    }

    fn perform_deferred_action(&self, _action: &ContextMenuAction) -> ContextMenuResult {
        self.0.deferred_menu.fetch_add(1, Ordering::Relaxed);
        ContextMenuResult::Dismissed
    }
}

struct FakeClipboard(Arc<EffectCounters>);

impl ClipboardPort for FakeClipboard {
    fn write_text(&self, text: &str) -> ClipboardOutcome {
        *self
            .0
            .clipboard_text
            .lock()
            .expect("recording clipboard lock") = Some(text.to_owned());
        self.0.clipboard.fetch_add(1, Ordering::Relaxed);
        ClipboardOutcome::Committed
    }
}

struct FakeOpener(Arc<EffectCounters>);

impl OpenerPort for FakeOpener {
    fn open(&self, _request: &OpenRequest) -> OpenOutcome {
        self.0.opener.fetch_add(1, Ordering::Relaxed);
        OpenOutcome::Accepted
    }
}

struct FakeTrash(Arc<EffectCounters>);

impl TrashPort for FakeTrash {
    fn move_to_trash(&self, _target: &TrashTarget) -> TrashItemOutcome {
        self.0.trash.fetch_add(1, Ordering::Relaxed);
        TrashItemOutcome::Unsupported(NativeFailure::unsupported(
            "visual QA never mutates fixture files",
        ))
    }
}

struct FakeFreeSpace(Arc<EffectCounters>);

impl FreeSpacePort for FakeFreeSpace {
    fn probe(&self, _path: &Path) -> SpaceProbeOutcome {
        self.0.free_space.fetch_add(1, Ordering::Relaxed);
        SpaceProbeOutcome::Known {
            bytes: 1 << 40,
            precision: SpacePrecision::Exact,
        }
    }

    fn volume_relation(&self, _source: &Path, _target: &Path) -> VolumeRelation {
        self.0.free_space.fetch_add(1, Ordering::Relaxed);
        VolumeRelation::Same
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ProbeId {
    LeftPane,
    RightPane,
    Confirmation,
    LeftRow,
}

impl ProbeId {
    const fn key(self) -> &'static str {
        match self {
            Self::LeftPane => "visual_qa_left_pane",
            Self::RightPane => "visual_qa_right_pane",
            Self::Confirmation => "visual_qa_confirmation",
            Self::LeftRow => "visual_qa_left_row",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ProbeRecord {
    rect: Rect,
    layer_id: egui::LayerId,
    enabled: bool,
    hovered: bool,
}

fn painted_glyph_key(glyph: crate::app::glyphs::PaintedGlyph) -> egui::Id {
    egui::Id::new(("visual_qa_painted_glyph", glyph.key()))
}

fn required_painted_glyphs(
    scenario: Scenario,
) -> impl Iterator<Item = crate::app::glyphs::PaintedGlyph> {
    crate::app::glyphs::PaintedGlyph::REQUIRED_CAPTURE
        .into_iter()
        .filter(move |glyph| {
            // Compact layouts intentionally move Compare from the toolbar into
            // the settings menu; every glyph that remains visible stays strict.
            // ConfirmationOwner covers the file-list document glyph with the
            // modal overlay, so framebuffer ink there is not required.
            !matches!(
                (scenario, glyph),
                (
                    Scenario::MinimumWindow | Scenario::Zoom200Accessible,
                    crate::app::glyphs::PaintedGlyph::ToolbarCompare
                ) | (
                    Scenario::ConfirmationOwner,
                    crate::app::glyphs::PaintedGlyph::FileDocument
                )
            )
        })
}

pub fn begin_probe_frame(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        for id in [
            ProbeId::LeftPane,
            ProbeId::RightPane,
            ProbeId::Confirmation,
            ProbeId::LeftRow,
        ] {
            data.remove::<ProbeRecord>(egui::Id::new(id.key()));
        }
        data.remove::<bool>(egui::Id::new("visual_qa_background_enabled"));
        data.remove::<String>(egui::Id::new("visual_qa_modal_owner"));
        for glyph in crate::app::glyphs::PaintedGlyph::REQUIRED_CAPTURE {
            data.remove::<Rect>(painted_glyph_key(glyph));
        }
    });
}

pub fn record_response(ctx: &egui::Context, id: ProbeId, response: &egui::Response) {
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(id.key()),
            ProbeRecord {
                rect: response.rect,
                layer_id: response.layer_id,
                enabled: response.enabled(),
                hovered: response.hovered(),
            },
        );
    });
}

pub fn record_painted_glyph(
    ctx: &egui::Context,
    glyph: crate::app::glyphs::PaintedGlyph,
    rect: Rect,
) {
    ctx.data_mut(|data| {
        // The contract checks one real instance of each glyph. Keep the first
        // painted instance instead of replacing it with a duplicate from the
        // other pane that may be occluded by a centered modal.
        let key = painted_glyph_key(glyph);
        if data.get_temp::<Rect>(key).is_none() {
            data.insert_temp(key, rect);
        }
    });
}

pub fn record_input_policy(
    ctx: &egui::Context,
    background_enabled: bool,
    modal_owner: Option<String>,
) {
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new("visual_qa_background_enabled"),
            background_enabled,
        );
        if let Some(owner) = modal_owner {
            data.insert_temp(egui::Id::new("visual_qa_modal_owner"), owner);
        }
    });
}

#[derive(Clone, Debug)]
struct ProbeSnapshot {
    viewport: Rect,
    pixels_per_point: f32,
    left_pane: Option<ProbeRecord>,
    right_pane: Option<ProbeRecord>,
    confirmation: Option<ProbeRecord>,
    left_row: Option<ProbeRecord>,
    background_enabled: Option<bool>,
    modal_owner: Option<String>,
    modal_above_background: Option<bool>,
    painted_glyphs: BTreeMap<crate::app::glyphs::PaintedGlyph, Rect>,
}

impl ProbeSnapshot {
    fn read(ctx: &egui::Context) -> Self {
        let viewport = ctx.input(|input| input.viewport_rect());
        let pixels_per_point = ctx.pixels_per_point();
        let (left_pane, right_pane, confirmation, left_row, background_enabled, modal_owner) = ctx
            .data_mut(|data| {
                (
                    data.get_temp::<ProbeRecord>(egui::Id::new(ProbeId::LeftPane.key())),
                    data.get_temp::<ProbeRecord>(egui::Id::new(ProbeId::RightPane.key())),
                    data.get_temp::<ProbeRecord>(egui::Id::new(ProbeId::Confirmation.key())),
                    data.get_temp::<ProbeRecord>(egui::Id::new(ProbeId::LeftRow.key())),
                    data.get_temp::<bool>(egui::Id::new("visual_qa_background_enabled")),
                    data.get_temp::<String>(egui::Id::new("visual_qa_modal_owner")),
                )
            });
        let modal_above_background = confirmation
            .zip(left_pane)
            .map(|(modal, pane)| modal.layer_id.order > pane.layer_id.order);
        let painted_glyphs = ctx.data_mut(|data| {
            crate::app::glyphs::PaintedGlyph::REQUIRED_CAPTURE
                .into_iter()
                .filter_map(|glyph| {
                    data.get_temp::<Rect>(painted_glyph_key(glyph))
                        .map(|rect| (glyph, rect))
                })
                .collect()
        });
        Self {
            viewport,
            pixels_per_point,
            left_pane,
            right_pane,
            confirmation,
            left_row,
            background_enabled,
            modal_owner,
            modal_above_background,
            painted_glyphs,
        }
    }

    fn ready(&self, scenario: Scenario) -> bool {
        self.left_pane.is_some()
            && self.right_pane.is_some()
            && required_painted_glyphs(scenario)
                .all(|glyph| self.painted_glyphs.contains_key(&glyph))
            && (if let Some(owner) = scenario.modal_owner() {
                self.confirmation.is_some()
                    && self.background_enabled == Some(false)
                    && self.modal_owner.as_deref() == Some(owner)
                    && self.modal_above_background == Some(true)
            } else {
                self.left_row.is_some_and(|row| row.enabled && row.hovered)
            })
    }

    fn geometry_key(&self) -> Vec<i32> {
        let mut values = Vec::new();
        for rect in [
            Some(self.viewport),
            self.left_pane.map(|record| record.rect),
            self.right_pane.map(|record| record.rect),
            self.confirmation.map(|record| record.rect),
            self.left_row.map(|record| record.rect),
        ]
        .into_iter()
        .flatten()
        {
            values.extend(
                [rect.min.x, rect.min.y, rect.max.x, rect.max.y]
                    .map(|value| (value * 4.0).round() as i32),
            );
        }
        for rect in self.painted_glyphs.values() {
            values.extend(
                [rect.min.x, rect.min.y, rect.max.x, rect.max.y]
                    .map(|value| (value * 4.0).round() as i32),
            );
        }
        values
    }
}

#[derive(Clone, Debug)]
struct ScreenshotToken {
    scenario: &'static str,
    request_id: u64,
}

#[derive(Clone, Debug, Serialize)]
struct RendererDiagnostics {
    renderer: &'static str,
    adapter_name: String,
    backend: String,
    device_type: String,
    target_format: String,
}

struct VisualQaApp {
    inner: App,
    scenario: Scenario,
    fixture: Fixture,
    scenario_dir: PathBuf,
    result: Arc<Mutex<Option<Result<(), String>>>>,
    counters: Arc<EffectCounters>,
    started: Instant,
    frames: u32,
    stable_frames: u8,
    last_geometry: Vec<i32>,
    capture_attempts: u8,
    next_request_id: u64,
    last_request_at: Option<Instant>,
    capture_probes: HashMap<u64, ProbeSnapshot>,
    confirmation_requested: bool,
    pane_tool_opened: bool,
    pdf_click_phase: u8,
    image_click_phase: u8,
    checksum_click_phase: u8,
    hex_click_phase: u8,
    hex_search_phase: u8,
    regex_click_phase: u8,
    text_size_click_phase: u8,
    tab_group_phase: u8,
    selection_groups_applied: bool,
    name_list_phase: u8,
    json_mode_phase: u8,
    preset_click_phase: u8,
    rename_target_phase: u8,
    rename_options_phase: u8,
    last_probe_summary: String,
    pending_screenshot: Option<(u64, Arc<ColorImage>)>,
    pending_event_error: Option<String>,
    renderer: RendererDiagnostics,
}

impl VisualQaApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        scenario: Scenario,
        fixture: Fixture,
        scenario_dir: PathBuf,
        result: Arc<Mutex<Option<Result<(), String>>>>,
        _allow_capture_skip: bool,
    ) -> Result<Self, VisualQaCreationError> {
        let counters = Arc::new(EffectCounters::default());
        if cc.gl.is_none() {
            return Err(VisualQaCreationError::UnsupportedBackend(
                "Glow render state is unavailable; native framebuffer readback requires OpenGL"
                    .to_string(),
            ));
        }
        let renderer = RendererDiagnostics {
            renderer: "glow",
            adapter_name: "system OpenGL context".to_string(),
            backend: "OpenGL".to_string(),
            device_type: "native_window".to_string(),
            target_format: "RGBA8 framebuffer readback".to_string(),
        };
        write_capabilities(
            &scenario_dir,
            CapabilityStatus::Attempted,
            "renderer_ready",
            "Glow renderer initialized; waiting for a stable native frame",
            Some(&renderer),
        )
        .map_err(VisualQaCreationError::Setup)?;
        let seed = VisualQaSeed {
            left: fixture.left.clone(),
            right: fixture.right.clone(),
            ui_scale: scenario.ui_scale(),
            theme_mode: scenario.theme(),
            accessibility_preferences: scenario.preferences(),
            show_tree: scenario.show_tree(),
        };
        let mut inner = App::new_visual_qa(
            cc,
            seed,
            Rc::new(FakeContextMenu(Arc::clone(&counters))),
            Rc::new(FakeClipboard(Arc::clone(&counters))),
            Rc::new(FakeOpener(Arc::clone(&counters))),
            Arc::new(FakeTrash(Arc::clone(&counters))),
            Arc::new(FakeFreeSpace(Arc::clone(&counters))),
        );
        if scenario == Scenario::FolderTabs {
            let opened_design = inner.ws.new_tab(
                crate::workspace::ActivePanel::Left,
                Some(fixture.left.join("Design")),
            );
            let opened_root = inner.ws.new_tab(
                crate::workspace::ActivePanel::Left,
                Some(fixture.left.clone()),
            );
            let closed_root = inner.ws.close_tab(crate::workspace::ActivePanel::Left);
            let reopened_root = inner
                .ws
                .reopen_closed_tab(crate::workspace::ActivePanel::Left);
            let opened_extra = inner.ws.new_tab(
                crate::workspace::ActivePanel::Left,
                Some(fixture.left.join("Design")),
            );
            let closed_extra = inner.ws.close_tab(crate::workspace::ActivePanel::Left);
            if !opened_design
                || !opened_root
                || !closed_root
                || !reopened_root
                || !opened_extra
                || !closed_extra
            {
                return Err(VisualQaCreationError::Setup(
                    "could not prepare three tabs and a recently closed tab".to_string(),
                ));
            }
        }
        if scenario == Scenario::IconsGrid {
            inner
                .ws
                .left
                .set_browser_mode(crate::panel::BrowserMode::Icons);
        }
        Ok(Self {
            inner,
            scenario,
            fixture,
            scenario_dir,
            result,
            counters,
            started: Instant::now(),
            frames: 0,
            stable_frames: 0,
            last_geometry: Vec::new(),
            capture_attempts: 0,
            next_request_id: 1,
            last_request_at: None,
            capture_probes: HashMap::new(),
            confirmation_requested: false,
            pane_tool_opened: false,
            pdf_click_phase: 0,
            image_click_phase: 0,
            checksum_click_phase: 0,
            hex_click_phase: 0,
            hex_search_phase: 0,
            regex_click_phase: 0,
            text_size_click_phase: 0,
            tab_group_phase: 0,
            selection_groups_applied: false,
            name_list_phase: 0,
            json_mode_phase: 0,
            preset_click_phase: 0,
            rename_target_phase: 0,
            rename_options_phase: 0,
            last_probe_summary: "no rendered frame yet".to_string(),
            pending_screenshot: None,
            pending_event_error: None,
            renderer,
        })
    }

    fn finish(&self, ctx: &egui::Context, result: Result<(), String>) {
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(result);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn fail(&self, ctx: &egui::Context, failure_kind: &'static str, message: String) {
        let _ = write_capabilities(
            &self.scenario_dir,
            CapabilityStatus::Failed,
            failure_kind,
            &message,
            Some(&self.renderer),
        );
        let _ =
            write_unfinished_manifest(&self.scenario_dir, self.scenario, "failed", Some(&message));
        self.finish(ctx, Err(message));
    }

    fn screenshot_event(
        events: &[egui::Event],
        scenario: Scenario,
    ) -> Result<Option<(u64, Arc<ColorImage>)>, String> {
        for event in events {
            let egui::Event::Screenshot {
                viewport_id,
                user_data,
                image,
            } = event
            else {
                continue;
            };
            let token = user_data
                .data
                .as_ref()
                .and_then(|data| data.downcast_ref::<ScreenshotToken>())
                .ok_or_else(|| "screenshot event returned unknown user_data".to_string())?;
            if *viewport_id != egui::ViewportId::ROOT {
                return Err(format!(
                    "screenshot event returned unexpected viewport {viewport_id:?}"
                ));
            }
            if token.scenario != scenario.name() {
                return Err(format!(
                    "screenshot event token belongs to {}, expected {}",
                    token.scenario,
                    scenario.name()
                ));
            }
            return Ok(Some((token.request_id, Arc::clone(image))));
        }
        Ok(None)
    }

    fn request_screenshot(&mut self, ctx: &egui::Context, probes: ProbeSnapshot) {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.capture_attempts = self.capture_attempts.saturating_add(1);
        self.last_request_at = Some(Instant::now());
        self.capture_probes.insert(request_id, probes);
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            ScreenshotToken {
                scenario: self.scenario.name(),
                request_id,
            },
        )));
        ctx.request_repaint();
    }
}

impl eframe::App for VisualQaApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if self.pending_screenshot.is_none() {
            match Self::screenshot_event(&input.events, self.scenario) {
                Ok(screenshot) => self.pending_screenshot = screenshot,
                Err(error) => self.pending_event_error = Some(error),
            }
        }
        if self.scenario.modal_owner().is_none()
            && !(self.scenario == Scenario::PdfPreview && self.pdf_click_phase < 9)
            && !(self.scenario == Scenario::ImagePreview && self.image_click_phase < 21)
            && !(self.scenario == Scenario::FolderTabs && self.tab_group_phase < 81)
            && !(self.scenario == Scenario::JsonPreview && self.json_mode_phase < 9)
            && !(self.scenario == Scenario::CodePreview && self.text_size_click_phase < 27)
            && !(self.scenario == Scenario::Checksums && self.checksum_click_phase < 12)
            && !(self.scenario == Scenario::HexPreview && self.hex_click_phase < 17)
            && !(self.scenario == Scenario::HexSearch && self.hex_search_phase < 13)
            && !(self.scenario == Scenario::RegexPreview && self.regex_click_phase < 6)
            && self.capture_probes.is_empty()
            && let Some(row) = ProbeSnapshot::read(ctx).left_row
        {
            input
                .events
                .push(egui::Event::PointerMoved(row.rect.center()));
        }
        if self.scenario == Scenario::NameList && self.name_list_phase < 13 {
            let phase = self.name_list_phase;
            let preview = ctx
                .data(|data| data.get_temp::<Option<usize>>(egui::Id::new("mask-names-preview")))
                .flatten();
            if (phase == 7 && preview != Some(3)) || (phase == 10 && preview != Some(2)) {
                return;
            }
            let (key, local) = match phase {
                0..=2 => (egui::Id::new("mask-names-toggle"), phase),
                3..=6 => (egui::Id::new("mask-names-input"), phase - 3),
                7..=9 => (egui::Id::new(("mask-selection-choice", "Files")), phase - 7),
                _ => (
                    egui::Id::new(("mask-selection-choice", "Folders")),
                    phase - 10,
                ),
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if phase == 6 {
                    input.events.push(egui::Event::Text(
                        "notes.md\nREADME.txt\nDesign\nmissing.txt".into(),
                    ));
                } else if !local.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: local % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                self.name_list_phase += 1;
            }
        }
        if self.scenario == Scenario::FolderTabs && (75..81).contains(&self.tab_group_phase) {
            let phase = self.tab_group_phase;
            let side = crate::workspace::ActivePanel::Left;
            let key = if phase < 78 {
                egui::Id::new(("folder-tab-anchor", side, 1usize))
            } else {
                egui::Id::new(("folder-tab-deduplicate", side))
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                let local = if phase < 78 { phase - 75 } else { phase - 78 };
                if local > 0 {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: if phase < 78 {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        },
                        pressed: local == 1,
                        modifiers: Default::default(),
                    });
                }
                self.tab_group_phase += 1;
            }
        }
        if self.scenario == Scenario::FolderTabs && (65..75).contains(&self.tab_group_phase) {
            let phase = self.tab_group_phase;
            let side = crate::workspace::ActivePanel::Left;
            if phase == 71 {
                input.events.push(egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                });
                self.tab_group_phase += 1;
            } else {
                if phase == 72 {
                    input.events.push(egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: Default::default(),
                    });
                }
                let key = if (68..71).contains(&phase) {
                    egui::Id::new(("folder-tab-protect", side))
                } else {
                    egui::Id::new(("folder-tab-anchor", side, 1usize))
                };
                if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                    let pos = rect.center();
                    input.events.push(egui::Event::PointerMoved(pos));
                    let local = if phase < 68 {
                        phase - 65
                    } else if phase < 71 {
                        phase - 68
                    } else {
                        phase - 72
                    };
                    if local > 0 {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: if phase < 68 {
                                egui::PointerButton::Secondary
                            } else if phase < 71 {
                                egui::PointerButton::Primary
                            } else {
                                egui::PointerButton::Middle
                            },
                            pressed: local == 1,
                            modifiers: Default::default(),
                        });
                    }
                    self.tab_group_phase += 1;
                }
            }
        }
        if self.scenario == Scenario::FolderTabs && (55..65).contains(&self.tab_group_phase) {
            let phase = self.tab_group_phase;
            let side = crate::workspace::ActivePanel::Left;
            let key = if phase < 58 {
                egui::Id::new(("folder-tab-anchor", side, 1usize))
            } else if phase < 62 {
                egui::Id::new(("folder-tab-title-input", side))
            } else {
                egui::Id::new(("folder-tab-title-save", side))
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if phase == 61 {
                    input.events.push(egui::Event::Text("Work".into()));
                } else {
                    let local = if phase < 58 {
                        phase - 55
                    } else if phase < 62 {
                        phase - 58
                    } else {
                        phase - 62
                    };
                    if local > 0 {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: if phase < 58 {
                                egui::PointerButton::Secondary
                            } else {
                                egui::PointerButton::Primary
                            },
                            pressed: local == 1,
                            modifiers: Default::default(),
                        });
                    }
                }
                self.tab_group_phase += 1;
            }
        }
        if self.scenario == Scenario::FolderTabs && (49..55).contains(&self.tab_group_phase) {
            let phase = self.tab_group_phase;
            let side = crate::workspace::ActivePanel::Right;
            let key = if phase < 52 {
                egui::Id::new(("folder-tab-anchor", side, 1usize))
            } else {
                egui::Id::new(("folder-tab-close-left", side))
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                let local = if phase < 52 { phase - 49 } else { phase - 52 };
                if local > 0 {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: if phase < 52 {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        },
                        pressed: local == 1,
                        modifiers: Default::default(),
                    });
                }
                self.tab_group_phase += 1;
            }
        }
        if self.scenario == Scenario::FolderTabs && (43..49).contains(&self.tab_group_phase) {
            let phase = self.tab_group_phase;
            let side = crate::workspace::ActivePanel::Right;
            let key = if phase < 46 {
                egui::Id::new(("folder-tab-anchor", side, 2usize))
            } else {
                egui::Id::new(("folder-tab-transfer", side))
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                let local = if phase < 46 { phase - 43 } else { phase - 46 };
                if local > 0 {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: if phase < 46 {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        },
                        pressed: local == 1,
                        modifiers: Default::default(),
                    });
                }
                self.tab_group_phase += 1;
            }
        }
        if self.scenario == Scenario::FolderTabs && (37..43).contains(&self.tab_group_phase) {
            let phase = self.tab_group_phase;
            let side = crate::workspace::ActivePanel::Right;
            let key = if phase < 40 {
                egui::Id::new(("folder-tab-anchor", side, 1usize))
            } else {
                egui::Id::new(("folder-tab-duplicate", side))
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                let local = if phase < 40 { phase - 37 } else { phase - 40 };
                if local > 0 {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: if phase < 40 {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        },
                        pressed: local == 1,
                        modifiers: Default::default(),
                    });
                }
                self.tab_group_phase += 1;
            }
        }
        if self.scenario == Scenario::FolderTabs && (34..37).contains(&self.tab_group_phase) {
            let key = egui::Id::new((
                "folder-tab-anchor",
                crate::workspace::ActivePanel::Right,
                2usize,
            ));
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if self.tab_group_phase > 34 {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Middle,
                        pressed: self.tab_group_phase == 35,
                        modifiers: Default::default(),
                    });
                }
                self.tab_group_phase += 1;
            }
        }
        if self.scenario == Scenario::FolderTabs && (30..34).contains(&self.tab_group_phase) {
            use crate::command::Command;
            let home = dirs::home_dir();
            let root = self.fixture.left.ancestors().last().unwrap();
            match self.tab_group_phase {
                30 => {
                    let modifiers = egui::Modifiers {
                        command: true,
                        mac_cmd: true,
                        shift: true,
                        ..Default::default()
                    };
                    input.events.push(egui::Event::ModifiersChanged(modifiers));
                    input.events.push(egui::Event::Key {
                        key: egui::Key::H,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    });
                    self.tab_group_phase = 31;
                }
                31 if home.as_ref() == Some(&self.inner.ws.left.current_path) => {
                    input.events.push(egui::Event::Key {
                        key: egui::Key::H,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: Default::default(),
                    });
                    input
                        .events
                        .push(egui::Event::ModifiersChanged(Default::default()));
                    self.inner.ws.execute(Command::JumpBack);
                    self.tab_group_phase = 32;
                }
                32 if self.inner.ws.left.current_path == self.fixture.left => {
                    self.inner.ws.execute(Command::GoRoot);
                    self.tab_group_phase = 33;
                }
                33 if self.inner.ws.left.current_path == root => {
                    self.inner.ws.execute(Command::JumpBack);
                    self.tab_group_phase = 34;
                }
                _ => {}
            }
        }
        if self.scenario == Scenario::FolderTabs && self.tab_group_phase < 30 {
            let phase = self.tab_group_phase;
            let side = crate::workspace::ActivePanel::Left;
            if phase == 6 && self.inner.ws.tab_count(side) != 2 {
                return;
            }
            if phase == 12 {
                if self.inner.ws.tab_count(side) != 1 {
                    return;
                }
                self.inner.ws.left.focus_child("Design".into());
                if !self
                    .inner
                    .ws
                    .left
                    .cursor_entry()
                    .is_some_and(|entry| entry.name == "Design")
                {
                    return;
                }
            }
            if phase == 18 && self.inner.ws.tab_count(side) != 2 {
                return;
            }
            if phase == 24 && self.inner.ws.tab_count(side.other()) != 3 {
                return;
            }
            let key = match phase {
                24..=26 => egui::Id::new(("folder-tab-anchor", side, 0usize)),
                27..=29 => egui::Id::new(("folder-open-other", side)),
                18..=20 => egui::Id::new(("folder-tab-anchor", side, 1usize)),
                21..=23 => egui::Id::new(("folder-tabs-copy-other", side)),
                12..=14 => egui::Id::new(("folder-tab-new", side)),
                15..=17 => egui::Id::new(("folder-tab-open-background", side)),
                0..=2 => egui::Id::new(("folder-tab-anchor", side, 1usize)),
                3..=5 => egui::Id::new(("folder-tab-close-right", side)),
                6..=8 => egui::Id::new(("folder-tab-anchor", side, 0usize)),
                _ => egui::Id::new(("folder-tab-close-others", side)),
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if !phase.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: if phase < 3
                            || (6..9).contains(&phase)
                            || (12..15).contains(&phase)
                            || (18..21).contains(&phase)
                            || (24..27).contains(&phase)
                        {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        },
                        pressed: phase % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                self.tab_group_phase += 1;
            }
        }
        if self.scenario == Scenario::JsonPreview && self.json_mode_phase < 9 {
            let phase = self.json_mode_phase;
            if phase == 3
                && ctx
                    .data(|data| data.get_temp::<bool>(egui::Id::new(("document_source", "right"))))
                    != Some(true)
            {
                return;
            }
            let key = if phase < 6 {
                egui::Id::new(("json-preview-mode", "right", phase < 3))
            } else {
                egui::Id::new(("copy-preview-text", "right"))
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if !phase.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                self.json_mode_phase += 1;
            }
        }
        if self.scenario == Scenario::CodePreview && (9..13).contains(&self.text_size_click_phase) {
            let id = egui::Id::new(("text_preview", "right")).with("find");
            let current = ctx
                .data(|data| data.get_temp::<crate::preview_find::PreviewFind>(id))
                .and_then(|find| find.current_match());
            let phase = self.text_size_click_phase;
            if phase == 9 || phase == 11 || current == Some(if phase == 10 { 1 } else { 0 }) {
                let modifiers = egui::Modifiers {
                    shift: phase == 11,
                    ..Default::default()
                };
                input.events.push(egui::Event::ModifiersChanged(modifiers));
                input.events.push(egui::Event::Key {
                    key: egui::Key::F3,
                    physical_key: None,
                    pressed: phase == 9 || phase == 11,
                    repeat: false,
                    modifiers,
                });
                self.text_size_click_phase += 1;
            }
        }
        if self.scenario == Scenario::CodePreview && self.text_size_click_phase < 9 {
            let key = if self.text_size_click_phase < 3 {
                egui::Id::new(("text_preview", "right")).with("A+")
            } else if self.text_size_click_phase < 6 {
                egui::Id::new(("text_preview", "right"))
                    .with("whitespace")
                    .with("control")
            } else {
                egui::Id::new(("text_preview", "right"))
                    .with("find")
                    .with("query-control")
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if !self.text_size_click_phase.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.text_size_click_phase % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                self.text_size_click_phase += 1;
            }
        }
        if self.scenario == Scenario::CodePreview && (13..16).contains(&self.text_size_click_phase)
        {
            let key = egui::Id::new(("text_preview", "right"))
                .with("find")
                .with("wrap-toggle");
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if self.text_size_click_phase > 13 {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.text_size_click_phase == 14,
                        modifiers: Default::default(),
                    });
                }
                self.text_size_click_phase += 1;
            }
        }
        if self.scenario == Scenario::CodePreview && (16..24).contains(&self.text_size_click_phase)
        {
            let phase = self.text_size_click_phase;
            let find_id = egui::Id::new(("text_preview", "right")).with("find");
            if phase == 22 {
                let modifiers = egui::Modifiers {
                    command: true,
                    mac_cmd: true,
                    ..Default::default()
                };
                input.events.push(egui::Event::ModifiersChanged(modifiers));
                input.events.push(egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
                self.text_size_click_phase += 1;
            } else if phase == 23 {
                input
                    .events
                    .push(egui::Event::ModifiersChanged(Default::default()));
                input.events.push(egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: Default::default(),
                });
                input.events.push(egui::Event::Text("Commander".into()));
                self.text_size_click_phase += 1;
            } else {
                let key = find_id.with(if phase < 19 {
                    "smart-case-toggle"
                } else {
                    "query-control"
                });
                if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                    let pos = rect.center();
                    input.events.push(egui::Event::PointerMoved(pos));
                    let click = if phase < 19 { phase - 16 } else { phase - 19 };
                    if click > 0 {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: click == 1,
                            modifiers: Default::default(),
                        });
                    }
                    self.text_size_click_phase += 1;
                }
            }
        }
        if self.scenario == Scenario::CodePreview && (24..27).contains(&self.text_size_click_phase)
        {
            let phase = self.text_size_click_phase;
            let find_id = egui::Id::new(("text_preview", "right")).with("find");
            let ready = ctx.data(|data| {
                data.get_temp::<crate::preview_find::PreviewFind>(find_id)
                    .is_some_and(|find| find.match_count() == 1 && find.smart_case_enabled())
            });
            if ready
                && let Some(rect) =
                    ctx.data(|data| data.get_temp::<egui::Rect>(find_id.with("clear-query")))
            {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if phase > 24 {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 25,
                        modifiers: Default::default(),
                    });
                }
                self.text_size_click_phase += 1;
            }
        }
        if self.scenario == Scenario::Checksums && self.checksum_click_phase < 12 {
            let phase = self.checksum_click_phase;
            if phase == 4
                && !self.inner.checksum_comparison_is(
                    crate::checksum::Algorithm::Sha256,
                    crate::checksum::ExpectedMatch::Match,
                )
            {
                return;
            }
            if phase == 7
                && !self.inner.checksum_comparison_is(
                    crate::checksum::Algorithm::Blake3,
                    crate::checksum::ExpectedMatch::Mismatch,
                )
            {
                return;
            }
            let key = if (4..7).contains(&phase) {
                egui::Id::new(("checksum-algorithm", "BLAKE3"))
            } else {
                egui::Id::new("checksum-expected")
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                match phase {
                    3 => input.events.push(egui::Event::Text(
                        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                    )),
                    10 => input.events.push(egui::Event::Key {
                        key: egui::Key::A,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers {
                            command: true,
                            mac_cmd: true,
                            ..Default::default()
                        },
                    }),
                    11 => input.events.push(egui::Event::Text(
                        "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85".into(),
                    )),
                    1 | 2 | 5 | 6 | 8 | 9 => input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: matches!(phase, 1 | 5 | 8),
                        modifiers: Default::default(),
                    }),
                    _ => {}
                }
                self.checksum_click_phase += 1;
            }
        }
        if self.scenario == Scenario::RegexPreview && self.regex_click_phase < 6 {
            let find_id = egui::Id::new(("text_preview", "right")).with("find");
            if self.regex_click_phase == 3
                && !ctx
                    .data(|data| data.get_temp::<crate::preview_find::PreviewFind>(find_id))
                    .is_some_and(|find| find.regex_ready() && find.match_count() == 3)
            {
                return;
            }
            let key = find_id.with(if self.regex_click_phase < 3 {
                "regex-toggle"
            } else {
                "whole-words-toggle"
            });
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if !self.regex_click_phase.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.regex_click_phase % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                self.regex_click_phase += 1;
            }
        }
        if self.scenario == Scenario::HexSearch && self.hex_search_phase < 13 {
            let viewer = self.inner.hex_viewer.as_ref();
            let ready = if self.hex_search_phase < 7 {
                viewer.and_then(crate::hex_preview::HexViewer::shown_offset) == Some(0)
            } else if self.hex_search_phase < 10 {
                viewer.and_then(crate::hex_preview::HexViewer::matched_offset) == Some(4094)
            } else {
                viewer.and_then(crate::hex_preview::HexViewer::matched_offset) == Some(65534)
            };
            if ready {
                let (key, phase) = if self.hex_search_phase < 4 {
                    ("hex-query", self.hex_search_phase)
                } else if self.hex_search_phase < 7 {
                    ("hex-find-next", self.hex_search_phase - 4)
                } else if self.hex_search_phase < 10 {
                    ("hex-find-next", self.hex_search_phase - 7)
                } else {
                    ("hex-find-previous", self.hex_search_phase - 10)
                };
                if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new(key)))
                {
                    let pos = rect.center();
                    input.events.push(egui::Event::PointerMoved(pos));
                    if self.hex_search_phase == 3 {
                        input.events.push(egui::Event::Text("de ad be ef".into()));
                    } else if !phase.is_multiple_of(3) {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: phase % 3 == 1,
                            modifiers: Default::default(),
                        });
                    }
                    self.hex_search_phase += 1;
                }
            }
        }
        if self.scenario == Scenario::HexPreview && self.hex_click_phase < 17 {
            let shown = self
                .inner
                .hex_viewer
                .as_ref()
                .and_then(crate::hex_preview::HexViewer::shown_offset);
            let expected = if self.hex_click_phase >= 14 {
                8192
            } else if self.hex_click_phase < 3 {
                0
            } else if self.hex_click_phase < 6 {
                4096
            } else {
                0
            };
            if shown == Some(expected) {
                let (key, phase) = if self.hex_click_phase < 3 {
                    ("hex-next", self.hex_click_phase)
                } else if self.hex_click_phase < 6 {
                    ("hex-previous", self.hex_click_phase - 3)
                } else if self.hex_click_phase < 11 {
                    ("hex-offset", self.hex_click_phase - 6)
                } else if self.hex_click_phase < 14 {
                    ("hex-go", self.hex_click_phase - 11)
                } else {
                    ("Decimal", self.hex_click_phase - 14)
                };
                let key = if self.hex_click_phase >= 14 {
                    egui::Id::new(("hex-byte-format", key))
                } else {
                    egui::Id::new(key)
                };
                if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                    let pos = rect.center();
                    input.events.push(egui::Event::PointerMoved(pos));
                    if self.hex_click_phase == 9 {
                        input.events.push(egui::Event::Key {
                            key: egui::Key::A,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers {
                                command: true,
                                mac_cmd: true,
                                ..Default::default()
                            },
                        });
                    } else if self.hex_click_phase == 10 {
                        input.events.push(egui::Event::Text("0x2000".into()));
                    } else if !phase.is_multiple_of(3) {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: phase % 3 == 1,
                            modifiers: Default::default(),
                        });
                    }
                    self.hex_click_phase += 1;
                }
            }
        }
        if self.scenario == Scenario::ImagePreview && self.image_click_phase < 21 {
            if self.image_click_phase == 9
                && !crate::image_preview::black_background_active(ctx, "right")
            {
                return;
            }
            let label = if self.image_click_phase < 3 {
                "Rotate right"
            } else if self.image_click_phase < 6 {
                "Mirror H"
            } else if self.image_click_phase < 9 {
                "Black"
            } else if self.image_click_phase < 12 {
                "Checkerboard"
            } else if self.image_click_phase < 15 {
                "Enlarge small images"
            } else if self.image_click_phase < 18 {
                "Center image"
            } else {
                "Rotate 180°"
            };
            let key = if self.image_click_phase < 12 || self.image_click_phase >= 18 {
                egui::Id::new(("image_preview_view", "right")).with(label)
            } else {
                egui::Id::new(("image-preview-control", "right", label))
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if !self.image_click_phase.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.image_click_phase % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                self.image_click_phase += 1;
            }
        }
        if self.scenario == Scenario::RenameOptions && self.rename_options_phase < 40 {
            let phase = self.rename_options_phase;
            let validated = self
                .inner
                .ui
                .modals
                .batch_rename
                .as_ref()
                .and_then(|state| state.validation.as_ref());
            let ready = match phase {
                0..=2 => true,
                3..=5 => validated.is_some_and(|validation| {
                    validation.options_preview(true, false, "doc-doc.doc")
                }),
                6..=8 => validated.is_some_and(|validation| {
                    validation.options_preview(true, true, "doc-TXT.txt")
                }),
                9..=11 => validated.is_some_and(|validation| {
                    validation
                        .extension_case_preview(crate::rename::ExtensionCase::Upper, "doc-TXT.TXT")
                }),
                12..=14 => validated.is_some_and(|validation| {
                    validation
                        .extension_case_preview(crate::rename::ExtensionCase::Lower, "doc-TXT.txt")
                }),
                15..=17 => validated.is_some_and(|validation| {
                    validation.options_preview(true, true, "doc-TXT-03.txt")
                }),
                18..=20 => validated.is_some_and(|validation| {
                    validation.options_preview(true, true, "-03doc-TXT.txt")
                }),
                21..=23 => validated
                    .is_some_and(|validation| validation.options_preview(true, true, "-03.txt")),
                24..=26 => {
                    validated.is_some_and(|validation| validation.reset_preview("TXT-TXT.txt"))
                }
                27..=36 => validated.is_some_and(|validation| {
                    validation.options_preview(false, false, "TXT-TXT.txt")
                }),
                _ => validated
                    .is_some_and(|validation| validation.options_preview(false, false, "TXT.txt")),
            };
            if ready {
                let key = match phase {
                    0..=2 => egui::Id::new("rename-ignore-case"),
                    3..=5 => egui::Id::new("rename-first-only"),
                    6..=8 => egui::Id::new(("rename-extension-case", "UPPER")),
                    9..=11 => egui::Id::new(("rename-extension-case", "lower")),
                    12..=14 => egui::Id::new("rename-numbering"),
                    15..=17 => egui::Id::new(("rename-number-position", "Before name")),
                    18..=20 => egui::Id::new(("rename-number-position", "Counter only")),
                    21..=23 => egui::Id::new("rename-reset-rule"),
                    24..=26 => egui::Id::new("rename-trim-spaces"),
                    27..=29 => egui::Id::new("rename-collapse-repeats"),
                    30..=32 => egui::Id::new(("rename-normalization", "NFC")),
                    33..=36 => egui::Id::new("rename-remove-prefix"),
                    _ => egui::Id::new("rename-stem-limit"),
                };
                if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                    let pos = rect.center();
                    input.events.push(egui::Event::PointerMoved(pos));
                    if phase == 36 {
                        input.events.push(egui::Event::Text("TXT-".into()));
                    } else if !phase.is_multiple_of(3) {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: phase % 3 == 1,
                            modifiers: Default::default(),
                        });
                    }
                    self.rename_options_phase += 1;
                }
            }
        }
        if self.scenario == Scenario::RenameTarget && self.rename_target_phase < 12 {
            let stem_ready = self.rename_target_phase != 6
                || self
                    .inner
                    .ui
                    .modals
                    .batch_rename
                    .as_ref()
                    .and_then(|state| state.validation.as_ref())
                    .is_some_and(|validation| {
                        validation.target_preview(crate::rename::ReplaceTarget::Stem, "item.txt")
                    });
            if stem_ready {
                let key =
                    if self.rename_target_phase < 3 || (6..9).contains(&self.rename_target_phase) {
                        egui::Id::new("rename-replace-target-combo")
                    } else {
                        egui::Id::new((
                            "rename-replace-target-option",
                            if self.rename_target_phase < 6 {
                                "Name stem"
                            } else {
                                "Extension"
                            },
                        ))
                    };
                if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                    let pos = rect.center();
                    input.events.push(egui::Event::PointerMoved(pos));
                    if !self.rename_target_phase.is_multiple_of(3) {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: self.rename_target_phase % 3 == 1,
                            modifiers: Default::default(),
                        });
                    }
                    self.rename_target_phase += 1;
                }
            }
        }
        if self.scenario == Scenario::RenamePresets && self.preset_click_phase < 10 {
            let key = if self.preset_click_phase < 3 {
                egui::Id::new("rename-preset-combo")
            } else if self.preset_click_phase < 6 {
                egui::Id::new(("rename-preset-row", "Photos"))
            } else {
                egui::Id::new("rename-preset-name")
            };
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(key)) {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if self.preset_click_phase == 9 {
                    input.events.push(egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    });
                } else if !self.preset_click_phase.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.preset_click_phase % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                self.preset_click_phase += 1;
            }
        }
        if self.scenario == Scenario::PdfPreview && self.pdf_click_phase < 9 {
            let forward = self.pdf_click_phase < 3;
            let expected_page = if self.pdf_click_phase < 3 || self.pdf_click_phase >= 6 {
                1
            } else {
                2
            };
            if self.inner.pdf_cache.shown_page("right") == Some(expected_page)
                && let Some(rect) = ctx.data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        if self.pdf_click_phase >= 6 {
                            "pdf-text-mode"
                        } else if forward {
                            "pdf-next"
                        } else {
                            "pdf-previous"
                        },
                        "right",
                    )))
                })
            {
                let pos = rect.center();
                input.events.push(egui::Event::PointerMoved(pos));
                if !self.pdf_click_phase.is_multiple_of(3) {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.pdf_click_phase % 3 == 1,
                        modifiers: Default::default(),
                    });
                }
                log::info!("PDF QA pointer phase {} at {pos:?}", self.pdf_click_phase);
                self.pdf_click_phase += 1;
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(error) = self.pending_event_error.take() {
            self.fail(&ctx, "screenshot_event_failure", error);
            return;
        }
        let screenshot = self.pending_screenshot.take().or_else(|| {
            ctx.input(|input| Self::screenshot_event(&input.events, self.scenario))
                .ok()
                .flatten()
        });
        if let Some((request_id, image)) = screenshot {
            let Some(probes) = self.capture_probes.remove(&request_id) else {
                self.fail(
                    &ctx,
                    "screenshot_token_mismatch",
                    "screenshot arrived without probe metadata".to_string(),
                );
                return;
            };
            match write_capture(
                &self.scenario_dir,
                self.scenario,
                &image,
                &probes,
                self.counters.snapshot(),
            ) {
                Ok(()) => {
                    let _ = write_capabilities(
                        &self.scenario_dir,
                        CapabilityStatus::Captured,
                        "capture_validated",
                        "native eframe/Glow framebuffer captured and validated",
                        Some(&self.renderer),
                    );
                    self.finish(&ctx, Ok(()));
                }
                Err(error) => {
                    let _ = write_capabilities(
                        &self.scenario_dir,
                        CapabilityStatus::Failed,
                        "capture_validation_failure",
                        &error,
                        Some(&self.renderer),
                    );
                    self.finish(&ctx, Err(error));
                }
            }
            return;
        }

        self.frames = self.frames.saturating_add(1);
        if self.started.elapsed() > TIMEOUT {
            let message = format!(
                "screenshot event timed out after {} frames and {:.1}s ({} attempts): {}",
                self.frames,
                self.started.elapsed().as_secs_f32(),
                self.capture_attempts,
                self.last_probe_summary,
            );
            self.fail(&ctx, "capture_timeout", message);
            return;
        }

        eframe::App::ui(&mut self.inner, ui, frame);

        if self.scenario.needs_confirmation() && !self.confirmation_requested {
            let first = self
                .inner
                .ws
                .left
                .entries()
                .first()
                .map(|entry| entry.path.clone());
            if let Some(path) = first {
                self.inner.ws.left.replace_selection([path]);
                self.inner.ws.request_delete();
                self.confirmation_requested = true;
                self.stable_frames = 0;
                ctx.request_repaint();
                return;
            }
        }

        if self.scenario.opens_pane_tool() && !self.pane_tool_opened {
            if !self.inner.ws.left.entries().is_empty() {
                self.inner.open_treemap(&ctx);
                self.pane_tool_opened = true;
                self.stable_frames = 0;
            }
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::ArchiveBrowser && !self.pane_tool_opened {
            self.inner.ws.active = crate::workspace::ActivePanel::Left;
            self.inner
                .open_archive(self.fixture.left.join("sample.tar"), &ctx);
            self.pane_tool_opened = true;
            self.stable_frames = 0;
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::ArchiveBrowser
            && let Some(state) = self.inner.ui.tools.archive.as_mut()
            && state.listing.is_some()
            && state.member_filter.order.field != crate::archive_order::Field::Size
        {
            // Listing publication resets its row cache: choose the fixture's
            // sort only after publication, then wait for a rendered sorted frame.
            state.member_filter.navigate(Some(PathBuf::from("src")));
            state.selected.insert(0); // docs/notes.txt stays selected outside this folder.
            state
                .member_filter
                .order
                .choose(crate::archive_order::Field::Size);
            self.stable_frames = 0;
            ctx.request_repaint();
            return;
        }

        if matches!(self.scenario, Scenario::HexPreview | Scenario::HexSearch)
            && !self.pane_tool_opened
        {
            self.inner.ws.left.focus_child("bytes.bin".into());
            if self
                .inner
                .ws
                .left
                .cursor_entry()
                .is_some_and(|entry| entry.name == "bytes.bin")
            {
                self.inner.ws.active = crate::workspace::ActivePanel::Left;
                self.inner.ws.execute(crate::command::Command::ViewHex);
                self.pane_tool_opened = true;
            }
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::Checksums && !self.pane_tool_opened {
            let path = self.fixture.left.join("checksum.txt");
            self.inner.ws.left.focus_child("checksum.txt".into());
            self.inner.start_checksum(vec![path], &ctx);
            self.pane_tool_opened = true;
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::NameList && !self.pane_tool_opened {
            if !self.inner.ws.left.entries().is_empty() {
                self.inner.ws.active = crate::workspace::ActivePanel::Left;
                self.inner.ws.left.clear_selection();
                self.inner.open_mask(&ctx);
                self.inner.ui.modals.mask_input = Some(String::new());
                self.pane_tool_opened = true;
            }
            ctx.request_repaint();
            return;
        }
        if self.scenario == Scenario::RenameOptions && !self.pane_tool_opened {
            self.inner.ws.left.focus_child("TXT-TXT.txt".into());
            if self
                .inner
                .ws
                .left
                .cursor_entry()
                .is_some_and(|entry| entry.name == "TXT-TXT.txt")
            {
                self.inner.ws.active = crate::workspace::ActivePanel::Left;
                self.inner.ws.left.clear_selection();
                self.inner
                    .ws
                    .left
                    .toggle_select(self.fixture.left.join("TXT-TXT.txt"));
                self.inner.open_batch_rename(&ctx);
                if let Some(state) = self.inner.ui.modals.batch_rename.as_mut() {
                    state.find = "txt".into();
                    state.replace = "doc".into();
                    state.num_start = -3;
                    state.num_step = -2;
                    state.num_pad = 3;
                }
                self.pane_tool_opened = true;
            }
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::RenameTarget && !self.pane_tool_opened {
            self.inner.ws.left.focus_child("item.txt".into());
            if self
                .inner
                .ws
                .left
                .cursor_entry()
                .is_some_and(|entry| entry.name == "item.txt")
            {
                self.inner.ws.active = crate::workspace::ActivePanel::Left;
                self.inner.ws.left.clear_selection();
                self.inner
                    .ws
                    .left
                    .toggle_select(self.fixture.left.join("item.txt"));
                self.inner.open_batch_rename(&ctx);
                if let Some(state) = self.inner.ui.modals.batch_rename.as_mut() {
                    state.find = "txt".into();
                    state.replace = "log".into();
                }
                self.pane_tool_opened = true;
            }
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::RenamePresets
            && !self.pane_tool_opened
            && !self.inner.ws.left.entries().is_empty()
        {
            self.inner.ws.active = crate::workspace::ActivePanel::Left;
            self.inner
                .ws
                .left
                .toggle_select(self.fixture.left.join("notes.md"));
            self.inner
                .ws
                .left
                .toggle_select(self.fixture.left.join("README.txt"));
            self.inner
                .rename_presets
                .add(
                    "Photos",
                    crate::rename::RenameRule {
                        prefix: "Photo_".into(),
                        numbering: Some(crate::rename::Numbering {
                            start: 1,
                            step: 1,
                            pad: 2,
                        }),
                        ..Default::default()
                    },
                )
                .expect("valid fixture preset");
            self.inner.open_batch_rename(&ctx);
            self.pane_tool_opened = true;
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::Permissions && !self.pane_tool_opened {
            self.inner.ws.left.focus_child("permissions.txt".into());
            if let Some(entry) = self
                .inner
                .ws
                .left
                .cursor_entry()
                .filter(|entry| entry.name == "permissions.txt")
            {
                let target = crate::permissions::Target {
                    path: entry.path.clone(),
                    identity: entry.identity.clone(),
                    capture_on_worker: false,
                };
                self.inner.open_permissions(vec![target], &ctx);
                self.pane_tool_opened = true;
            }
            ctx.request_repaint();
            return;
        }
        if self.scenario == Scenario::Permissions
            && let Some(state) = self.inner.ui.modals.permissions.as_mut()
            && state
                .plan
                .as_ref()
                .is_some_and(crate::permissions::Plan::ready)
        {
            state.change = crate::permissions::Change::exact(0o644);
        }

        if self.scenario == Scenario::PdfPreview && !self.pane_tool_opened {
            self.inner.ws.left.focus_child("preview.pdf".into());
            if self
                .inner
                .ws
                .left
                .cursor_entry()
                .is_some_and(|entry| entry.name == "preview.pdf")
            {
                self.inner.ws.active = crate::workspace::ActivePanel::Left;
                self.inner
                    .ws
                    .execute(crate::command::Command::TogglePreview);
                self.pane_tool_opened = true;
            }
            ctx.request_repaint();
            return;
        }

        if self.scenario == Scenario::ImagePreview && !self.pane_tool_opened {
            self.inner.ws.left.focus_child("photo.png".to_string());
            if self
                .inner
                .ws
                .left
                .cursor_entry()
                .is_some_and(|entry| entry.name == "photo.png")
            {
                self.inner.ws.active = crate::workspace::ActivePanel::Left;
                crate::image_preview::seed(
                    &ctx,
                    "right",
                    &self.fixture.left.join("photo.png"),
                    2.0,
                );
                self.inner
                    .ws
                    .execute(crate::command::Command::TogglePreview);
                self.pane_tool_opened = true;
            }
            ctx.request_repaint();
            return;
        }

        if matches!(
            self.scenario,
            Scenario::CodePreview
                | Scenario::UnicodePreview
                | Scenario::RegexPreview
                | Scenario::JsonPreview
        ) && !self.pane_tool_opened
        {
            let filename = if self.scenario == Scenario::JsonPreview {
                "preview.json"
            } else if self.scenario == Scenario::RegexPreview {
                "regex.txt"
            } else if self.scenario == Scenario::UnicodePreview {
                "unicode.txt"
            } else {
                "preview.rs"
            };
            if !self.inner.ws.left.entries().is_empty() {
                self.inner.ws.left.focus_child(filename.to_string());
                if self
                    .inner
                    .ws
                    .left
                    .cursor_entry()
                    .is_some_and(|entry| entry.name == filename)
                {
                    self.inner.ws.active = crate::workspace::ActivePanel::Left;
                    ctx.data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new(("text_preview", "right")).with("find"),
                            crate::preview_find::PreviewFind::with_query(
                                if self.scenario == Scenario::RegexPreview {
                                    r"item-\d+"
                                } else {
                                    "message"
                                },
                            ),
                        )
                    });
                    self.inner
                        .ws
                        .execute(crate::command::Command::TogglePreview);
                    self.pane_tool_opened = true;
                }
                self.stable_frames = 0;
            }
            ctx.request_repaint();
            return;
        }
        if self.scenario == Scenario::PdfPreview
            && self.pdf_click_phase == 9
            && self.inner.pdf_cache.shown_page("right") == Some(1)
        {
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::new(("pdf-page-text", "right")).with("find"),
                    crate::preview_find::PreviewFind::with_query("Commander"),
                )
            });
            self.pdf_click_phase = 10;
        }
        let probes = ProbeSnapshot::read(&ctx);
        let listings_ready =
            !self.inner.ws.left.entries().is_empty() && !self.inner.ws.right.entries().is_empty();
        self.last_probe_summary = format!(
            "attempts={}, pending={}, stable={}, listings={}/{}, probes={probes:?}, {}",
            self.capture_attempts,
            self.capture_probes.len(),
            self.stable_frames,
            self.inner.ws.left.entries().len(),
            self.inner.ws.right.entries().len(),
            if self.scenario == Scenario::PdfPreview {
                format!(
                    "PDF phase {}: {}",
                    self.pdf_click_phase,
                    self.inner.pdf_cache.diagnostic("right")
                )
            } else {
                String::new()
            },
        );
        if self.scenario == Scenario::PdfPreview
            && self.pdf_click_phase == 10
            && ctx.data(|data| {
                data.get_temp::<crate::preview_find::PreviewFind>(
                    egui::Id::new(("pdf-page-text", "right")).with("find"),
                )
                .is_some_and(|find| find.match_count() == 1)
            })
            && self.inner.pdf_cache.shown_page("right") == Some(1)
            && self.stable_frames == 0
        {
            log::info!(
                "PDF native QA: pointer round trip 1 -> 2 -> 1 and Page text/search validated"
            );
        }
        let thumbnail_ready = self.scenario != Scenario::IconsGrid
            || self
                .inner
                .image_cache
                .get(&self.fixture.left.join("photo.png"))
                .is_some();
        let scenario_state_ready = match self.scenario {
            Scenario::JsonPreview => {
                self.json_mode_phase == 9
                    && ctx.data(|data| {
                        data.get_temp::<bool>(egui::Id::new(("document_source", "right")))
                    }) == Some(false)
                    && self.counters.clipboard.load(Ordering::Relaxed) == 1
                    && self
                        .counters
                        .clipboard_text
                        .lock()
                        .expect("recorded clipboard")
                        .as_deref()
                        == crate::json_preview::shown_format(&ctx, "right").as_deref()
                    && crate::json_preview::shown_format(&ctx, "right").is_some_and(|text| {
                        text.lines().count() > 1
                            && text.contains("9007199254740993123456789")
                            && text.matches("\"z\"").count() == 2
                    })
                    && ctx
                        .data(|data| {
                            data.get_temp::<crate::preview_find::PreviewFind>(
                                egui::Id::new(("text_preview", "right")).with("find"),
                            )
                        })
                        .is_some_and(|find| find.match_count() == 2)
            }

            Scenario::NameList => {
                self.name_list_phase == 13
                    && self.inner.ui.modals.mask_input.as_deref()
                        == Some("notes.md\nREADME.txt\nDesign\nmissing.txt")
                    && self.inner.ws.left.selected_count() == 0
                    && ctx
                        .data(|data| {
                            data.get_temp::<Option<usize>>(egui::Id::new("mask-names-preview"))
                        })
                        .flatten()
                        == Some(1)
            }
            Scenario::IconsGrid => {
                if !self.selection_groups_applied
                    && self
                        .inner
                        .ws
                        .left
                        .entries()
                        .iter()
                        .any(|entry| entry.name == "photo.png")
                {
                    self.inner.ws.left.focus_child("photo.png".into());
                    if self
                        .inner
                        .ws
                        .left
                        .cursor_entry()
                        .is_some_and(|entry| entry.name == "photo.png")
                    {
                        self.inner
                            .ws
                            .execute(crate::command::Command::SelectVisibleFiles);
                        self.inner
                            .ws
                            .execute(crate::command::Command::DeselectLikeCursor);
                        self.inner
                            .ws
                            .execute(crate::command::Command::SelectVisibleFolders);
                        self.selection_groups_applied = true;
                    }
                }
                self.selection_groups_applied
                    && self.inner.ws.left.selected_count() == 4
                    && self.inner.ws.left.entries().iter().all(|entry| {
                        self.inner.ws.left.is_selected(&entry.path)
                            == (entry.is_dir || entry.extension != "png")
                    })
            }

            Scenario::HexSearch => {
                self.hex_search_phase == 13
                    && self
                        .inner
                        .hex_viewer
                        .as_ref()
                        .and_then(crate::hex_preview::HexViewer::matched_offset)
                        == Some(4094)
            }
            Scenario::HexPreview => {
                self.hex_click_phase == 17
                    && self
                        .inner
                        .hex_viewer
                        .as_ref()
                        .is_some_and(crate::hex_preview::HexViewer::decimal_mode)
                    && self
                        .inner
                        .hex_viewer
                        .as_ref()
                        .and_then(crate::hex_preview::HexViewer::shown_offset)
                        == Some(8192)
            }
            Scenario::Checksums => {
                self.checksum_click_phase == 12
                    && self.inner.checksum_comparison_is(
                        crate::checksum::Algorithm::Blake3,
                        crate::checksum::ExpectedMatch::Match,
                    )
            }
            Scenario::RenameOptions => {
                self.rename_options_phase == 40
                    && self
                        .inner
                        .ui
                        .modals
                        .batch_rename
                        .as_ref()
                        .is_some_and(|state| {
                            state.rule()
                                == (crate::rename::RenameRule {
                                    remove_prefix: "TXT-".into(),
                                    stem_limit: Some(32),
                                    trim_spaces: true,
                                    collapse_repeats: true,
                                    normalization: crate::rename::NameNormalization::Nfc,
                                    ..Default::default()
                                })
                                && (state.num_start, state.num_step, state.num_pad) == (1, 1, 2)
                                && state.preset_selected.is_none()
                                && state.validation.as_ref().is_some_and(|validation| {
                                    validation.options_preview(false, false, "TXT.txt")
                                })
                        })
                    && self.fixture.left.join("TXT-TXT.txt").exists()
                    && !self.fixture.left.join("-03doc-TXT.txt").exists()
            }
            Scenario::RenameTarget => {
                self.rename_target_phase == 12
                    && self
                        .inner
                        .ui
                        .modals
                        .batch_rename
                        .as_ref()
                        .and_then(|state| state.validation.as_ref())
                        .is_some_and(|validation| {
                            validation
                                .target_preview(crate::rename::ReplaceTarget::Extension, "item.log")
                        })
                    && self.fixture.left.join("item.txt").exists()
                    && !self.fixture.left.join("item.log").exists()
            }
            Scenario::RenamePresets => {
                self.preset_click_phase == 10
                    && self
                        .inner
                        .ui
                        .modals
                        .batch_rename
                        .as_ref()
                        .is_some_and(|state| {
                            state.preset_selected.as_deref() == Some("Photos")
                                && state.rule().prefix == "Photo_"
                                && state.rule().numbering.is_some()
                        })
                    && self.fixture.left.join("notes.md").exists()
                    && self.fixture.left.join("README.txt").exists()
            }
            Scenario::Permissions => {
                self.inner
                    .ui
                    .modals
                    .permissions
                    .as_ref()
                    .is_some_and(|state| {
                        state.plan.as_ref().is_some_and(|plan| {
                            plan.ready() && plan.changed_count(state.change) == 1
                        })
                    })
            }
            Scenario::PdfPreview => {
                self.pdf_click_phase == 10
                    && ctx.data(|data| {
                        data.get_temp::<crate::preview_find::PreviewFind>(
                            egui::Id::new(("pdf-page-text", "right")).with("find"),
                        )
                        .is_some_and(|find| find.match_count() == 1)
                    })
                    && self.inner.pdf_cache.ready("right")
                    && self.inner.pdf_cache.shown_page("right") == Some(1)
            }
            Scenario::ImagePreview => {
                self.image_click_phase == 21
                    && crate::image_preview::centered(&ctx, "right") == Some(false)
                    && crate::image_preview::enlarges_small(&ctx, "right")
                    && crate::image_preview::orientation(&ctx, "right") == Some([2, 1, 0, 3])
                    && crate::image_preview::checkerboard_active(&ctx, "right")
                    && matches!(
                        self.inner.ws.right.preview,
                        Some(crate::panel::PreviewContent::Image(_))
                    )
                    && self
                        .inner
                        .image_cache
                        .get(&self.fixture.left.join("photo.png"))
                        .is_some()
                    && crate::image_preview::zoom(&ctx, "right") == Some(2.0)
            }
            Scenario::ArchiveBrowser => self.inner.ui.tools.archive.as_ref().is_some_and(|state| {
                state.listing.as_ref().is_some_and(|listing| {
                    let ordered = |a: &str, b: &str, folders: bool| {
                        let members = if folders {
                            &listing.tree.as_ref().unwrap().directories
                        } else {
                            &listing.members
                        };
                        let position = |path: &str| {
                            members
                                .iter()
                                .position(|entry| entry.path == Path::new(path))
                                .and_then(|index| {
                                    state.member_filter.row_position(
                                        index + if folders { listing.members.len() } else { 0 },
                                    )
                                })
                        };
                        matches!((position(a), position(b)), (Some(a), Some(b)) if a < b)
                    };
                    listing.kind == crate::archive::ArchiveKind::Tar
                        && listing.members.len() == 7
                        && state.member_filter.order.natural
                        && ordered("src/file2.rs", "src/file10.rs", false)
                        && ordered("src/folder2", "src/folder10", true)
                        && state.member_filter.current_folder() == Some(Path::new("src"))
                        && state.selected.contains(&0)
                        && !listing.truncated
                        && state.member_filter.order.field == crate::archive_order::Field::Size
                        && state.member_filter.order.descending
                        && state
                            .member_filter
                            .first_visible_member()
                            .and_then(|index| listing.members.get(index))
                            .is_some_and(|member| member.path == Path::new("src/main.rs"))
                })
            }),
            Scenario::FolderTabs => {
                self.tab_group_phase == 81
                    && self
                        .inner
                        .ws
                        .tab_snapshot(crate::workspace::ActivePanel::Left)
                        .entries[0]
                        .close_protected
                    && self
                        .inner
                        .ws
                        .tab_snapshot(crate::workspace::ActivePanel::Left)
                        .entries[0]
                        .title
                        .as_deref()
                        == Some("Work")
                    && self
                        .inner
                        .ws
                        .tab_snapshot(crate::workspace::ActivePanel::Left)
                        .entries[0]
                        .path
                        == self.fixture.left
                    && self.inner.ws.tab_count(crate::workspace::ActivePanel::Left) == 2
                    && self
                        .inner
                        .ws
                        .closed_tab_count(crate::workspace::ActivePanel::Left)
                        == 4
                    && self
                        .inner
                        .ws
                        .active_tab_index(crate::workspace::ActivePanel::Left)
                        == 0
                    && self.inner.ws.left.current_path == self.fixture.left
                    && self.inner.ws.right.current_path == self.fixture.left
                    && self.inner.ws.active == crate::workspace::ActivePanel::Left
                    && self
                        .inner
                        .ws
                        .tab_count(crate::workspace::ActivePanel::Right)
                        == 1
                    && self
                        .inner
                        .ws
                        .active_tab_index(crate::workspace::ActivePanel::Right)
                        == 0
                    && self
                        .inner
                        .ws
                        .tab_snapshot(crate::workspace::ActivePanel::Right)
                        .entries[0]
                        .path
                        == self.fixture.left
                    && self
                        .inner
                        .ws
                        .closed_tab_paths(crate::workspace::ActivePanel::Right)
                        == vec![
                            self.fixture.left.join("Design"),
                            self.fixture.left.join("Design"),
                        ]
                    && self
                        .inner
                        .ws
                        .tab_snapshot(crate::workspace::ActivePanel::Left)
                        .entries[1]
                        .path
                        == self.fixture.left.join("Design")
            }
            Scenario::RegexPreview => {
                self.regex_click_phase == 6
                    && ctx.data(|data| {
                        data.get_temp::<crate::preview_find::PreviewFind>(
                            egui::Id::new(("text_preview", "right")).with("find"),
                        )
                        .is_some_and(|find| find.regex_ready() && find.match_count() == 2)
                    })
            }
            Scenario::UnicodePreview => {
                matches!(&self.inner.ws.right.preview,
                    Some(crate::panel::PreviewContent::Text { content, .. })
                    if content.as_ref() == "UTF-16 preview\nmessage: café / ёж\nmessage: deuxième\n")
                    && ctx.data(|data| {
                        data.get_temp::<crate::preview_find::PreviewFind>(
                            egui::Id::new(("text_preview", "right")).with("find"),
                        )
                        .is_some_and(|find| find.match_count() == 2)
                    })
            }
            Scenario::CodePreview => {
                self.text_size_click_phase == 27
                    && ctx.data(|data| {
                        data.get_temp::<[bool; 2]>(
                            egui::Id::new(("text_preview", "right"))
                                .with("find")
                                .with("navigation-enabled"),
                        )
                    }) == Some([false, false])
                    && ctx
                        .data(|data| {
                            data.get_temp::<[usize; 2]>(
                                egui::Id::new(("text_preview", "right")).with("whitespace-counts"),
                            )
                        })
                        .is_some_and(|counts| counts[0] > 0 && counts[1] > 0)
                    && ctx.data(|data| {
                        data.get_temp::<u16>(
                            egui::Id::new(("text_preview", "right")).with("text-percent"),
                        )
                    }) == Some(125)
                    && matches!(
                        self.inner.ws.right.preview,
                        Some(crate::panel::PreviewContent::Text { .. })
                    )
                    && ctx.data(|data| {
                        data.get_temp::<crate::preview_find::PreviewFind>(
                            egui::Id::new(("text_preview", "right")).with("find"),
                        )
                        .is_some_and(|find| find.match_count() == 0 && find.smart_case_enabled())
                    })
            }
            _ => true,
        };
        if self.scenario == Scenario::FolderTabs {
            self.last_probe_summary.push_str(&format!(
                "; tab_phase={}, focus={:?}, left={:?}, right={:?}",
                self.tab_group_phase,
                self.inner.ws.active,
                self.inner
                    .ws
                    .tab_snapshot(crate::workspace::ActivePanel::Left),
                self.inner
                    .ws
                    .tab_snapshot(crate::workspace::ActivePanel::Right)
            ));
        }
        if self.scenario == Scenario::CodePreview {
            self.last_probe_summary.push_str(&format!(
                "; right_text={}, find_matches={:?}",
                matches!(
                    self.inner.ws.right.preview,
                    Some(crate::panel::PreviewContent::Text { .. })
                ),
                ctx.data(|data| data
                    .get_temp::<crate::preview_find::PreviewFind>(
                        egui::Id::new(("text_preview", "right")).with("find")
                    )
                    .map(|find| find.match_count()))
            ));
        }
        if listings_ready && thumbnail_ready && scenario_state_ready && probes.ready(self.scenario)
        {
            let geometry = probes.geometry_key();
            if geometry == self.last_geometry {
                self.stable_frames = self.stable_frames.saturating_add(1);
            } else {
                self.last_geometry = geometry;
                self.stable_frames = 0;
            }
            if self.stable_frames >= 2 {
                let should_request = self.capture_probes.is_empty()
                    || (self.capture_attempts < MAX_CAPTURE_ATTEMPTS
                        && self
                            .last_request_at
                            .is_some_and(|requested| requested.elapsed() >= CAPTURE_RETRY_AFTER));
                if should_request {
                    self.request_screenshot(&ctx, probes);
                }
            }
        }
        // Keep timeout/retry observation alive while waiting for the native
        // screenshot event too; an idle backend must not leave QA hung forever.
        ctx.request_repaint_after(Duration::from_millis(16));
        let _keep_fixture_alive = &self.fixture;
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum CapabilityStatus {
    Attempted,
    Captured,
    Unsupported,
    Failed,
}

#[derive(Serialize)]
struct Capabilities<'a> {
    schema: u32,
    platform: &'a str,
    status: CapabilityStatus,
    framebuffer: &'a str,
    failure_kind: &'a str,
    detail: &'a str,
    renderer: Option<&'a RendererDiagnostics>,
    native_menu_model: &'a str,
    native_menu_pixels: &'a str,
    whole_window_capture: &'a str,
    voice_over: &'a str,
}

fn write_capabilities(
    directory: &Path,
    status: CapabilityStatus,
    failure_kind: &str,
    detail: &str,
    renderer: Option<&RendererDiagnostics>,
) -> Result<(), String> {
    let framebuffer = match status {
        CapabilityStatus::Attempted => "attempted",
        CapabilityStatus::Captured => "captured",
        CapabilityStatus::Unsupported => "skipped_capability_unavailable",
        CapabilityStatus::Failed => "failed",
    };
    write_json(
        &directory.join("capabilities.json"),
        &Capabilities {
            schema: QA_SCHEMA,
            platform: std::env::consts::OS,
            status,
            framebuffer,
            failure_kind,
            detail,
            renderer,
            native_menu_model: "automated_pure_contract",
            native_menu_pixels: "excluded_use_native_release_qa_evidence",
            whole_window_capture: "excluded_use_native_release_qa_evidence",
            voice_over: "excluded_use_native_release_qa_attestation",
        },
    )
}

#[derive(Clone, Copy, Debug, Serialize)]
struct RectArtifact {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl From<Rect> for RectArtifact {
    fn from(rect: Rect) -> Self {
        Self {
            min_x: rect.min.x,
            min_y: rect.min.y,
            max_x: rect.max.x,
            max_y: rect.max.y,
        }
    }
}

#[derive(Serialize)]
struct PaintedGlyphArtifact {
    name: &'static str,
    rect: RectArtifact,
}

#[derive(Serialize)]
struct Check {
    name: &'static str,
    passed: bool,
    detail: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct NativeEffectCalls {
    context_menu: usize,
    deferred_menu: usize,
    clipboard: usize,
    opener: usize,
    trash: usize,
    free_space: usize,
}

impl NativeEffectCalls {
    fn expected_for(self, scenario: Scenario) -> bool {
        if scenario == Scenario::JsonPreview {
            self.clipboard == 1 && self.total() == 1
        } else {
            self.total() == 0
        }
    }
    const fn total(self) -> usize {
        self.context_menu
            + self.deferred_menu
            + self.clipboard
            + self.opener
            + self.trash
            + self.free_space
    }
}

#[derive(Serialize)]
struct Manifest {
    schema: u32,
    scenario: String,
    status: String,
    requested_viewport: [f32; 2],
    ui_scale: f32,
    theme: &'static str,
    high_contrast: bool,
    reduced_motion: bool,
    pixels_per_point: Option<f32>,
    image_size: Option<[usize; 2]>,
    viewport: Option<RectArtifact>,
    left_pane: Option<RectArtifact>,
    right_pane: Option<RectArtifact>,
    confirmation: Option<RectArtifact>,
    left_row: Option<RectArtifact>,
    modal_owner: Option<String>,
    background_enabled: Option<bool>,
    modal_above_background: Option<bool>,
    painted_glyphs: Vec<PaintedGlyphArtifact>,
    native_effect_calls: NativeEffectCalls,
    checks: Vec<Check>,
    error: Option<String>,
}

fn base_manifest(scenario: Scenario, status: &str) -> Manifest {
    let preferences = scenario.preferences();
    Manifest {
        schema: QA_SCHEMA,
        scenario: scenario.name().to_string(),
        status: status.to_string(),
        requested_viewport: scenario.viewport(),
        ui_scale: scenario.ui_scale(),
        theme: match scenario.theme() {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        },
        high_contrast: preferences.high_contrast,
        reduced_motion: preferences.reduced_motion,
        pixels_per_point: None,
        image_size: None,
        viewport: None,
        left_pane: None,
        right_pane: None,
        confirmation: None,
        left_row: None,
        modal_owner: None,
        background_enabled: None,
        modal_above_background: None,
        painted_glyphs: Vec::new(),
        native_effect_calls: NativeEffectCalls::default(),
        checks: Vec::new(),
        error: None,
    }
}

fn write_unfinished_manifest(
    directory: &Path,
    scenario: Scenario,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let mut manifest = base_manifest(scenario, status);
    manifest.error = error.map(str::to_string);
    write_json(&directory.join("manifest.json"), &manifest)
}

fn write_capture(
    directory: &Path,
    scenario: Scenario,
    image: &ColorImage,
    probes: &ProbeSnapshot,
    calls: NativeEffectCalls,
) -> Result<(), String> {
    let [width, height] = image.size;
    let mut rgba = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for pixel in &image.pixels {
        rgba.extend_from_slice(&pixel.to_array());
    }
    image::save_buffer_with_format(
        directory.join("frame.png"),
        &rgba,
        width as u32,
        height as u32,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| format!("could not write framebuffer PNG: {error}"))?;

    let mut checks = Vec::new();
    let expected_width = (probes.viewport.width() * probes.pixels_per_point).round() as usize;
    let expected_height = (probes.viewport.height() * probes.pixels_per_point).round() as usize;
    checks.push(check(
        "framebuffer_dimensions",
        width.abs_diff(expected_width) <= 2 && height.abs_diff(expected_height) <= 2,
        format!("{width}x{height}, expected about {expected_width}x{expected_height}"),
    ));
    let requested = scenario.viewport();
    let logical_width = probes.viewport.width();
    let logical_height = probes.viewport.height();
    checks.push(check(
        "logical_viewport_dimensions",
        (logical_width - requested[0]).abs() <= 2.0 && (logical_height - requested[1]).abs() <= 2.0,
        format!(
            "{logical_width:.1}x{logical_height:.1}, requested {}x{}",
            requested[0], requested[1]
        ),
    ));

    let mut exact = HashMap::<u32, usize>::new();
    let mut buckets = HashSet::<u16>::new();
    let mut nonzero_alpha = 0_usize;
    for pixel in &image.pixels {
        let [red, green, blue, alpha] = pixel.to_array();
        nonzero_alpha += usize::from(alpha > 0);
        *exact
            .entry(u32::from_be_bytes([red, green, blue, alpha]))
            .or_default() += 1;
        buckets.insert(
            (u16::from(red >> 5) << 6) | (u16::from(green >> 5) << 3) | u16::from(blue >> 5),
        );
    }
    let pixels = image.pixels.len().max(1);
    let dominant = exact.values().copied().max().unwrap_or_default();
    checks.push(check(
        "nonzero_alpha",
        nonzero_alpha == pixels,
        format!("{nonzero_alpha}/{pixels} pixels have non-zero alpha"),
    ));
    checks.push(check(
        "nonblank_frame",
        pixels.saturating_sub(dominant) * 100 >= pixels,
        format!(
            "{:.2}% pixels differ from the dominant color",
            pixels.saturating_sub(dominant) as f64 * 100.0 / pixels as f64
        ),
    ));
    checks.push(check(
        "color_diversity",
        buckets.len() >= 12,
        format!("{} quantized RGB buckets", buckets.len()),
    ));

    let mut required_rects = vec![
        ("viewport_rect", Some(probes.viewport)),
        ("left_pane_rect", probes.left_pane.map(|record| record.rect)),
        (
            "right_pane_rect",
            probes.right_pane.map(|record| record.rect),
        ),
    ];
    if scenario.modal_owner().is_some() {
        required_rects.push((
            "confirmation_rect",
            probes.confirmation.map(|record| record.rect),
        ));
    } else {
        required_rects.push(("left_row_rect", probes.left_row.map(|record| record.rect)));
    }
    for (name, rect) in required_rects {
        checks.push(check(
            name,
            rect.is_some_and(|rect| rect_is_valid(rect, probes.viewport)),
            format!("{rect:?} inside {:?}", probes.viewport),
        ));
    }
    let missing_painted_glyphs = required_painted_glyphs(scenario)
        .filter(|glyph| !probes.painted_glyphs.contains_key(glyph))
        .map(crate::app::glyphs::PaintedGlyph::key)
        .collect::<Vec<_>>();
    let painted_rects_valid = probes
        .painted_glyphs
        .values()
        .all(|rect| rect_is_valid(*rect, probes.viewport));
    checks.push(check(
        "mandatory_painted_glyph_contract",
        missing_painted_glyphs.is_empty() && painted_rects_valid,
        format!(
            "missing={missing_painted_glyphs:?}, rects={:?}",
            probes
                .painted_glyphs
                .iter()
                .map(|(glyph, rect)| (glyph.key(), rect))
                .collect::<Vec<_>>()
        ),
    ));
    let mut glyph_pixel_details = Vec::new();
    let painted_pixels_valid = required_painted_glyphs(scenario).all(|glyph| {
        let Some(rect) = probes.painted_glyphs.get(&glyph).copied() else {
            glyph_pixel_details.push(format!("{}=missing", glyph.key()));
            return false;
        };
        let Some((ink, total)) = glyph_region_ink(image, probes.viewport, rect) else {
            glyph_pixel_details.push(format!("{}=invalid_region", glyph.key()));
            return false;
        };
        glyph_pixel_details.push(format!("{}={ink}/{total}", glyph.key()));
        ink >= 8 && ink.saturating_mul(100) >= total
    });
    checks.push(check(
        "mandatory_painted_glyph_pixels",
        painted_pixels_valid,
        glyph_pixel_details.join(", "),
    ));
    let panes_do_not_overlap = probes
        .left_pane
        .zip(probes.right_pane)
        .is_some_and(|(left, right)| left.rect.right() <= right.rect.left() + 1.0);
    checks.push(check(
        "pane_non_overlap",
        panes_do_not_overlap,
        format!("left={:?}, right={:?}", probes.left_pane, probes.right_pane),
    ));
    if let Some(owner) = scenario.modal_owner() {
        checks.push(check(
            "modal_owner",
            probes.modal_owner.as_deref() == Some(owner),
            format!("owner={:?}", probes.modal_owner),
        ));
        checks.push(check(
            "modal_background_disabled",
            probes.background_enabled == Some(false),
            format!("background_enabled={:?}", probes.background_enabled),
        ));
        checks.push(check(
            "modal_above_background",
            probes.modal_above_background == Some(true),
            format!("modal_above_background={:?}", probes.modal_above_background),
        ));
    } else {
        if scenario.opens_pane_tool() {
            checks.push(check(
                "pane_tool_leaves_input_live",
                probes.background_enabled == Some(true) && probes.modal_owner.is_none(),
                format!(
                    "background_enabled={:?}, owner={:?}",
                    probes.background_enabled, probes.modal_owner
                ),
            ));
        }
        checks.push(check(
            "row_runtime_hover",
            probes
                .left_row
                .is_some_and(|row| row.enabled && row.hovered),
            format!("left_row={:?}", probes.left_row),
        ));
    }
    checks.push(check(
        "native_effect_isolation",
        calls.expected_for(scenario),
        format!(
            "{calls:?}; expected {} recording clipboard write(s)",
            usize::from(scenario == Scenario::JsonPreview)
        ),
    ));

    let failed = checks.iter().filter(|check| !check.passed).count();
    let mut manifest = base_manifest(scenario, if failed == 0 { "captured" } else { "failed" });
    manifest.pixels_per_point = Some(probes.pixels_per_point);
    manifest.image_size = Some(image.size);
    manifest.viewport = Some(probes.viewport.into());
    manifest.left_pane = probes.left_pane.map(|record| record.rect.into());
    manifest.right_pane = probes.right_pane.map(|record| record.rect.into());
    manifest.confirmation = probes.confirmation.map(|record| record.rect.into());
    manifest.left_row = probes.left_row.map(|record| record.rect.into());
    manifest.modal_owner = probes.modal_owner.clone();
    manifest.background_enabled = probes.background_enabled;
    manifest.modal_above_background = probes.modal_above_background;
    manifest.painted_glyphs = required_painted_glyphs(scenario)
        .filter_map(|glyph| {
            probes
                .painted_glyphs
                .get(&glyph)
                .copied()
                .map(|rect| PaintedGlyphArtifact {
                    name: glyph.key(),
                    rect: rect.into(),
                })
        })
        .collect();
    manifest.native_effect_calls = calls;
    manifest.checks = checks;
    if failed > 0 {
        manifest.error = Some(format!("{failed} visual QA checks failed"));
    }
    write_json(&directory.join("manifest.json"), &manifest)?;
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} visual QA checks failed"))
    }
}

fn glyph_region_ink(image: &ColorImage, viewport: Rect, rect: Rect) -> Option<(usize, usize)> {
    if !rect_is_valid(rect, viewport) || viewport.width() <= 0.0 || viewport.height() <= 0.0 {
        return None;
    }
    let [width, height] = image.size;
    let scale_x = width as f32 / viewport.width();
    let scale_y = height as f32 / viewport.height();
    let x0 = ((rect.min.x - viewport.min.x) * scale_x)
        .floor()
        .clamp(0.0, width as f32) as usize;
    let y0 = ((rect.min.y - viewport.min.y) * scale_y)
        .floor()
        .clamp(0.0, height as f32) as usize;
    let x1 = ((rect.max.x - viewport.min.x) * scale_x)
        .ceil()
        .clamp(0.0, width as f32) as usize;
    let y1 = ((rect.max.y - viewport.min.y) * scale_y)
        .ceil()
        .clamp(0.0, height as f32) as usize;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut colors = HashMap::<u32, usize>::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let [red, green, blue, alpha] = image.pixels[y * width + x].to_array();
            *colors
                .entry(u32::from_be_bytes([red, green, blue, alpha]))
                .or_default() += 1;
        }
    }
    let total = (x1 - x0).saturating_mul(y1 - y0);
    let dominant = colors.values().copied().max().unwrap_or_default();
    Some((total.saturating_sub(dominant), total))
}

fn rect_is_valid(rect: Rect, viewport: Rect) -> bool {
    rect.is_finite()
        && rect.is_positive()
        && rect.min.x >= viewport.min.x - 0.51
        && rect.min.y >= viewport.min.y - 0.51
        && rect.max.x <= viewport.max.x + 0.51
        && rect.max.y <= viewport.max.y + 0.51
}

const fn check(name: &'static str, passed: bool, detail: String) -> Check {
    Check {
        name,
        passed,
        detail,
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("could not serialize {}: {error}", path.display()))?;
    std::fs::write(path, bytes)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_json_copy_scenario_accepts_exactly_one_recording_clipboard_call() {
        for scenario in Scenario::ALL {
            let empty = NativeEffectCalls::default();
            assert_eq!(
                empty.expected_for(scenario),
                scenario != Scenario::JsonPreview
            );
            assert_eq!(
                NativeEffectCalls {
                    clipboard: 1,
                    ..empty
                }
                .expected_for(scenario),
                scenario == Scenario::JsonPreview
            );
            assert!(
                !NativeEffectCalls {
                    clipboard: 2,
                    ..empty
                }
                .expected_for(scenario)
            );
            assert!(!NativeEffectCalls { opener: 1, ..empty }.expected_for(scenario));
            assert!(
                !NativeEffectCalls {
                    clipboard: 1,
                    opener: 1,
                    ..empty
                }
                .expected_for(scenario)
            );
        }
    }

    #[test]
    fn required_scenario_matrix_is_stable() {
        assert_eq!(
            Scenario::ALL.map(Scenario::name),
            [
                "desktop_base",
                "icons_grid",
                "image_preview",
                "pdf_preview",
                "checksums",
                "hex_preview",
                "hex_search",
                "permissions",
                "rename_presets",
                "rename_target",
                "rename_options",
                "name_list",
                "folder_tabs",
                "code_preview",
                "unicode_preview",
                "regex_preview",
                "json_preview",
                "archive_browser",
                "minimum_window",
                "zoom_200_accessible",
                "confirmation_owner",
                "pane_tool",
            ]
        );
        assert_eq!(Scenario::MinimumWindow.viewport(), [900.0, 500.0]);
        assert_eq!(Scenario::Zoom200Accessible.ui_scale(), 2.0);
        assert_eq!(
            Scenario::Zoom200Accessible.native_window_size(),
            [1800.0, 1000.0]
        );
        assert!(Scenario::Zoom200Accessible.preferences().high_contrast);
        assert!(Scenario::ConfirmationOwner.needs_confirmation());
        assert!(Scenario::PaneTool.opens_pane_tool());
    }

    #[test]
    fn rect_contract_rejects_clipping_and_non_finite_values() {
        let viewport = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(900.0, 500.0));
        assert!(rect_is_valid(
            Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(899.0, 499.0)),
            viewport
        ));
        assert!(!rect_is_valid(
            Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(903.0, 499.0)),
            viewport
        ));
        assert!(!rect_is_valid(
            Rect::from_min_max(egui::pos2(f32::NAN, 1.0), egui::pos2(2.0, 2.0)),
            viewport
        ));
    }

    #[test]
    fn painted_glyph_probe_rejects_blank_regions_and_detects_ink() {
        let viewport = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(20.0, 20.0));
        let region = Rect::from_min_max(egui::pos2(5.0, 5.0), egui::pos2(15.0, 15.0));
        let mut image = ColorImage::filled([20, 20], egui::Color32::BLACK);
        assert_eq!(glyph_region_ink(&image, viewport, region), Some((0, 100)));

        for x in 7..13 {
            image[(x, 10)] = egui::Color32::WHITE;
        }
        assert_eq!(glyph_region_ink(&image, viewport, region), Some((6, 100)));
    }
}
