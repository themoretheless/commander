#[cfg(any(test, feature = "visual-qa"))]
use super::contract::{
    ATTESTATION_SCHEMA, CheckEvidence, CheckStatus, ManualAttestation, ManualCase,
    NativeCapabilities, NativeVerdict, QaSubject, QaSubjectBinding, REQUIRED_MANUAL_CASES,
};
use super::contract::{DisplayTopology, PopupPlacement, PopupSize, ScreenPoint, ScreenRect};
#[cfg(any(test, feature = "visual-qa"))]
use std::time::Duration;

#[cfg(any(test, feature = "visual-qa"))]
const ATTESTATION_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
#[cfg(any(test, feature = "visual-qa"))]
const FUTURE_CLOCK_TOLERANCE: Duration = Duration::from_secs(5 * 60);

/// Place a full menu content rectangle inside one NSScreen visibleFrame.
///
/// `anchor` is AppKit global-screen top-left. Menu content grows toward lower
/// y, so the returned rectangle spans `[top_left.y - height, top_left.y]`.
pub fn place_popup(
    anchor: ScreenPoint,
    size: PopupSize,
    displays: &[DisplayTopology],
) -> Option<PopupPlacement> {
    if !anchor.x.is_finite() || !anchor.y.is_finite() || !size.is_valid() {
        return None;
    }
    let display = displays
        .iter()
        .filter(|display| display.frame.is_valid() && display.visible_frame.is_valid())
        .min_by(|left, right| {
            let left_distance = if left.frame.contains(anchor) {
                -1.0
            } else {
                left.frame.distance_squared(anchor)
            };
            let right_distance = if right.frame.contains(anchor) {
                -1.0
            } else {
                right.frame.distance_squared(anchor)
            };
            left_distance.total_cmp(&right_distance)
        })?;
    let visible = display.visible_frame;
    if size.width > visible.width() || size.height > visible.height() {
        return None;
    }
    let top_left = ScreenPoint {
        x: anchor.x.clamp(visible.min_x, visible.max_x - size.width),
        y: anchor.y.clamp(visible.min_y + size.height, visible.max_y),
    };
    let content_rect = ScreenRect::new(
        top_left.x,
        top_left.y - size.height,
        size.width,
        size.height,
    );
    visible.contains_rect(content_rect).then(|| PopupPlacement {
        display_id: display.id.clone(),
        top_left,
        content_rect,
    })
}

#[cfg(any(test, feature = "visual-qa"))]
pub(super) fn popup_placement_matrix(
    displays: &[DisplayTopology],
    popup_size: Option<PopupSize>,
) -> CheckEvidence {
    let Some(popup_size) = popup_size else {
        return CheckEvidence::failed(
            "popup_content_rect_matrix",
            "actual NSMenu size was unavailable",
        );
    };
    if displays.is_empty() {
        return CheckEvidence::failed("popup_content_rect_matrix", "no NSScreen topology");
    }
    let mut samples = 0_usize;
    for display in displays {
        if !display.frame.is_valid()
            || !display.visible_frame.is_valid()
            || !display.backing_scale.is_finite()
            || display.backing_scale < 1.0
        {
            return CheckEvidence::failed(
                "popup_content_rect_matrix",
                format!("invalid display geometry: {}", display.id),
            );
        }
        let frame = display.frame;
        let edge_inset = 1.0 / display.backing_scale.max(1.0);
        let center = ScreenPoint {
            x: (frame.min_x + frame.max_x) / 2.0,
            y: (frame.min_y + frame.max_y) / 2.0,
        };
        for anchor in [
            center,
            ScreenPoint {
                x: frame.min_x + edge_inset,
                y: frame.min_y + edge_inset,
            },
            ScreenPoint {
                x: frame.min_x + edge_inset,
                y: frame.max_y - edge_inset,
            },
            ScreenPoint {
                x: frame.max_x - edge_inset,
                y: frame.min_y + edge_inset,
            },
            ScreenPoint {
                x: frame.max_x - edge_inset,
                y: frame.max_y - edge_inset,
            },
        ] {
            let Some(placement) = place_popup(anchor, popup_size, displays) else {
                return CheckEvidence::failed(
                    "popup_content_rect_matrix",
                    format!(
                        "{} popup does not fit on {}",
                        size_label(popup_size),
                        display.id
                    ),
                );
            };
            if placement.display_id != display.id
                || !display.visible_frame.contains_rect(placement.content_rect)
            {
                return CheckEvidence::failed(
                    "popup_content_rect_matrix",
                    format!("popup content escaped visibleFrame on {}", display.id),
                );
            }
            samples += 1;
        }
    }
    CheckEvidence::passed(
        "popup_content_rect_matrix",
        format!(
            "{samples} full-rect placements for {} across {} display(s)",
            size_label(popup_size),
            displays.len()
        ),
    )
}

#[cfg(any(test, feature = "visual-qa"))]
fn size_label(size: PopupSize) -> String {
    format!("{:.1}x{:.1}", size.width, size.height)
}

#[cfg(any(test, feature = "visual-qa"))]
pub(super) fn mixed_scale_topology(displays: &[DisplayTopology]) -> CheckEvidence {
    let has_1x = displays
        .iter()
        .any(|display| (display.backing_scale - 1.0).abs() <= 0.05);
    let has_hidpi = displays.iter().any(|display| display.backing_scale >= 1.95);
    if displays.len() >= 2 && has_1x && has_hidpi {
        CheckEvidence::passed(
            "mixed_scale_topology",
            format!("{} display(s), both 1x and 2x present", displays.len()),
        )
    } else {
        CheckEvidence::blocked(
            "mixed_scale_topology",
            format!(
                "{} display(s), has_1x={has_1x}, has_2x={has_hidpi}",
                displays.len()
            ),
        )
    }
}

#[cfg(any(test, feature = "visual-qa"))]
pub(super) fn attestation_template(subject: &QaSubjectBinding) -> ManualAttestation {
    ManualAttestation {
        schema: ATTESTATION_SCHEMA,
        subject: subject.clone(),
        completed_at_unix: 0,
        reviewer: String::new(),
        cases: REQUIRED_MANUAL_CASES
            .into_iter()
            .map(|name| ManualCase {
                name: name.to_string(),
                status: CheckStatus::NotRun,
                notes: String::new(),
            })
            .collect(),
    }
}

#[cfg(any(test, feature = "visual-qa"))]
pub(super) fn validate_attestation(
    attestation: &ManualAttestation,
    expected: &QaSubjectBinding,
    now: u64,
) -> CheckEvidence {
    if attestation.schema != ATTESTATION_SCHEMA {
        return CheckEvidence::failed(
            "manual_attestation",
            format!(
                "schema {} does not match {}",
                attestation.schema, ATTESTATION_SCHEMA
            ),
        );
    }
    if &attestation.subject != expected {
        return CheckEvidence::failed(
            "manual_attestation",
            "commit, binary digest, or topology fingerprint does not match",
        );
    }
    if attestation.reviewer.trim().is_empty() {
        return CheckEvidence::blocked("manual_attestation", "reviewer is empty");
    }
    if attestation.cases.len() != REQUIRED_MANUAL_CASES.len() {
        return CheckEvidence::failed(
            "manual_attestation",
            format!(
                "expected exactly {} manual cases, found {}",
                REQUIRED_MANUAL_CASES.len(),
                attestation.cases.len()
            ),
        );
    }
    let newest_allowed = now.saturating_add(FUTURE_CLOCK_TOLERANCE.as_secs());
    if attestation.completed_at_unix == 0 || attestation.completed_at_unix > newest_allowed {
        return CheckEvidence::failed(
            "manual_attestation",
            "completion timestamp is missing or in the future",
        );
    }
    if now.saturating_sub(attestation.completed_at_unix) > ATTESTATION_MAX_AGE.as_secs() {
        return CheckEvidence::failed("manual_attestation", "attestation is older than seven days");
    }
    for required in REQUIRED_MANUAL_CASES {
        let matching = attestation
            .cases
            .iter()
            .filter(|case| case.name == required)
            .collect::<Vec<_>>();
        if matching.len() != 1 || matching[0].status != CheckStatus::Passed {
            return CheckEvidence::blocked(
                "manual_attestation",
                format!("{required} is missing, duplicated, or not passed"),
            );
        }
    }
    CheckEvidence::passed(
        "manual_attestation",
        format!("{} exact-subject cases passed", REQUIRED_MANUAL_CASES.len()),
    )
}

#[cfg(any(test, feature = "visual-qa"))]
pub(super) fn evaluate_verdict(
    capabilities: &NativeCapabilities,
    subject: &QaSubject,
    checks: &[CheckEvidence],
    attestation: &CheckEvidence,
) -> (NativeVerdict, Vec<String>) {
    let mut reasons = Vec::new();
    if !subject.worktree_clean {
        reasons.push("worktree is not clean".to_string());
    }
    if subject.commit == "unknown" {
        reasons.push("subject commit is unknown".to_string());
    }
    if !capabilities.release_ready() {
        reasons.push("one or more native capabilities are unavailable".to_string());
    }
    for check in checks {
        if check.status != CheckStatus::Passed {
            reasons.push(format!("{} is {:?}", check.name, check.status));
        }
    }
    if attestation.status != CheckStatus::Passed {
        reasons.push(format!("manual attestation is {:?}", attestation.status));
    }
    if reasons.is_empty() {
        return (NativeVerdict::Passed, reasons);
    }
    let hard_failure = checks
        .iter()
        .any(|check| check.status == CheckStatus::Failed)
        || attestation.status == CheckStatus::Failed
        || subject.commit == "unknown";
    (
        if hard_failure {
            NativeVerdict::Failed
        } else {
            NativeVerdict::Blocked
        },
        reasons,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_release_qa::contract::{CapabilityState, NativeCapabilities, QaSubject};

    fn mixed_topology() -> Vec<DisplayTopology> {
        vec![
            DisplayTopology {
                id: "left-1x".to_string(),
                main: false,
                frame: ScreenRect::new(-1920.0, 0.0, 1920.0, 1080.0),
                visible_frame: ScreenRect::new(-1920.0, 24.0, 1920.0, 1056.0),
                backing_scale: 1.0,
            },
            DisplayTopology {
                id: "main-2x".to_string(),
                main: true,
                frame: ScreenRect::new(0.0, 0.0, 1512.0, 982.0),
                visible_frame: ScreenRect::new(0.0, 38.0, 1512.0, 944.0),
                backing_scale: 2.0,
            },
            DisplayTopology {
                id: "upper-2x".to_string(),
                main: false,
                frame: ScreenRect::new(100.0, 982.0, 1440.0, 900.0),
                visible_frame: ScreenRect::new(100.0, 1006.0, 1440.0, 876.0),
                backing_scale: 2.0,
            },
        ]
    }

    #[test]
    fn popup_matrix_contains_full_rect_on_negative_vertical_and_mixed_displays() {
        let topology = mixed_topology();
        let size = PopupSize {
            width: 420.0,
            height: 720.0,
        };
        assert_eq!(
            popup_placement_matrix(&topology, Some(size)).status,
            CheckStatus::Passed
        );
        assert_eq!(mixed_scale_topology(&topology).status, CheckStatus::Passed);
        let placed = place_popup(ScreenPoint { x: -1919.0, y: 1.0 }, size, &topology)
            .expect("negative-coordinate display");
        assert_eq!(placed.display_id, "left-1x");
        assert!(topology[0].visible_frame.contains_rect(placed.content_rect));
    }

    #[test]
    fn oversized_popup_fails_instead_of_clipping() {
        let topology = mixed_topology();
        assert!(
            place_popup(
                ScreenPoint { x: 100.0, y: 100.0 },
                PopupSize {
                    width: 10_000.0,
                    height: 10_000.0,
                },
                &topology,
            )
            .is_none()
        );
    }

    #[test]
    fn attestation_is_bound_to_subject_and_freshness() {
        let binding = QaSubjectBinding {
            commit: "abc".to_string(),
            binary_blake3: "digest".to_string(),
            topology_fingerprint: "topology".to_string(),
        };
        let now = 1_000_000;
        let mut attestation = attestation_template(&binding);
        attestation.completed_at_unix = now;
        attestation.reviewer = "Release QA".to_string();
        for case in &mut attestation.cases {
            case.status = CheckStatus::Passed;
        }
        assert_eq!(
            validate_attestation(&attestation, &binding, now).status,
            CheckStatus::Passed
        );
        attestation.subject.binary_blake3 = "other".to_string();
        assert_eq!(
            validate_attestation(&attestation, &binding, now).status,
            CheckStatus::Failed
        );
        attestation.subject = binding.clone();
        attestation.completed_at_unix = now - ATTESTATION_MAX_AGE.as_secs() - 1;
        assert_eq!(
            validate_attestation(&attestation, &binding, now).status,
            CheckStatus::Failed
        );
        attestation.completed_at_unix = now;
        attestation.cases.push(ManualCase {
            name: "unexpected".to_string(),
            status: CheckStatus::Passed,
            notes: String::new(),
        });
        assert_eq!(
            validate_attestation(&attestation, &binding, now).status,
            CheckStatus::Failed
        );
    }

    #[test]
    fn release_verdict_fails_closed_for_blocked_checks() {
        let subject = QaSubject {
            commit: "abc".to_string(),
            binary_blake3: "digest".to_string(),
            worktree_clean: true,
        };
        let capabilities = NativeCapabilities {
            window_server: CapabilityState::Available,
            accessibility: CapabilityState::Denied,
            screen_recording: CapabilityState::Denied,
            voice_over: CapabilityState::NotRunning,
        };
        let checks = [CheckEvidence::passed("model", "ok")];
        let attestation = CheckEvidence::blocked("manual_attestation", "not run");
        assert_eq!(
            evaluate_verdict(&capabilities, &subject, &checks, &attestation).0,
            NativeVerdict::Blocked
        );
    }

    #[test]
    fn release_verdict_never_accepts_a_non_passed_check() {
        let subject = QaSubject {
            commit: "abc".to_string(),
            binary_blake3: "digest".to_string(),
            worktree_clean: true,
        };
        let capabilities = NativeCapabilities {
            window_server: CapabilityState::Available,
            accessibility: CapabilityState::Available,
            screen_recording: CapabilityState::Available,
            voice_over: CapabilityState::Available,
        };
        let attestation = CheckEvidence::passed("manual_attestation", "complete");
        for (status, expected) in [
            (CheckStatus::Blocked, NativeVerdict::Blocked),
            (CheckStatus::NotRun, NativeVerdict::Blocked),
            (CheckStatus::Failed, NativeVerdict::Failed),
        ] {
            let checks = [CheckEvidence {
                name: "required_check".to_string(),
                status,
                detail: String::new(),
            }];
            assert_eq!(
                evaluate_verdict(&capabilities, &subject, &checks, &attestation).0,
                expected
            );
        }
    }
}
