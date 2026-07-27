//! UI-independent presentation contract for long-running filesystem work.
//!
//! Keeping these values outside egui makes operation state truthful across
//! the progress window, queue, notifications, recovery, and accessibility.

use std::path::{Path, PathBuf};

use crate::operation::OperationId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OperationPhase {
    #[default]
    Scan,
    Plan,
    Transfer,
    Verify,
    Finalize,
}

impl OperationPhase {
    pub const ALL: [Self; 5] = [
        Self::Scan,
        Self::Plan,
        Self::Transfer,
        Self::Verify,
        Self::Finalize,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Scan => "Scan",
            Self::Plan => "Plan",
            Self::Transfer => "Transfer",
            Self::Verify => "Verify",
            Self::Finalize => "Finalize",
        }
    }

    pub fn ordinal(self) -> usize {
        Self::ALL
            .iter()
            .position(|phase| *phase == self)
            .expect("operation phase belongs to ALL")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseReason {
    QuietHours { resume_at: Option<String> },
    MountDisconnected { label: String, timeout_secs: u64 },
    User,
}

impl PauseReason {
    pub fn label(&self) -> String {
        match self {
            Self::QuietHours { .. } => "Paused for quiet hours".to_string(),
            Self::MountDisconnected { label, .. } => format!("{label} disconnected"),
            Self::User => "Paused by you".to_string(),
        }
    }

    pub fn resume_condition(&self) -> String {
        match self {
            Self::QuietHours {
                resume_at: Some(time),
            } => format!("Resumes automatically at {time}"),
            Self::QuietHours { resume_at: None } => {
                "Resumes automatically when quiet hours end".to_string()
            }
            Self::MountDisconnected {
                label,
                timeout_secs,
            } => format!(
                "Resumes when the same {label} mount returns, for up to {timeout_secs} seconds"
            ),
            Self::User => "Resumes when you choose Resume".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancellationAction {
    StopAfterCurrentFile,
    CancelPending,
    CancelCurrent,
    RollBack,
}

impl CancellationAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::StopAfterCurrentFile => "Stop after current file",
            Self::CancelPending => "Cancel pending",
            Self::CancelCurrent => "Cancel current",
            Self::RollBack => "Roll back",
        }
    }

    pub fn consequence(self) -> &'static str {
        match self {
            Self::StopAfterCurrentFile => {
                "Finishes the current file, then keeps a recoverable checkpoint"
            }
            Self::CancelPending => "Removes queued work without interrupting the current file",
            Self::CancelCurrent => {
                "Stops current work and keeps completed files plus a recoverable checkpoint"
            }
            Self::RollBack => "Reverses completed placements after a recovery review",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedSummary {
    pub verb: String,
    pub item_count: usize,
    pub item_names: Vec<String>,
    pub source_roots: Vec<PathBuf>,
    pub destination: PathBuf,
}

impl SubmittedSummary {
    pub fn capture(verb: impl Into<String>, paths: &[PathBuf], destination: PathBuf) -> Self {
        let mut source_roots = paths
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<Vec<_>>();
        source_roots.sort();
        source_roots.dedup();
        Self {
            verb: verb.into(),
            item_count: paths.len(),
            item_names: paths
                .iter()
                .take(3)
                .map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                })
                .collect(),
            source_roots,
            destination,
        }
    }

    pub fn label(&self) -> String {
        let item = if self.item_count == 1 {
            "item"
        } else {
            "items"
        };
        format!(
            "{} {} {} -> {}",
            self.verb,
            self.item_count,
            item,
            path_label(&self.destination)
        )
    }

    pub fn detail(&self) -> String {
        let mut names = self.item_names.join(", ");
        if self.item_count > self.item_names.len() {
            names.push_str(&format!(
                " +{} more",
                self.item_count - self.item_names.len()
            ));
        }
        let roots = match self.source_roots.as_slice() {
            [] => "unknown source".to_string(),
            [root] => root.display().to_string(),
            roots => format!("{} source folders", roots.len()),
        };
        format!("{names} from {roots}")
    }
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureNotice {
    pub operation_id: OperationId,
    pub summary: SubmittedSummary,
    pub errors: Vec<String>,
    pub failures: Vec<crate::operation::ClassifiedFailure>,
    pub created_at_millis: u64,
}

impl FailureNotice {
    pub fn title(&self) -> String {
        let noun = if self.errors.len() == 1 {
            "error"
        } else {
            "errors"
        };
        format!("{} {}", self.errors.len(), noun)
    }
}

#[derive(Default)]
pub struct FailureInbox {
    notices: Vec<FailureNotice>,
}

impl FailureInbox {
    const MAX_NOTICES: usize = 100;

    pub fn upsert(&mut self, notice: FailureNotice) {
        if let Some(existing) = self
            .notices
            .iter_mut()
            .find(|existing| existing.operation_id == notice.operation_id)
        {
            *existing = notice;
            return;
        }
        self.notices.insert(0, notice);
        self.notices.truncate(Self::MAX_NOTICES);
    }

    pub fn dismiss(&mut self, operation_id: &OperationId) -> bool {
        let before = self.notices.len();
        self.notices
            .retain(|notice| &notice.operation_id != operation_id);
        self.notices.len() != before
    }

    pub fn notices(&self) -> &[FailureNotice] {
        &self.notices
    }

    pub fn is_empty(&self) -> bool {
        self.notices.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragEffect {
    Move,
    Copy,
}

impl DragEffect {
    fn label(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Copy => "Copy",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DragAnnouncement {
    Valid {
        destination: PathBuf,
        effect: DragEffect,
    },
    Rejected {
        reason: String,
    },
}

impl DragAnnouncement {
    pub fn valid(destination: PathBuf, effect: DragEffect) -> Self {
        Self::Valid {
            destination,
            effect,
        }
    }

    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
        }
    }

    pub fn text(&self) -> String {
        match self {
            Self::Valid {
                destination,
                effect,
            } => format!("{} to {}", effect.label(), path_label(destination)),
            Self::Rejected { reason } => format!("Cannot drop: {reason}"),
        }
    }
}

/// Content-scroll delta in pixels/second. Positive moves content down near the
/// top edge; negative moves content up near the bottom edge.
pub fn edge_autoscroll_velocity(
    pointer_y: f32,
    top: f32,
    bottom: f32,
    edge_width: f32,
    max_speed: f32,
) -> f32 {
    if edge_width <= 0.0 || max_speed <= 0.0 || bottom <= top {
        return 0.0;
    }
    if pointer_y < top || pointer_y > bottom {
        return 0.0;
    }
    if pointer_y < top + edge_width {
        let proximity = 1.0 - (pointer_y - top) / edge_width;
        return max_speed * proximity.clamp(0.0, 1.0).powi(2);
    }
    if pointer_y > bottom - edge_width {
        let proximity = 1.0 - (bottom - pointer_y) / edge_width;
        return -max_speed * proximity.clamp(0.0, 1.0).powi(2);
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_are_ordered_and_named() {
        assert_eq!(OperationPhase::ALL[2], OperationPhase::Transfer);
        assert_eq!(OperationPhase::Verify.ordinal(), 3);
        assert_eq!(OperationPhase::Finalize.label(), "Finalize");
    }

    #[test]
    fn pause_reason_states_exact_resume_condition() {
        let reason = PauseReason::MountDisconnected {
            label: "Destination volume".to_string(),
            timeout_secs: 30,
        };
        assert_eq!(reason.label(), "Destination volume disconnected");
        assert!(
            reason
                .resume_condition()
                .contains("same Destination volume mount")
        );
        assert!(reason.resume_condition().contains("30 seconds"));
    }

    #[test]
    fn cancellation_labels_describe_consequences() {
        assert_eq!(
            CancellationAction::StopAfterCurrentFile.label(),
            "Stop after current file"
        );
        assert!(
            CancellationAction::CancelPending
                .consequence()
                .contains("without interrupting")
        );
        assert!(
            CancellationAction::RollBack
                .consequence()
                .contains("recovery review")
        );
    }

    #[test]
    fn submitted_summary_is_a_frozen_snapshot() {
        let mut paths = vec![PathBuf::from("/source/a"), PathBuf::from("/source/b")];
        let summary = SubmittedSummary::capture("Move", &paths, PathBuf::from("/target"));
        paths.clear();
        assert_eq!(summary.label(), "Move 2 items -> target");
        assert_eq!(summary.item_names, ["a", "b"]);
        assert!(summary.detail().contains("/source"));
    }

    #[test]
    fn failure_inbox_upserts_and_requires_explicit_dismissal() {
        let operation_id = OperationId("op-1".to_string());
        let summary = SubmittedSummary::capture(
            "Copy",
            &[PathBuf::from("/source/a")],
            PathBuf::from("/target"),
        );
        let mut inbox = FailureInbox::default();
        inbox.upsert(FailureNotice {
            operation_id: operation_id.clone(),
            summary: summary.clone(),
            errors: vec!["first".to_string()],
            failures: Vec::new(),
            created_at_millis: 1,
        });
        inbox.upsert(FailureNotice {
            operation_id: operation_id.clone(),
            summary,
            errors: vec!["new".to_string(), "second".to_string()],
            failures: Vec::new(),
            created_at_millis: 2,
        });
        assert_eq!(inbox.notices().len(), 1);
        assert_eq!(inbox.notices()[0].title(), "2 errors");
        assert!(inbox.dismiss(&operation_id));
        assert!(inbox.is_empty());
    }

    #[test]
    fn drag_announcement_names_effect_destination_and_rejection() {
        assert_eq!(
            DragAnnouncement::valid(PathBuf::from("/target/folder"), DragEffect::Move).text(),
            "Move to folder"
        );
        assert_eq!(
            DragAnnouncement::rejected("destination is read-only").text(),
            "Cannot drop: destination is read-only"
        );
        assert_eq!(
            DragAnnouncement::valid(PathBuf::from("/target/folder"), DragEffect::Copy).text(),
            "Copy to folder"
        );
    }

    #[test]
    fn edge_autoscroll_is_bounded_and_eases_toward_the_center() {
        let top = edge_autoscroll_velocity(0.0, 0.0, 200.0, 40.0, 600.0);
        let near_top = edge_autoscroll_velocity(20.0, 0.0, 200.0, 40.0, 600.0);
        let center = edge_autoscroll_velocity(100.0, 0.0, 200.0, 40.0, 600.0);
        let bottom = edge_autoscroll_velocity(200.0, 0.0, 200.0, 40.0, 600.0);
        assert_eq!(top, 600.0);
        assert!(near_top > 0.0 && near_top < top);
        assert_eq!(center, 0.0);
        assert_eq!(bottom, -600.0);
    }

    #[test]
    fn edge_autoscroll_ignores_invalid_or_outside_geometry() {
        assert_eq!(edge_autoscroll_velocity(-1.0, 0.0, 200.0, 40.0, 600.0), 0.0);
        assert_eq!(edge_autoscroll_velocity(10.0, 20.0, 10.0, 40.0, 600.0), 0.0);
        assert_eq!(edge_autoscroll_velocity(10.0, 0.0, 200.0, 0.0, 600.0), 0.0);
    }
}
