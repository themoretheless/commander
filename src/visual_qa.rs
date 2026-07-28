use crate::app::{App, VisualQaSeed};
use crate::ports::{
    ClipboardOutcome, ClipboardPort, ContextMenuAction, ContextMenuPort, ContextMenuResult,
    FreeSpacePort, NativeFailure, OpenOutcome, OpenRequest, OpenerPort, SpacePrecision,
    SpaceProbeOutcome, TrashItemOutcome, TrashPort, TrashTarget, VolumeRelation,
};
use crate::theme::ThemeMode;
use egui::{ColorImage, Rect};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const QA_SCHEMA: u32 = 1;
const MAX_FRAMES: u32 = 1_000;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    DesktopBase,
    MinimumWindow,
    Zoom200Accessible,
    ConfirmationOwner,
}

impl Scenario {
    const ALL: [Self; 4] = [
        Self::DesktopBase,
        Self::MinimumWindow,
        Self::Zoom200Accessible,
        Self::ConfirmationOwner,
    ];

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == value)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::DesktopBase => "desktop_base",
            Self::MinimumWindow => "minimum_window",
            Self::Zoom200Accessible => "zoom_200_accessible",
            Self::ConfirmationOwner => "confirmation_owner",
        }
    }

    const fn viewport(self) -> [f32; 2] {
        match self {
            Self::MinimumWindow | Self::Zoom200Accessible => [900.0, 500.0],
            Self::DesktopBase | Self::ConfirmationOwner => [1280.0, 760.0],
        }
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
            Self::DesktopBase | Self::ConfirmationOwner => ThemeMode::Dark,
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

    const fn needs_confirmation(self) -> bool {
        matches!(self, Self::ConfirmationOwner)
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

pub(crate) fn maybe_run() -> Option<eframe::Result<()>> {
    match Request::parse() {
        Ok(Some(request)) => Some(run(request)),
        Ok(None) => None,
        Err(error) => Some(Err(app_error(error))),
    }
}

fn run(request: Request) -> eframe::Result<()> {
    let scenario_dir = request.output_root.join(request.scenario.name());
    std::fs::create_dir_all(&scenario_dir)
        .map_err(|error| app_error(format!("could not create QA artifact directory: {error}")))?;
    let stale_frame = scenario_dir.join("frame.png");
    if stale_frame.is_file() {
        std::fs::remove_file(&stale_frame).map_err(|error| {
            app_error(format!(
                "could not remove stale QA frame {}: {error}",
                stale_frame.display()
            ))
        })?;
    }
    write_capabilities(
        &scenario_dir,
        CapabilityStatus::Attempted,
        "starting native eframe/WGPU capture",
    )
    .map_err(app_error)?;
    write_unfinished_manifest(&scenario_dir, request.scenario, "attempted", None)
        .map_err(app_error)?;

    let fixture = Fixture::create(request.scenario).map_err(app_error)?;
    crate::fs_util::install_storage_root_override(fixture.root.join("state"))
        .map_err(|_| app_error("visual QA storage override was already installed"))?;

    let viewport = request.scenario.viewport();
    let options = eframe::NativeOptions {
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
        let capability_missing = !matches!(error, eframe::Error::AppCreation(_));
        let status = if capability_missing {
            CapabilityStatus::Skipped
        } else {
            CapabilityStatus::Failed
        };
        write_capabilities(&scenario_dir, status, &detail).map_err(app_error)?;
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
    fn show_context_menu(&self, _path: &Path) -> ContextMenuResult {
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
    fn write_text(&self, _text: &str) -> ClipboardOutcome {
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
pub(crate) enum ProbeId {
    LeftPane,
    RightPane,
    Confirmation,
}

impl ProbeId {
    const fn key(self) -> &'static str {
        match self {
            Self::LeftPane => "visual_qa_left_pane",
            Self::RightPane => "visual_qa_right_pane",
            Self::Confirmation => "visual_qa_confirmation",
        }
    }
}

pub(crate) fn begin_probe_frame(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        for id in [ProbeId::LeftPane, ProbeId::RightPane, ProbeId::Confirmation] {
            data.remove::<Rect>(egui::Id::new(id.key()));
        }
        data.remove::<bool>(egui::Id::new("visual_qa_background_enabled"));
        data.remove::<String>(egui::Id::new("visual_qa_modal_owner"));
    });
}

pub(crate) fn record_rect(ctx: &egui::Context, id: ProbeId, rect: Rect) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(id.key()), rect));
}

pub(crate) fn record_input_policy(
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
    left_pane: Option<Rect>,
    right_pane: Option<Rect>,
    confirmation: Option<Rect>,
    background_enabled: Option<bool>,
    modal_owner: Option<String>,
}

impl ProbeSnapshot {
    fn read(ctx: &egui::Context) -> Self {
        let viewport = ctx.input(|input| input.viewport_rect());
        let pixels_per_point = ctx.pixels_per_point();
        ctx.data_mut(|data| Self {
            viewport,
            pixels_per_point,
            left_pane: data.get_temp(egui::Id::new(ProbeId::LeftPane.key())),
            right_pane: data.get_temp(egui::Id::new(ProbeId::RightPane.key())),
            confirmation: data.get_temp(egui::Id::new(ProbeId::Confirmation.key())),
            background_enabled: data.get_temp(egui::Id::new("visual_qa_background_enabled")),
            modal_owner: data.get_temp(egui::Id::new("visual_qa_modal_owner")),
        })
    }

    fn ready(&self, scenario: Scenario) -> bool {
        self.left_pane.is_some()
            && self.right_pane.is_some()
            && (!scenario.needs_confirmation()
                || (self.confirmation.is_some()
                    && self.background_enabled == Some(false)
                    && self.modal_owner.as_deref() == Some("Confirmation")))
    }

    fn geometry_key(&self) -> Vec<i32> {
        let mut values = Vec::new();
        for rect in [
            Some(self.viewport),
            self.left_pane,
            self.right_pane,
            self.confirmation,
        ]
        .into_iter()
        .flatten()
        {
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
    requested: bool,
    last_request_frame: u32,
    capture_probes: Option<ProbeSnapshot>,
    confirmation_requested: bool,
    last_probe_summary: String,
    pending_screenshot: Option<Arc<ColorImage>>,
    wgpu_device: eframe::wgpu::Device,
    allow_capture_skip: bool,
}

impl VisualQaApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        scenario: Scenario,
        fixture: Fixture,
        scenario_dir: PathBuf,
        result: Arc<Mutex<Option<Result<(), String>>>>,
        allow_capture_skip: bool,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let counters = Arc::new(EffectCounters::default());
        let wgpu_device = cc
            .wgpu_render_state
            .as_ref()
            .map(|state| state.device.clone())
            .ok_or_else(|| std::io::Error::other("WGPU render state is unavailable"))?;
        let seed = VisualQaSeed {
            left: fixture.left.clone(),
            right: fixture.right.clone(),
            ui_scale: scenario.ui_scale(),
            theme_mode: scenario.theme(),
            accessibility_preferences: scenario.preferences(),
            show_tree: scenario.show_tree(),
        };
        let inner = App::new_visual_qa(
            cc,
            seed,
            Rc::new(FakeContextMenu(Arc::clone(&counters))),
            Rc::new(FakeClipboard(Arc::clone(&counters))),
            Rc::new(FakeOpener(Arc::clone(&counters))),
            Arc::new(FakeTrash(Arc::clone(&counters))),
            Arc::new(FakeFreeSpace(Arc::clone(&counters))),
        );
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
            requested: false,
            last_request_frame: 0,
            capture_probes: None,
            confirmation_requested: false,
            last_probe_summary: "no rendered frame yet".to_string(),
            pending_screenshot: None,
            wgpu_device,
            allow_capture_skip,
        })
    }

    fn finish(&self, ctx: &egui::Context, result: Result<(), String>) {
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(result);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn fail(&self, ctx: &egui::Context, message: String) {
        let _ = write_capabilities(&self.scenario_dir, CapabilityStatus::Failed, &message);
        let _ =
            write_unfinished_manifest(&self.scenario_dir, self.scenario, "failed", Some(&message));
        self.finish(ctx, Err(message));
    }

    fn screenshot_event(events: &[egui::Event], scenario: Scenario) -> Option<Arc<ColorImage>> {
        events.iter().find_map(|event| {
            let egui::Event::Screenshot {
                user_data, image, ..
            } = event
            else {
                return None;
            };
            let token = user_data
                .data
                .as_ref()
                .and_then(|data| data.downcast_ref::<ScreenshotToken>())?;
            (token.scenario == scenario.name()).then(|| Arc::clone(image))
        })
    }

    fn request_screenshot(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            ScreenshotToken {
                scenario: self.scenario.name(),
            },
        )));
    }
}

impl eframe::App for VisualQaApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        if self.pending_screenshot.is_none() {
            self.pending_screenshot = Self::screenshot_event(&input.events, self.scenario);
        }
        if !self.scenario.needs_confirmation() {
            let viewport = self.scenario.viewport();
            input.events.push(egui::Event::PointerMoved(egui::pos2(
                viewport[0] * 0.24 / self.scenario.ui_scale(),
                viewport[1] * 0.42 / self.scenario.ui_scale(),
            )));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.requested
            && let Err(error) = self.wgpu_device.poll(eframe::wgpu::PollType::Poll)
        {
            self.fail(&ctx, format!("WGPU screenshot poll failed: {error}"));
            return;
        }
        let screenshot = self
            .pending_screenshot
            .take()
            .or_else(|| ctx.input(|input| Self::screenshot_event(&input.events, self.scenario)));
        if let Some(image) = screenshot {
            let Some(probes) = self.capture_probes.take() else {
                self.fail(
                    &ctx,
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
                        "eframe/WGPU framebuffer captured and validated",
                    );
                    self.finish(&ctx, Ok(()));
                }
                Err(error) => self.fail(&ctx, error),
            }
            return;
        }

        self.frames = self.frames.saturating_add(1);
        if self.frames > MAX_FRAMES || self.started.elapsed() > TIMEOUT {
            let message = format!(
                "screenshot event unavailable after {} frames and {:.1}s: {}",
                self.frames,
                self.started.elapsed().as_secs_f32(),
                self.last_probe_summary,
            );
            if self.allow_capture_skip && self.requested {
                let _ = write_capabilities(&self.scenario_dir, CapabilityStatus::Skipped, &message);
                let _ = write_skipped_capture_manifest(
                    &self.scenario_dir,
                    self.scenario,
                    &message,
                    self.capture_probes.as_ref(),
                    self.counters.snapshot(),
                );
                self.finish(&ctx, Ok(()));
            } else {
                self.fail(&ctx, message);
            }
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

        let probes = ProbeSnapshot::read(&ctx);
        let listings_ready =
            !self.inner.ws.left.entries().is_empty() && !self.inner.ws.right.entries().is_empty();
        self.last_probe_summary = format!(
            "requested={}, stable={}, listings={}/{}, probes={probes:?}",
            self.requested,
            self.stable_frames,
            self.inner.ws.left.entries().len(),
            self.inner.ws.right.entries().len(),
        );
        if listings_ready && probes.ready(self.scenario) {
            let geometry = probes.geometry_key();
            if geometry == self.last_geometry {
                self.stable_frames = self.stable_frames.saturating_add(1);
            } else {
                self.last_geometry = geometry;
                self.stable_frames = 0;
            }
            if self.stable_frames >= 2 {
                self.capture_probes = Some(probes);
                if !self.requested || self.frames.saturating_sub(self.last_request_frame) >= 30 {
                    self.requested = true;
                    self.last_request_frame = self.frames;
                    self.request_screenshot(&ctx);
                }
            }
        }
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
    Skipped,
    Failed,
}

#[derive(Serialize)]
struct Capabilities<'a> {
    schema: u32,
    platform: &'a str,
    status: CapabilityStatus,
    framebuffer: &'a str,
    detail: &'a str,
    native_menu_model: &'a str,
    native_menu_pixels: &'a str,
    whole_window_capture: &'a str,
    voice_over: &'a str,
}

fn write_capabilities(
    directory: &Path,
    status: CapabilityStatus,
    detail: &str,
) -> Result<(), String> {
    let framebuffer = match status {
        CapabilityStatus::Attempted => "attempted",
        CapabilityStatus::Captured => "captured",
        CapabilityStatus::Skipped => "skipped_capability_unavailable",
        CapabilityStatus::Failed => "failed",
    };
    write_json(
        &directory.join("capabilities.json"),
        &Capabilities {
            schema: QA_SCHEMA,
            platform: std::env::consts::OS,
            status,
            framebuffer,
            detail,
            native_menu_model: "automated_pure_contract",
            native_menu_pixels: "manual_appkit_tracking_boundary",
            whole_window_capture: "manual_screen_recording_boundary",
            voice_over: "manual_accessibility_permission_boundary",
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
    fn total(self) -> usize {
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
    modal_owner: Option<String>,
    background_enabled: Option<bool>,
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
        modal_owner: None,
        background_enabled: None,
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

fn write_skipped_capture_manifest(
    directory: &Path,
    scenario: Scenario,
    reason: &str,
    probes: Option<&ProbeSnapshot>,
    calls: NativeEffectCalls,
) -> Result<(), String> {
    let mut manifest = base_manifest(scenario, "skipped");
    manifest.error = Some(reason.to_string());
    manifest.native_effect_calls = calls;
    manifest.checks.push(check(
        "framebuffer_capture_capability",
        false,
        reason.to_string(),
    ));
    if let Some(probes) = probes {
        manifest.pixels_per_point = Some(probes.pixels_per_point);
        manifest.viewport = Some(probes.viewport.into());
        manifest.left_pane = probes.left_pane.map(Into::into);
        manifest.right_pane = probes.right_pane.map(Into::into);
        manifest.confirmation = probes.confirmation.map(Into::into);
        manifest.modal_owner = probes.modal_owner.clone();
        manifest.background_enabled = probes.background_enabled;
    }
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

    for (name, rect) in [
        ("viewport_rect", Some(probes.viewport)),
        ("left_pane_rect", probes.left_pane),
        ("right_pane_rect", probes.right_pane),
        (
            "confirmation_rect",
            scenario
                .needs_confirmation()
                .then_some(probes.confirmation)
                .flatten(),
        ),
    ] {
        if let Some(rect) = rect {
            checks.push(check(
                name,
                rect_is_valid(rect, probes.viewport),
                format!("{rect:?} inside {:?}", probes.viewport),
            ));
        }
    }
    let panes_do_not_overlap = probes
        .left_pane
        .zip(probes.right_pane)
        .is_some_and(|(left, right)| left.right() <= right.left() + 1.0);
    checks.push(check(
        "pane_non_overlap",
        panes_do_not_overlap,
        format!("left={:?}, right={:?}", probes.left_pane, probes.right_pane),
    ));
    if scenario.needs_confirmation() {
        checks.push(check(
            "modal_owner",
            probes.modal_owner.as_deref() == Some("Confirmation"),
            format!("owner={:?}", probes.modal_owner),
        ));
        checks.push(check(
            "modal_background_disabled",
            probes.background_enabled == Some(false),
            format!("background_enabled={:?}", probes.background_enabled),
        ));
    }
    checks.push(check(
        "native_effect_isolation",
        calls.total() == 0,
        format!("{calls:?}"),
    ));

    let failed = checks.iter().filter(|check| !check.passed).count();
    let mut manifest = base_manifest(scenario, if failed == 0 { "captured" } else { "failed" });
    manifest.pixels_per_point = Some(probes.pixels_per_point);
    manifest.image_size = Some(image.size);
    manifest.viewport = Some(probes.viewport.into());
    manifest.left_pane = probes.left_pane.map(Into::into);
    manifest.right_pane = probes.right_pane.map(Into::into);
    manifest.confirmation = probes.confirmation.map(Into::into);
    manifest.modal_owner = probes.modal_owner.clone();
    manifest.background_enabled = probes.background_enabled;
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

fn rect_is_valid(rect: Rect, viewport: Rect) -> bool {
    rect.is_finite()
        && rect.is_positive()
        && rect.min.x >= viewport.min.x - 2.5
        && rect.min.y >= viewport.min.y - 2.5
        && rect.max.x <= viewport.max.x + 2.5
        && rect.max.y <= viewport.max.y + 2.5
}

fn check(name: &'static str, passed: bool, detail: String) -> Check {
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
    fn required_scenario_matrix_is_stable() {
        assert_eq!(
            Scenario::ALL.map(Scenario::name),
            [
                "desktop_base",
                "minimum_window",
                "zoom_200_accessible",
                "confirmation_owner",
            ]
        );
        assert_eq!(Scenario::MinimumWindow.viewport(), [900.0, 500.0]);
        assert_eq!(Scenario::Zoom200Accessible.ui_scale(), 2.0);
        assert!(Scenario::Zoom200Accessible.preferences().high_contrast);
        assert!(Scenario::ConfirmationOwner.needs_confirmation());
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
}
