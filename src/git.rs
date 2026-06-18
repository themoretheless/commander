//! Git status extracted for SRP (separate concern from panel listing).
//! Small piece. Sync + debounce for immediate glyphs (blocks reload slightly for shell git).
//! Full off-main + result apply noted for perf follow-up. Errors reported via crate::error.
//! Can become plugin column.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::CommanderError;
use crate::panel::fs_pool;

/// Pure compute: run git porcelain, return map. Used both sync (immediate on reload) and bg.
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
            // non-success: report
            let msg = format!("git status failed: {}", String::from_utf8_lossy(&out.stderr));
            crate::error::report_error(CommanderError::Git(msg));
        }
        Err(e) => {
            crate::error::report_error(CommanderError::Git(format!("spawn git: {}", e)));
        }
    }
    out_map
}

pub fn refresh_git_status(
    path: &Path,
    git_status: &mut HashMap<PathBuf, char>,
    last_git_refresh: &mut Option<Instant>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
    git_tx: Option<Sender<(PathBuf, HashMap<PathBuf, char>)>>,
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
        git_status.clear();
        return;
    }
    // Do sync for immediate display (small cost for most repos)
    let map = compute_git_status(path);
    git_status.clear();
    git_status.extend(map.clone());
    *last_git_refresh = Some(Instant::now());

    // Also push via channel (for consistency + multi-tab wiring). Avoid double shell by sending what we have.
    if let Some(tx) = git_tx {
        let _ = tx.send((path.to_path_buf(), map.clone()));
    }
    if let Some(n) = notify {
        n();
    }
}
