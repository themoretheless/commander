use serde::{Deserialize, Serialize};
#[cfg(feature = "visual-qa")]
use std::path::PathBuf;

#[cfg(feature = "visual-qa")]
pub(super) const EVIDENCE_SCHEMA: u32 = 4;
#[cfg(any(test, feature = "visual-qa"))]
pub(super) const ATTESTATION_SCHEMA: u32 = 2;
#[cfg(any(test, feature = "visual-qa"))]
pub(super) const REQUIRED_MANUAL_CASES: [&str; 5] = [
    "voiceover_primary_journey",
    "voiceover_context_menu",
    "appkit_popup_pixels",
    "popup_escape_focus_return",
    "mixed_scale_multi_monitor_placement",
];

#[cfg(feature = "visual-qa")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeQaMode {
    Diagnostic,
    Strict,
}

#[cfg(feature = "visual-qa")]
#[derive(Clone, Debug)]
pub struct NativeQaRequest {
    pub mode: NativeQaMode,
    pub output_root: PathBuf,
    pub attestation: Option<PathBuf>,
}

#[cfg(feature = "visual-qa")]
impl NativeQaRequest {
    #[cfg(feature = "visual-qa")]
    pub fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut arguments = arguments.peekable();
        let mode = match arguments.peek().map(String::as_str) {
            Some("diagnostic") => {
                arguments.next();
                NativeQaMode::Diagnostic
            }
            Some("strict") => {
                arguments.next();
                NativeQaMode::Strict
            }
            Some(value) if !value.starts_with('-') => {
                return Err(format!(
                    "unknown native release QA mode {value:?}; expected diagnostic or strict"
                ));
            }
            _ => NativeQaMode::Diagnostic,
        };
        let mut output_root = PathBuf::from("target/native-release-qa");
        let mut attestation = None;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--output" => {
                    output_root = arguments
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| "--output requires a path".to_string())?;
                }
                "--attestation" => {
                    attestation = arguments.next().map(PathBuf::from);
                    if attestation.is_none() {
                        return Err("--attestation requires a path".to_string());
                    }
                }
                other => return Err(format!("unknown native release QA argument: {other}")),
            }
        }
        Ok(Self {
            mode,
            output_root,
            attestation,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenRect {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl ScreenRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            min_x: x,
            min_y: y,
            max_x: x + width.max(0.0),
            max_y: y + height.max(0.0),
        }
    }

    pub(super) fn width(self) -> f64 {
        self.max_x - self.min_x
    }

    pub(super) fn height(self) -> f64 {
        self.max_y - self.min_y
    }

    pub(super) fn is_valid(self) -> bool {
        [self.min_x, self.min_y, self.max_x, self.max_y]
            .into_iter()
            .all(f64::is_finite)
            && self.width() > 0.0
            && self.height() > 0.0
    }

    pub(super) fn contains(self, point: ScreenPoint) -> bool {
        point.x >= self.min_x
            && point.x <= self.max_x
            && point.y >= self.min_y
            && point.y <= self.max_y
    }

    pub(super) fn contains_half_open(self, point: ScreenPoint) -> bool {
        point.x >= self.min_x
            && point.x < self.max_x
            && point.y >= self.min_y
            && point.y < self.max_y
    }

    pub(super) fn contains_rect(self, rect: Self) -> bool {
        rect.is_valid()
            && rect.min_x >= self.min_x
            && rect.min_y >= self.min_y
            && rect.max_x <= self.max_x
            && rect.max_y <= self.max_y
    }

    pub(super) fn distance_squared(self, point: ScreenPoint) -> f64 {
        let x = point.x.clamp(self.min_x, self.max_x);
        let y = point.y.clamp(self.min_y, self.max_y);
        (point.x - x).powi(2) + (point.y - y).powi(2)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PopupSize {
    pub width: f64,
    pub height: f64,
}

impl PopupSize {
    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PopupPlacement {
    pub display_id: String,
    /// AppKit global-screen top-left anchor. The menu grows toward lower y.
    pub top_left: ScreenPoint,
    pub content_rect: ScreenRect,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayTopology {
    pub id: String,
    pub main: bool,
    pub frame: ScreenRect,
    pub visible_frame: ScreenRect,
    pub backing_scale: f64,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    Denied,
    NotRunning,
    Unavailable,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCapabilities {
    pub window_server: CapabilityState,
    pub accessibility: CapabilityState,
    pub screen_recording: CapabilityState,
    pub voice_over: CapabilityState,
}

#[cfg(any(test, feature = "visual-qa"))]
impl NativeCapabilities {
    pub(super) fn release_ready(&self) -> bool {
        [
            self.window_server,
            self.accessibility,
            self.screen_recording,
            self.voice_over,
        ]
        .into_iter()
        .all(|state| state == CapabilityState::Available)
    }
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    Blocked,
    NotRun,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckEvidence {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[cfg(any(test, feature = "visual-qa"))]
impl CheckEvidence {
    pub fn passed(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Passed,
            detail: detail.into(),
        }
    }

    pub fn failed(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Failed,
            detail: detail.into(),
        }
    }

    pub(super) fn blocked(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: CheckStatus::Blocked,
            detail: detail.into(),
        }
    }
}

#[cfg(feature = "visual-qa")]
pub struct NativeMenuRendererEvidence {
    pub check: CheckEvidence,
    pub popup_size: Option<PopupSize>,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QaSubject {
    pub commit: String,
    pub binary_blake3: String,
    pub binary_identity_verified: bool,
    pub worktree_clean: bool,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualCase {
    pub name: String,
    pub status: CheckStatus,
    pub notes: String,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualAttestation {
    pub schema: u32,
    pub mode: AttestationMode,
    pub subject: QaSubjectBinding,
    pub completed_at_unix: u64,
    pub reviewer: String,
    pub cases: Vec<ManualCase>,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QaSubjectBinding {
    pub commit: String,
    pub binary_blake3: String,
    pub topology_fingerprint: String,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationMode {
    Diagnostic,
    Strict,
}

#[cfg(any(test, feature = "visual-qa"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeVerdict {
    Passed,
    Blocked,
    Failed,
}

#[cfg(feature = "visual-qa")]
#[derive(Serialize)]
pub(super) struct NativeReleaseEvidence {
    pub schema: u32,
    pub mode: NativeQaMode,
    pub generated_at_unix: u64,
    pub subject: QaSubject,
    pub macos_build: String,
    pub architecture: String,
    pub capabilities: NativeCapabilities,
    pub topology_fingerprint: String,
    pub topology: Vec<DisplayTopology>,
    pub automated_checks: Vec<CheckEvidence>,
    pub manual_attestation: CheckEvidence,
    pub verdict: NativeVerdict,
    pub reasons: Vec<String>,
}
