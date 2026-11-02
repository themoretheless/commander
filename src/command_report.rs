//! Read-only export of retained output and its immutable invocation context.
use crate::{
    app::CommandOutput,
    cmdtemplate::{ExecutionMode, SelectionCtx, WorkingDirectory},
};
use serde::ser::{Serialize, SerializeSeq, Serializer};
use std::{io::Write, path::PathBuf};
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug, serde::Serialize)]
pub struct Context {
    pub name: String,
    pub template: String,
    pub execution: ExecutionMode,
    pub attempted_invocations: usize,
    pub working_directory_policy: WorkingDirectory,
    pub initial_working_directory: PathBuf,
    pub opening_directory: PathBuf,
    pub other_directory: PathBuf,
    pub inputs: Vec<PathBuf>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Stdout,
    Stderr,
    Text,
    Json,
}
impl Format {
    pub const ALL: [Self; 4] = [Self::Stdout, Self::Stderr, Self::Text, Self::Json];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stdout => "Copy stdout",
            Self::Stderr => "Copy stderr",
            Self::Text => "Copy report",
            Self::Json => "Copy report as JSON",
        }
    }
    #[cfg(any(test, feature = "visual-qa"))]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Text => "text",
            Self::Json => "json",
        }
    }
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("Command report exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn commands(output: &CommandOutput) -> impl Iterator<Item = String> + '_ {
    let completed = output.batch.as_ref().map_or(1, |report| report.completed);
    let count = output.context.as_ref().map_or(1, |context| {
        if context.execution.per_item() {
            completed
                .min(context.inputs.len())
                .min(context.attempted_invocations)
        } else {
            context.attempted_invocations.min(1)
        }
    });
    (0..count).map(move |index| {
        if let Some(context) = &output.context
            && context.execution.per_item()
        {
            crate::cmdtemplate::expand(
                &context.template,
                &SelectionCtx {
                    paths: vec![context.inputs[index].clone()],
                    dir: context.opening_directory.clone(),
                    dir_other: context.other_directory.clone(),
                },
            )
        } else {
            output.cmdline.clone()
        }
    })
}
struct Commands<'a>(&'a CommandOutput);
impl Serialize for Commands<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        for command in commands(self.0) {
            seq.serialize_element(&command)?;
        }
        seq.end()
    }
}
#[derive(serde::Serialize)]
struct Report<'a> {
    format: &'static str,
    version: u32,
    context: Option<&'a Context>,
    command: &'a str,
    attempted_commands: Commands<'a>,
    exit_code: Option<i32>,
    error: Option<&'a str>,
    batch: Option<&'a crate::command_batch::Report>,
    stdout: &'a str,
    stderr: &'a str,
}
pub fn export(output: &CommandOutput, format: Format) -> Result<String, String> {
    if matches!(format, Format::Stdout | Format::Stderr) {
        let text = if format == Format::Stdout {
            &output.stdout
        } else {
            &output.stderr
        };
        return if text.len() <= MAX_BYTES {
            Ok(text.clone())
        } else {
            Err("Captured stream exceeds 4 MiB".into())
        };
    }
    if let Some(context) = &output.context
        && [
            &context.initial_working_directory,
            &context.opening_directory,
            &context.other_directory,
        ]
        .into_iter()
        .chain(&context.inputs)
        .any(|path| path.to_str().is_none())
    {
        return Err(
            "Report context contains a non-UTF-8 path; copy stdout/stderr separately".into(),
        );
    }
    let mut writer = Bounded(Vec::new());
    if format == Format::Json {
        serde_json::to_writer_pretty(
            &mut writer,
            &Report {
                format: "commander.command-report",
                version: 1,
                context: output.context.as_ref(),
                command: &output.cmdline,
                attempted_commands: Commands(output),
                exit_code: output.exit_code,
                error: output.error.as_deref(),
                batch: output.batch.as_ref(),
                stdout: &output.stdout,
                stderr: &output.stderr,
            },
        )
        .map_err(|e| e.to_string())?;
    } else {
        (||->std::io::Result<()> {
            writeln!(writer,"Commander command report (v1)\nCommand: {}\nExit code: {:?}\nError: {}",output.cmdline,output.exit_code,output.error.as_deref().unwrap_or("none"))?;
            if let Some(context)=&output.context {
                writeln!(writer,"Name: {}\nTemplate: {}\nExecution: {}\nWorking folder policy: {}\nInitial working folder: {}\nOpening folder: {}\nOther folder: {}\nInputs: {}",context.name,context.template,context.execution.label(),context.working_directory_policy.label(),context.initial_working_directory.display(),context.opening_directory.display(),context.other_directory.display(),context.inputs.len())?;
                for path in &context.inputs {writeln!(writer,"- {}",path.display())?;}
            } else {writeln!(writer,"Invocation context unavailable")?;}
            if let Some(batch)=&output.batch {writeln!(writer,"Batch: {} / {} completed; {} succeeded; {} failed; {} remaining; cancelled={}; stopped_on_error={}",batch.completed,batch.total,batch.succeeded,batch.failed,batch.total.saturating_sub(batch.completed),batch.cancelled,batch.stopped_on_error)?;}
            writeln!(writer,"Attempted commands:")?;
            for command in commands(output) {writeln!(writer,"{command}")?;}
            write!(writer,"\n--- stdout ---\n{}\n--- stderr ---\n{}",output.stdout,output.stderr)
        })().map_err(|e|e.to_string())?;
    }
    String::from_utf8(writer.0).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn output() -> CommandOutput {
        CommandOutput {
            cmdline: "Per item: printf '%s' {paths}".into(),
            stdout: " first\n\0last ".into(),
            stderr: "warning\n".into(),
            exit_code: Some(7),
            error: None,
            batch: Some(crate::command_batch::Report {
                total: 2,
                completed: 1,
                succeeded: 0,
                failed: 1,
                cancelled: false,
                stopped_on_error: true,
            }),
            context: Some(Context {
                name: "Original".into(),
                template: "printf '%s' {paths}".into(),
                execution: ExecutionMode::PerItemStop,
                attempted_invocations: 1,
                working_directory_policy: WorkingDirectory::OtherPane,
                initial_working_directory: "/other".into(),
                opening_directory: "/opening".into(),
                other_directory: "/other".into(),
                inputs: vec![
                    "/opening/one file.txt".into(),
                    "/opening/not-started.txt".into(),
                ],
            }),
        }
    }
    #[test]
    fn reports_retain_context_exact_streams_and_only_attempted_per_item_commands() {
        let output = output();
        let json = export(&output, Format::Json).unwrap();
        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(report["context"]["initial_working_directory"], "/other");
        assert_eq!(report["context"]["template"], "printf '%s' {paths}");
        assert_eq!(
            report["attempted_commands"],
            serde_json::json!(["printf '%s' '/opening/one file.txt'"])
        );
        assert_eq!(report["stdout"], output.stdout);
        assert_eq!(report["stderr"], output.stderr);
        assert_eq!(report["batch"]["total"], 2);
        assert_eq!(export(&output, Format::Stdout).unwrap(), output.stdout);
        assert_eq!(export(&output, Format::Stderr).unwrap(), output.stderr);
        let text = export(&output, Format::Text).unwrap();
        assert!(text.contains("Initial working folder: /other"));
        assert!(text.contains("1 remaining"));
        assert!(text.contains(&output.stdout));
    }
    #[test]
    fn reports_refuse_oversize_payloads_without_returning_a_prefix_and_handle_missing_context() {
        let mut output = output();
        output.stdout = "\n".repeat(MAX_BYTES / 2 + 1);
        assert!(export(&output, Format::Json).is_err());
        output.stdout = "x".repeat(MAX_BYTES);
        assert!(export(&output, Format::Text).is_err());
        assert_eq!(export(&output, Format::Stdout).unwrap().len(), MAX_BYTES);
        output.stdout.clear();
        output.context = None;
        output.batch = None;
        output.error = Some("worker disconnected".into());
        assert!(
            export(&output, Format::Json)
                .unwrap()
                .contains("worker disconnected")
        );
    }
    #[cfg(unix)]
    #[test]
    fn metadata_exports_refuse_lossy_paths_but_captured_stream_copy_stays_available() {
        use std::os::unix::ffi::OsStringExt;
        let mut output = output();
        output.context.as_mut().unwrap().inputs[0] =
            std::ffi::OsString::from_vec(vec![b'/', 0xff]).into();
        assert!(export(&output, Format::Json).is_err());
        assert!(export(&output, Format::Text).is_err());
        assert_eq!(export(&output, Format::Stdout).unwrap(), output.stdout);
    }

    #[test]
    fn cancelled_before_invocation_does_not_invent_an_attempt() {
        let mut output = output();
        for execution in [ExecutionMode::WholeSelection, ExecutionMode::PerItemStop] {
            let context = output.context.as_mut().unwrap();
            context.execution = execution;
            context.attempted_invocations = 0;
            output.batch = execution
                .per_item()
                .then_some(crate::command_batch::Report {
                    total: 2,
                    completed: 0,
                    succeeded: 0,
                    failed: 0,
                    cancelled: true,
                    stopped_on_error: false,
                });
            let report: serde_json::Value =
                serde_json::from_str(&export(&output, Format::Json).unwrap()).unwrap();
            assert_eq!(report["attempted_commands"], serde_json::json!([]));
        }
        output.stderr = "x".repeat(MAX_BYTES + 1);
        assert!(export(&output, Format::Stderr).is_err());
    }
}
