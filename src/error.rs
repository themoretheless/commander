//! Centralized error types for the app.
//! New idea: use thiserror or simple enum for errors, replace unwraps/panics with proper handling and toasts.
//! Inspired by robust error handling in VSCode or Path Finder.
//! report_error now feeds a tiny shared queue; UI drains to toasts (see app update).

use std::fmt;
use std::sync::{Mutex, OnceLock};

#[derive(Debug)]
pub enum CommanderError {
    Io(std::io::Error),
    Git(String),
    Other(String),
}

impl fmt::Display for CommanderError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CommanderError::Io(e) => write!(f, "IO error: {}", e),
            CommanderError::Git(s) => write!(f, "Git error: {}", s),
            CommanderError::Other(s) => write!(f, "Error: {}", s),
        }
    }
}

impl std::error::Error for CommanderError {}

fn error_queue() -> &'static Mutex<Vec<String>> {
    static Q: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Vec::new()))
}

/// Report an error. Stores for UI toast drain + logs.
pub fn report_error(e: CommanderError) {
    let msg = format!("{}", e);
    if let Ok(mut q) = error_queue().lock() {
        q.push(msg.clone());
        if q.len() > 5 { q.remove(0); }
    }
    eprintln!("Commander error: {}", msg);
}

/// Drain pending error messages (called from UI layer to turn into toasts).
pub fn drain_errors() -> Vec<String> {
    if let Ok(mut q) = error_queue().lock() {
        let v = std::mem::take(&mut *q);
        v
    } else { vec![] }
}
