//! Fail-closed native release QA.
//!
//! Diagnostic collection never asks macOS for TCC permissions. Strict release
//! policy is intentionally impossible to pass with blocked capabilities,
//! mismatched evidence, or an incomplete human VoiceOver attestation.

#[cfg(feature = "visual-qa")]
mod artifact;
mod contract;
mod macos_probe;
mod policy;

#[cfg(feature = "visual-qa")]
pub use artifact::run;
#[cfg(feature = "visual-qa")]
pub use contract::{CheckEvidence, NativeMenuRendererEvidence, NativeQaRequest};
pub use contract::{PopupSize, ScreenPoint};
pub use macos_probe::capture_display_topology;
pub use policy::place_popup;
