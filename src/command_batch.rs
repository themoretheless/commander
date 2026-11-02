//! Sequential per-item command expansion with bounded aggregate output.
use crate::{
    app::CommandOutput,
    cmdtemplate::{ExecutionMode, SelectionCtx},
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
const OUTPUT_CAP: usize = 1024 * 1024;
#[derive(Default)]
pub struct Progress {
    pub cancelled: AtomicBool,
    pub completed: AtomicUsize,
    pub succeeded: AtomicUsize,
    pub failed: AtomicUsize,
    pub total: usize,
}
impl Progress {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            ..Default::default()
        }
    }
}
#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct Report {
    pub total: usize,
    pub completed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub stopped_on_error: bool,
}
struct Log {
    bytes: Vec<u8>,
    dropped: usize,
}
impl Log {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            dropped: 0,
        }
    }
    fn push(&mut self, text: &str) {
        let kept = text.len().min(OUTPUT_CAP.saturating_sub(self.bytes.len()));
        self.bytes.extend_from_slice(&text.as_bytes()[..kept]);
        self.dropped = self.dropped.saturating_add(text.len() - kept);
    }
    fn finish(self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.dropped > 0 {
            text.push_str(&format!(
                "\n… aggregate output truncated ({} bytes not shown)",
                self.dropped
            ));
        }
        text
    }
}
fn cancelled_output(raw: String) -> CommandOutput {
    CommandOutput {
        cmdline: raw,
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        error: Some("Command cancelled before invocation".into()),
        batch: None,
        context: None,
    }
}
/// `invoke` owns process creation/capture; the plan never reads live panel state.
pub fn run(
    raw: String,
    mode: ExecutionMode,
    context: &SelectionCtx,
    progress: &Progress,
    mut invoke: impl FnMut(String) -> CommandOutput,
) -> CommandOutput {
    if !mode.per_item() {
        if progress.cancelled.load(Ordering::Acquire) {
            return cancelled_output(raw);
        }
        let output = invoke(crate::cmdtemplate::expand(&raw, context));
        progress.completed.store(1, Ordering::Release);
        if output.error.is_none() && output.exit_code == Some(0) {
            progress.succeeded.store(1, Ordering::Release);
        } else {
            progress.failed.store(1, Ordering::Release);
        }
        return output;
    }
    let mut out = Log::new();
    let mut err = Log::new();
    let mut first_failure = None;
    let mut first_error = None;
    let mut stopped_on_error = false;
    for (index, path) in context.paths.iter().enumerate() {
        if progress.cancelled.load(Ordering::Acquire) {
            break;
        }
        let single = SelectionCtx {
            paths: vec![path.clone()],
            dir: context.dir.clone(),
            dir_other: context.dir_other.clone(),
        };
        let result = invoke(crate::cmdtemplate::expand(&raw, &single));
        let label = format!(
            "\n--- item {} / {}: {} ---\n",
            index + 1,
            context.paths.len(),
            path.display()
        );
        out.push(&label);
        out.push(&result.stdout);
        if !result.stderr.is_empty() {
            err.push(&label);
            err.push(&result.stderr);
        }
        let success = result.error.is_none() && result.exit_code == Some(0);
        if success {
            progress.succeeded.fetch_add(1, Ordering::Release);
        } else {
            progress.failed.fetch_add(1, Ordering::Release);
            first_failure.get_or_insert(result.exit_code.unwrap_or(1));
            err.push(&label);
            err.push(&match (&result.error, result.exit_code) {
                (Some(error), _) => format!("Invocation failed: {error}\n"),
                (None, Some(code)) => format!("Invocation failed: exit {code}\n"),
                (None, None) => "Invocation terminated by signal\n".to_string(),
            });

            if let Some(error) = result.error {
                err.push(&label);
                err.push(&error);
                first_error.get_or_insert(error);
            }
        }
        progress.completed.fetch_add(1, Ordering::Release);
        if !success
            && mode == ExecutionMode::PerItemStop
            && !progress.cancelled.load(Ordering::Acquire)
        {
            stopped_on_error = true;
            break;
        }
    }
    let report = Report {
        total: context.paths.len(),
        completed: progress.completed.load(Ordering::Acquire),
        succeeded: progress.succeeded.load(Ordering::Acquire),
        failed: progress.failed.load(Ordering::Acquire),
        cancelled: progress.cancelled.load(Ordering::Acquire),
        stopped_on_error,
    };
    let summary = format!(
        "Per item: {} / {} completed · {} succeeded · {} failed · {} remaining{}{}\n",
        report.completed,
        report.total,
        report.succeeded,
        report.failed,
        report.total.saturating_sub(report.completed),
        if report.cancelled {
            " · cancelled"
        } else {
            ""
        },
        if report.stopped_on_error {
            " · stopped on error"
        } else {
            ""
        }
    );
    let error = if report.cancelled {
        Some("Per-item command cancelled".into())
    } else if report.total == 0 {
        Some("Per-item mode needs at least one opening item".into())
    } else {
        first_error
    };
    CommandOutput {
        cmdline: format!("Per item: {raw}"),
        stdout: summary + &out.finish(),
        stderr: err.finish(),
        exit_code: if report.cancelled || report.total == 0 {
            None
        } else {
            Some(first_failure.unwrap_or(0))
        },
        error,
        batch: Some(report),
        context: None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> SelectionCtx {
        SelectionCtx {
            paths: vec![
                "/a/one file.txt".into(),
                "/a/-two.txt".into(),
                "/a/three.txt".into(),
            ],
            dir: "/a".into(),
            dir_other: "/other".into(),
        }
    }
    fn output(line: String, code: i32) -> CommandOutput {
        CommandOutput {
            cmdline: line,
            stdout: "output\n".into(),
            stderr: String::new(),
            exit_code: Some(code),
            error: None,
            batch: None,
            context: None,
        }
    }
    #[test]
    fn per_item_expands_exactly_one_path_and_preserves_opening_order_and_other_directory() {
        let progress = Progress::new(3);
        let mut lines = Vec::new();
        let result = run(
            "tool {paths} {names} {dir_other}".into(),
            ExecutionMode::PerItemContinue,
            &context(),
            &progress,
            |line| {
                lines.push(line.clone());
                output(line, 0)
            },
        );
        assert_eq!(
            lines,
            [
                "tool '/a/one file.txt' 'one file.txt' '/other'",
                "tool '/a/-two.txt' './-two.txt' '/other'",
                "tool '/a/three.txt' 'three.txt' '/other'"
            ]
        );
        assert_eq!(
            result.batch.unwrap(),
            Report {
                total: 3,
                completed: 3,
                succeeded: 3,
                failed: 0,
                cancelled: false,
                stopped_on_error: false
            }
        );
    }
    #[test]
    fn failure_policies_and_between_item_cancellation_never_start_unwanted_calls() {
        for (mode, expected) in [
            (ExecutionMode::PerItemStop, 1),
            (ExecutionMode::PerItemContinue, 3),
        ] {
            let progress = Progress::new(3);
            let mut called = 0;
            let result = run("tool {paths}".into(), mode, &context(), &progress, |line| {
                called += 1;
                output(line, if called == 1 { 7 } else { 0 })
            });
            assert_eq!(called, expected);
            assert_eq!(result.exit_code, Some(7));
            assert!(result.stderr.contains("Invocation failed: exit 7"));
            assert_eq!(result.batch.unwrap().failed, 1);
        }
        let progress = Progress::new(3);
        let mut called = 0;
        let result = run(
            "tool".into(),
            ExecutionMode::PerItemContinue,
            &context(),
            &progress,
            |line| {
                called += 1;
                progress.cancelled.store(true, Ordering::Release);
                output(line, 0)
            },
        );
        assert_eq!(called, 1);
        assert!(result.batch.unwrap().cancelled);
        let result = run(
            "tool".into(),
            ExecutionMode::PerItemContinue,
            &context(),
            &progress,
            |_| panic!("already cancelled"),
        );
        assert!(result.batch.unwrap().cancelled);
    }
    #[test]
    fn whole_selection_stays_one_call_and_aggregate_output_is_bounded() {
        let progress = Progress::new(1);
        let mut called = 0;
        let result = run(
            "tool {paths}".into(),
            ExecutionMode::WholeSelection,
            &context(),
            &progress,
            |line| {
                called += 1;
                assert!(line.contains("three.txt"));
                output(line, 0)
            },
        );
        assert_eq!(called, 1);
        assert!(result.batch.is_none());
        let progress = Progress::new(3);
        let result = run(
            "tool".into(),
            ExecutionMode::PerItemContinue,
            &context(),
            &progress,
            |line| {
                let mut output = output(line, 0);
                output.stdout = "x".repeat(OUTPUT_CAP);
                output
            },
        );
        assert!(result.stdout.len() < OUTPUT_CAP + 1024);
        assert!(result.stdout.contains("aggregate output truncated"));
    }
}
