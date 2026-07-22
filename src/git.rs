//! Git status extracted for SRP (separate concern from panel listing).
//! Always background: schedule via thread, apply only via channel message.
//! No sync shell on UI thread. Debounce on last refresh. Matches desired tokio+channels direction (using std thread + mpsc for compatibility with egui loop).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::CommanderError;

/// Pure compute (blocking, run only in bg).
pub fn compute_git_status(dir: &Path) -> HashMap<PathBuf, char> {
    let mut out_map = HashMap::new();
    let git_marker = dir.join(".git");
    if !git_marker.exists() {
        return out_map;
    }
    match std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("status")
        .arg("--porcelain")
        .arg("--untracked-files=all")
        .output()
    {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if line.len() >= 3 {
                    let status = line.chars().find(|c| !c.is_whitespace()).unwrap_or(' ');
                    let name_part = &line[3..];
                    let path_str = if let Some(arrow) = name_part.find(" -> ") {
                        &name_part[arrow + 4..]
                    } else {
                        name_part
                    };
                    let p = dir.join(path_str);
                    if status != ' ' {
                        out_map.insert(p, status);
                    }
                }
            }
        }
        Ok(out) => {
            let msg = format!("git status failed: {}", String::from_utf8_lossy(&out.stderr));
            crate::error::report_error(CommanderError::Git(msg));
        }
        Err(e) => {
            crate::error::report_error(CommanderError::Git(format!("spawn git: {}", e)));
        }
    }
    out_map
}

/// Schedule git status computation in background. Never blocks UI.
/// Result delivered via git_tx (apply in main loop by message).
/// last_git_refresh used only for debounce of scheduling.
pub fn refresh_git_status(
    path: &Path,
    last_git_refresh: &mut Option<Instant>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    git_tx: Option<UnboundedSender<(PathBuf, HashMap<PathBuf, char>)>>,
    tokio_handle: Option<tokio::runtime::Handle>,
) {
    const GIT_DEBOUNCE: Duration = Duration::from_millis(1500);
    if let Some(last) = *last_git_refresh {
        if last.elapsed() < GIT_DEBOUNCE {
            return;
        }
    }
    let git_marker = path.join(".git");
    if !git_marker.exists() {
        *last_git_refresh = Some(Instant::now());
        // Send empty to clear on receiver side if needed
        if let Some(tx) = git_tx {
            let _ = tx.send((path.to_path_buf(), HashMap::new()));
        }
        if let Some(n) = notify.clone() {
            n();
        }
        return;
    }
    *last_git_refresh = Some(Instant::now());

    let path2 = path.to_path_buf();
    let tx2 = git_tx.clone();
    let notify2 = notify.clone();
    let handle = tokio_handle.clone();
    let spawn = move |f: Box<dyn FnOnce() + Send>| {
        if let Some(h) = handle {
            h.spawn_blocking(f);
        } else {
            std::thread::spawn(f);
        }
    };
    spawn(Box::new(move || {
        let map = compute_git_status(&path2);
        if let Some(tx) = tx2 {
            let _ = tx.send((path2.clone(), map));
        }
        if let Some(n) = notify2 {
            n();
        }
    }));
}
