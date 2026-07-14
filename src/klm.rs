//! Keystroke-Level Model fixtures for core keyboard workflows.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operator {
    M,
    K,
    P,
    H,
    R,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorStep {
    pub operator: Operator,
    pub repetitions: u32,
    #[serde(default)]
    pub response_ms: Option<u64>,
    pub action: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    Keyboard,
    Pointer,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFixture {
    pub id: String,
    pub title: String,
    pub input_mode: InputMode,
    pub minimum_user_operators: u32,
    pub allowed_avoidable_growth: u32,
    pub steps: Vec<OperatorStep>,
}

impl WorkflowFixture {
    pub fn user_operators(&self) -> u32 {
        self.steps
            .iter()
            .filter(|step| step.operator != Operator::R)
            .map(|step| step.repetitions)
            .sum()
    }

    pub fn estimated_duration_ms(&self, model: &OperatorDurations) -> u64 {
        self.steps
            .iter()
            .map(|step| {
                let per_operator = match step.operator {
                    Operator::M => model.mental_ms,
                    Operator::K => model.keystroke_ms,
                    Operator::P => model.pointing_ms,
                    Operator::H => model.homing_ms,
                    Operator::R => step.response_ms.unwrap_or_default(),
                };
                per_operator.saturating_mul(u64::from(step.repetitions))
            })
            .sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorDurations {
    pub mental_ms: u64,
    pub keystroke_ms: u64,
    pub pointing_ms: u64,
    pub homing_ms: u64,
    pub shortcut_chord_policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KlmSuite {
    pub schema: u32,
    pub model: OperatorDurations,
    pub workflows: Vec<WorkflowFixture>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KlmViolation {
    pub workflow: Option<String>,
    pub detail: String,
}

impl std::fmt::Display for KlmViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.workflow {
            Some(workflow) => write!(formatter, "{workflow}: {}", self.detail),
            None => formatter.write_str(&self.detail),
        }
    }
}

pub const CORE_WORKFLOW_IDS: [&str; 10] = [
    "switch-pane",
    "open-directory",
    "copy-selection",
    "move-selection",
    "rename-entry",
    "delete-selection",
    "go-to-path",
    "find-files",
    "toggle-preview",
    "undo-operation",
];

pub fn ci_suite() -> Result<KlmSuite, serde_json::Error> {
    serde_json::from_str(include_str!("../ci/klm-workflows.json"))
}

pub fn evaluate(suite: &KlmSuite) -> Vec<KlmViolation> {
    let mut violations = Vec::new();
    if suite.schema != 1 {
        violations.push(KlmViolation {
            workflow: None,
            detail: "KLM schema must be 1".to_string(),
        });
    }
    if suite.model.shortcut_chord_policy.trim().is_empty()
        || [
            suite.model.mental_ms,
            suite.model.keystroke_ms,
            suite.model.pointing_ms,
            suite.model.homing_ms,
        ]
        .contains(&0)
    {
        violations.push(KlmViolation {
            workflow: None,
            detail: "KLM operator durations and shortcut policy must be explicit".to_string(),
        });
    }

    let expected = CORE_WORKFLOW_IDS.into_iter().collect::<HashSet<_>>();
    let actual = suite
        .workflows
        .iter()
        .map(|workflow| workflow.id.as_str())
        .collect::<HashSet<_>>();
    if suite.workflows.len() != CORE_WORKFLOW_IDS.len() || actual != expected {
        violations.push(KlmViolation {
            workflow: None,
            detail: "suite must contain each of the ten core workflows exactly once".to_string(),
        });
    }

    for workflow in &suite.workflows {
        if workflow.title.trim().is_empty() || workflow.steps.is_empty() {
            violations.push(KlmViolation {
                workflow: Some(workflow.id.clone()),
                detail: "workflow needs a title and operator sequence".to_string(),
            });
            continue;
        }
        if workflow
            .steps
            .iter()
            .any(|step| step.repetitions == 0 || step.action.trim().is_empty())
        {
            violations.push(KlmViolation {
                workflow: Some(workflow.id.clone()),
                detail: "every operator needs a positive repetition count and action".to_string(),
            });
        }
        if workflow
            .steps
            .iter()
            .any(|step| step.operator == Operator::R && step.response_ms.is_none())
        {
            violations.push(KlmViolation {
                workflow: Some(workflow.id.clone()),
                detail: "response operators need an explicit measured duration".to_string(),
            });
        }
        if workflow.input_mode == InputMode::Keyboard
            && workflow
                .steps
                .iter()
                .any(|step| matches!(step.operator, Operator::P | Operator::H))
        {
            violations.push(KlmViolation {
                workflow: Some(workflow.id.clone()),
                detail: "keyboard workflow unexpectedly requires pointing or homing".to_string(),
            });
        }
        let maximum = workflow
            .minimum_user_operators
            .saturating_add(workflow.allowed_avoidable_growth);
        let actual = workflow.user_operators();
        if actual > maximum {
            violations.push(KlmViolation {
                workflow: Some(workflow.id.clone()),
                detail: format!("{actual} user operators exceed the reviewed maximum {maximum}"),
            });
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_core_workflows_stay_within_operator_budgets() {
        let suite = ci_suite().unwrap();
        let violations = evaluate(&suite);
        assert!(
            violations.is_empty(),
            "{}",
            violations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(
            suite
                .workflows
                .iter()
                .all(|workflow| workflow.estimated_duration_ms(&suite.model) > 0)
        );
    }

    #[test]
    fn adding_an_operator_without_reviewing_the_budget_fails() {
        let mut suite = ci_suite().unwrap();
        suite.workflows[0].steps.push(OperatorStep {
            operator: Operator::K,
            repetitions: 1,
            response_ms: None,
            action: "avoidable extra confirmation".to_string(),
        });
        assert!(evaluate(&suite).iter().any(|violation| {
            violation.workflow.as_deref() == Some("switch-pane")
                && violation.detail.contains("exceed")
        }));
    }
}
