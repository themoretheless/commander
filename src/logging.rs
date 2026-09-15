//! Structured logging facade: `log` macros with a dual stderr + file sink.
//!
//! Background failures that previously vanished (or only hit `eprintln!`) should
//! prefer `log::warn!` / `log::error!` so a durable trail lands under the app
//! config directory while still echoing to stderr during development.

use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

struct DualLogger {
    file: std::sync::Mutex<Option<File>>,
}

impl DualLogger {
    fn open_log_file() -> Option<File> {
        let path = default_log_path();
        let dir = path.parent()?;
        std::fs::create_dir_all(dir).ok()?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    }
}

impl Log for DualLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= LevelFilter::Info
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:5} {}: {}\n",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
            record.level(),
            record.target(),
            record.args()
        );
        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Ok(mut guard) = self.file.lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
        if let Ok(mut guard) = self.file.lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.flush();
        }
    }
}

fn default_log_path() -> PathBuf {
    crate::fs_util::config_dir().join("logs").join("commander.log")
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static INIT_ERROR: OnceLock<String> = OnceLock::new();

/// Install the dual stderr/file logger once. Safe to call from `main` and tests.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    let logger = DualLogger {
        file: std::sync::Mutex::new(DualLogger::open_log_file()),
    };
    match install(logger) {
        Ok(()) => log::info!(target: "commander::logging", "logging initialized"),
        Err(error) => {
            let _ = INIT_ERROR.set(error.to_string());
            eprintln!("commander: failed to install logger: {error}");
        }
    }
}

fn install(logger: DualLogger) -> Result<(), SetLoggerError> {
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(LevelFilter::Info);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_log_path_ends_with_logs_commander_log() {
        let path = default_log_path();
        assert!(path.ends_with("logs/commander.log"));
    }
}
