//! Test-only helpers (compiled with `cargo test` only).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Clear unapplied texture deltas so `egui::FullOutput` can drop safely.
///
/// epaint 0.36 panics if a `TexturesDelta` is dropped with pending deltas.
/// Headless tests never upload fonts/textures, so they must clear first.
pub fn discard_egui_output(mut output: egui::FullOutput) {
    output.textures_delta.clear();
}

/// Same as [`discard_egui_output`], but keep the cleared output for inspection.
pub fn take_egui_output(mut output: egui::FullOutput) -> egui::FullOutput {
    output.textures_delta.clear();
    output
}

/// A unique temporary directory removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("commander-test-{}-{}", std::process::id(), n));
        // Clear any stale leftover from a crashed run with a recycled pid,
        // then fail loudly if the directory still unexpectedly exists.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Create a file (with parent dirs) and return its path.
    pub fn file(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.0.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, contents).unwrap();
        p
    }

    /// Create a subdirectory and return its path.
    pub fn dir(&self, name: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            log::warn!(
                "failed to remove temp dir {}: {}",
                self.0.display(),
                e
            );
        }
    }
}
