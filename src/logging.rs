//! Structured logging facade: `log` macros with a dual stderr + file sink.
//!
//! Background failures that previously vanished (or only hit `eprintln!`) should
//! prefer `log::warn!` / `log::error!` so a durable trail lands under the app
//! config directory while still echoing to stderr during development.

use log::{LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

struct DualLogger {
    file: std::sync::Mutex<Option<File>>,
}

impl DualLogger {
    /// Open (rotating first) the append-only log file at `path`; the error is
    /// kept so `init` can say why file logging is off.
    fn open_log_file(path: &Path) -> std::io::Result<File> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        rotate_if_large(path, MAX_LOG_BYTES);
        OpenOptions::new().create(true).append(true).open(path)
    }
}

/// Size at which the log is rotated on startup; one previous generation is kept.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// Moves an oversized log to `<name>.1` (replacing the older generation) so the
/// append-only log cannot grow without bound across sessions.
fn rotate_if_large(path: &Path, max_bytes: u64) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() <= max_bytes {
        return;
    }
    let mut rotated = path.as_os_str().to_owned();
    rotated.push(".1");
    let rotated = PathBuf::from(rotated);
    // Windows `rename` refuses to replace an existing file.
    let _ = std::fs::remove_file(&rotated);
    if std::fs::rename(path, &rotated).is_err() {
        // Could not keep the old log; start fresh rather than grow forever.
        let _ = OpenOptions::new().write(true).truncate(true).open(path);
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
    crate::fs_util::config_dir()
        .join("logs")
        .join("commander.log")
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static INIT_ERROR: OnceLock<String> = OnceLock::new();

/// Install the dual stderr/file logger once. Safe to call from `main` and tests.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    let path = default_log_path();
    let (file, file_error) = match DualLogger::open_log_file(&path) {
        Ok(file) => (Some(file), None),
        Err(error) => (None, Some(error)),
    };
    let logger = DualLogger {
        file: std::sync::Mutex::new(file),
    };
    match install(logger) {
        Ok(()) => {
            log::info!(target: "commander::logging", "logging initialized");
            if let Some(error) = file_error {
                log::warn!(
                    target: "commander::logging",
                    "{}",
                    file_logging_disabled_message(&path, &error)
                );
            }
        }
        Err(error) => {
            let _ = INIT_ERROR.set(error.to_string());
            eprintln!("commander: failed to install logger: {error}");
        }
    }
}

fn file_logging_disabled_message(path: &Path, error: &std::io::Error) -> String {
    format!(
        "file logging disabled: could not open {}: {error}; logging to stderr only (check that the folder is writable)",
        path.display()
    )
}

/// Best-effort text of a panic payload (`panic!` produces `&str` or `String`).
pub fn panic_payload_text(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

fn panic_log_line(thread: Option<&str>, location: Option<String>, message: &str) -> String {
    format!(
        "thread '{}' panicked at {}: {message}",
        thread.unwrap_or("<unnamed>"),
        location.as_deref().unwrap_or("<unknown location>")
    )
}

/// Record panics in the app log (stderr is lost in a bundled GUI app), then
/// run the previous hook so the default report and backtrace still happen.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()));
        let line = panic_log_line(
            std::thread::current().name(),
            location,
            panic_payload_text(info.payload()),
        );
        log::error!(target: "commander::panic", "{line}");
        log::logger().flush();
        previous(info);
    }));
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

    #[test]
    fn unopenable_log_file_reports_path_and_os_error() {
        let temp = crate::testutil::TempDir::new();
        let blocker = temp.path().join("logs");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let path = blocker.join("commander.log");
        let error = DualLogger::open_log_file(&path).unwrap_err();
        let message = file_logging_disabled_message(&path, &error);
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(message.contains(&error.to_string()), "{message}");
    }

    #[test]
    fn panic_payload_and_line_name_message_thread_and_location() {
        let literal: Box<dyn std::any::Any + Send> = Box::new("boom");
        let owned: Box<dyn std::any::Any + Send> = Box::new(String::from("owned boom"));
        let other: Box<dyn std::any::Any + Send> = Box::new(7_u8);
        assert_eq!(panic_payload_text(literal.as_ref()), "boom");
        assert_eq!(panic_payload_text(owned.as_ref()), "owned boom");
        assert_eq!(
            panic_payload_text(other.as_ref()),
            "non-string panic payload"
        );
        assert_eq!(
            panic_log_line(Some("work-list-3"), Some("src/a.rs:9".to_string()), "boom"),
            "thread 'work-list-3' panicked at src/a.rs:9: boom"
        );
    }

    #[test]
    fn oversized_log_rotates_to_previous_generation() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.path().join("commander.log");
        let previous = temp.path().join("commander.log.1");
        std::fs::write(&previous, b"ancient").unwrap();
        std::fs::write(&path, b"0123456789").unwrap();

        rotate_if_large(&path, 100);
        assert!(path.exists());

        rotate_if_large(&path, 4);
        assert!(!path.exists());
        assert_eq!(std::fs::read(&previous).unwrap(), b"0123456789");
    }
}
