use super::*;
use std::io::{Read as IoRead, Write as IoWrite};
use std::sync::{Arc, Mutex};

const COPY_BUF_SIZE: usize = 1024 * 1024; // 1 MB buffer

/// A flat entry for the confirmation dialog, with depth for indentation.
#[derive(Clone)]
pub(crate) struct FlatFileEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub depth: usize,
    pub rel_path: String, // relative path from root entry
}

const MAX_FLAT_DEPTH: usize = 5;
const MAX_FLAT_ENTRIES: usize = 5000;

impl App {
    /// Recursively collect all files/dirs into a flat list with depth.
    /// Limited to MAX_FLAT_DEPTH levels and MAX_FLAT_ENTRIES total.
    pub(crate) fn flatten_entries(entries: &[crate::panel::FileEntry]) -> Vec<FlatFileEntry> {
        let mut result = Vec::new();
        let mut truncated = false;
        for entry in entries {
            Self::flatten_entry(&entry.path, &entry.name, entry.is_dir, entry.size, 0, &mut result, &mut truncated);
            if truncated { break; }
        }
        if truncated {
            result.push(FlatFileEntry {
                name: format!("... (truncated at {} entries)", MAX_FLAT_ENTRIES),
                size: 0,
                is_dir: false,
                depth: 0,
                rel_path: String::new(),
            });
        }
        result
    }

    fn flatten_entry(
        path: &std::path::Path,
        name: &str,
        is_dir: bool,
        size: u64,
        depth: usize,
        result: &mut Vec<FlatFileEntry>,
        truncated: &mut bool,
    ) {
        if result.len() >= MAX_FLAT_ENTRIES {
            *truncated = true;
            return;
        }

        result.push(FlatFileEntry {
            name: name.to_string(),
            size,
            is_dir,
            depth,
            rel_path: name.to_string(),
        });

        if is_dir && depth < MAX_FLAT_DEPTH {
            if let Ok(rd) = std::fs::read_dir(path) {
                let mut children: Vec<_> = rd.filter_map(|e| e.ok()).collect();
                children.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
                for child in children {
                    if *truncated { return; }
                    let cp = child.path();
                    let cn = child.file_name().to_string_lossy().to_string();
                    let child_is_dir = cp.is_dir();
                    let child_size = if child_is_dir { 0 } else { cp.metadata().map(|m| m.len()).unwrap_or(0) };
                    Self::flatten_entry(&cp, &cn, child_is_dir, child_size, depth + 1, result, truncated);
                }
            }
        } else if is_dir && depth >= MAX_FLAT_DEPTH {
            // Show placeholder for deep dirs
            result.push(FlatFileEntry {
                name: "...".to_string(),
                size: 0,
                is_dir: false,
                depth: depth + 1,
                rel_path: String::new(),
            });
        }
    }

    fn find_conflicts(entries: &[crate::panel::FileEntry], target: &std::path::Path) -> Vec<String> {
        entries
            .iter()
            .filter(|e| target.join(&e.name).exists())
            .map(|e| e.name.clone())
            .collect()
    }

    fn spawn_scan(
        entries: Vec<crate::panel::FileEntry>,
        target: Option<PathBuf>,
    ) -> (FlatList, std::sync::Arc<std::sync::Mutex<Option<Vec<String>>>>) {
        let flat: FlatList = std::sync::Arc::new(std::sync::Mutex::new(None));
        let conflicts_arc: std::sync::Arc<std::sync::Mutex<Option<Vec<String>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(if target.is_some() { None } else { Some(vec![]) }));

        let flat_clone = flat.clone();
        let conflicts_clone = conflicts_arc.clone();
        let entries_clone = entries.clone();

        std::thread::spawn(move || {
            // Flatten first
            let result = Self::flatten_entries(&entries_clone);
            *flat_clone.lock().unwrap() = Some(result);

            // Then find conflicts
            if let Some(ref target) = target {
                let c = Self::find_conflicts(&entries_clone, target);
                *conflicts_clone.lock().unwrap() = Some(c);
            }
        });

        (flat, conflicts_arc)
    }

    pub(crate) fn request_copy(&mut self) {
        let target = self.inactive_panel().current_path.clone();
        let entries = self.active_panel().selected_or_cursor();
        if !entries.is_empty() {
            let (flat, _conflicts_bg) = Self::spawn_scan(entries.clone(), Some(target.clone()));
            // Conflicts checked quickly inline for now (top-level only)
            let conflicts = Self::find_conflicts(&entries, &target);
            self.pending_op = Some(PendingOp::Copy { entries, target, conflicts, policy: OverwritePolicy::Ask, method: CopyMethod::Native, flat });
        }
    }

    pub(crate) fn request_move(&mut self) {
        let target = self.inactive_panel().current_path.clone();
        let entries = self.active_panel().selected_or_cursor();
        if !entries.is_empty() {
            let (flat, _conflicts_bg) = Self::spawn_scan(entries.clone(), Some(target.clone()));
            let conflicts = Self::find_conflicts(&entries, &target);
            self.pending_op = Some(PendingOp::Move { entries, target, conflicts, policy: OverwritePolicy::Ask, method: CopyMethod::Native, flat });
        }
    }

    pub(crate) fn request_delete(&mut self) {
        let entries = self.active_panel().selected_or_cursor();
        if !entries.is_empty() {
            let (flat, _) = Self::spawn_scan(entries.clone(), None);
            self.pending_op = Some(PendingOp::Delete { entries, flat });
        }
    }

    /// Calculate total bytes for all entries (recursively for dirs).
    fn total_bytes(entries: &[crate::panel::FileEntry]) -> u64 {
        let mut total = 0u64;
        for e in entries {
            if e.is_dir {
                total += Self::dir_size_recursive(&e.path);
            } else {
                total += e.size;
            }
        }
        total
    }

    fn dir_size_recursive(path: &std::path::Path) -> u64 {
        let mut size = 0u64;
        if let Ok(rd) = std::fs::read_dir(path) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    size += Self::dir_size_recursive(&p);
                } else if let Ok(m) = p.metadata() {
                    size += m.len();
                }
            }
        }
        size
    }

    /// Copy a single file with progress reporting.
    fn copy_file_with_progress(
        src: &std::path::Path,
        dst: &std::path::Path,
        state: &TransferState,
    ) -> std::io::Result<()> {
        let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);

        // Init per-file progress
        {
            let mut s = state.lock().unwrap();
            s.current_file = src.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            s.current_file_size = file_size;
            s.current_file_copied = 0;
        }

        let mut reader = std::io::BufReader::with_capacity(
            COPY_BUF_SIZE,
            std::fs::File::open(src)?,
        );
        let mut writer = std::io::BufWriter::with_capacity(
            COPY_BUF_SIZE,
            std::fs::File::create(dst)?,
        );

        if let Ok(meta) = src.metadata() {
            let _ = std::fs::set_permissions(dst, meta.permissions());
        }

        let mut buf = vec![0u8; COPY_BUF_SIZE];
        let mut last_sample = std::time::Instant::now();

        loop {
            {
                let s = state.lock().unwrap();
                if s.cancelled {
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
                }
            }

            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;

            {
                let mut s = state.lock().unwrap();
                s.copied_bytes += n as u64;
                s.current_file_copied += n as u64;
                if last_sample.elapsed().as_millis() >= 500 {
                    s.record_sample();
                    last_sample = std::time::Instant::now();
                }
            }
        }
        writer.flush()?;
        Ok(())
    }

    /// Recursively copy a directory with progress.
    fn copy_dir_with_progress(
        src: &std::path::Path,
        dst: &std::path::Path,
        state: &TransferState,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                Self::copy_dir_with_progress(&src_path, &dst_path, state)?;
            } else {
                {
                    let mut s = state.lock().unwrap();
                    s.current_file = entry.file_name().to_string_lossy().to_string();
                }
                Self::copy_file_with_progress(&src_path, &dst_path, state)?;
            }
        }
        Ok(())
    }

    /// Start background copy/move with progress tracking.
    pub(crate) fn start_transfer(&mut self, ctx: &egui::Context) {
        if let Some(op) = self.pending_op.take() {
            let (entries, target, conflicts, policy, is_move, method) = match op {
                PendingOp::Copy { entries, target, conflicts, policy, method, .. } => {
                    (entries, target, conflicts, policy, false, method)
                }
                PendingOp::Move { entries, target, conflicts, policy, method, .. } => {
                    (entries, target, conflicts, policy, true, method)
                }
                PendingOp::Delete { entries, .. } => {
                    Self::exec_delete(&entries);
                    self.left.refresh();
                    self.right.refresh();
                    return;
                }
            };

            let total = Self::total_bytes(&entries);
            let file_count = entries.len();
            let progress = Arc::new(Mutex::new(TransferProgress::new(total, file_count)));
            self.active_transfer = Some(progress.clone());

            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let mut base_bytes: u64 = 0;

                for (i, entry) in entries.iter().enumerate() {
                    let dest = target.join(&entry.name);
                    let exists = conflicts.contains(&entry.name);

                    if exists && policy == OverwritePolicy::SkipAll {
                        let mut s = progress.lock().unwrap();
                        s.files_done = i + 1;
                        continue;
                    }

                    {
                        let mut s = progress.lock().unwrap();
                        s.current_file = entry.name.clone();
                        if s.cancelled {
                            return;
                        }
                    }

                    if exists && dest.is_dir() {
                        let _ = std::fs::remove_dir_all(&dest);
                    }

                    let result = match method {
                        CopyMethod::Native => {
                            if entry.is_dir {
                                crate::native_copy::copy_dir_native(
                                    &entry.path, &dest, &progress, base_bytes,
                                ).map(|b| { base_bytes += b; })
                            } else {
                                crate::native_copy::copy_file_native(
                                    &entry.path, &dest, &progress, base_bytes,
                                ).map(|b| { base_bytes += b; })
                            }
                        }
                        CopyMethod::Buffered => {
                            if entry.is_dir {
                                Self::copy_dir_with_progress(&entry.path, &dest, &progress)
                            } else {
                                Self::copy_file_with_progress(&entry.path, &dest, &progress)
                            }
                        }
                    };

                    if result.is_err() {
                        let s = progress.lock().unwrap();
                        if s.cancelled {
                            return;
                        }
                    }

                    if is_move && result.is_ok() {
                        if entry.is_dir {
                            let _ = std::fs::remove_dir_all(&entry.path);
                        } else {
                            let _ = std::fs::remove_file(&entry.path);
                        }
                    }

                    {
                        let mut s = progress.lock().unwrap();
                        s.files_done = i + 1;
                        s.record_sample();
                    }
                    ctx.request_repaint();
                }

                {
                    let mut s = progress.lock().unwrap();
                    s.finished = true;
                    s.record_sample();
                }
                ctx.request_repaint();
            });
        }
    }

    /// Cancel active transfer.
    pub(crate) fn cancel_transfer(&mut self) {
        if let Some(ref state) = self.active_transfer {
            let mut s = state.lock().unwrap();
            s.cancelled = true;
        }
    }

    /// Check if transfer finished and clean up.
    pub(crate) fn poll_transfer(&mut self) {
        let finished = self.active_transfer.as_ref().map(|s| {
            let s = s.lock().unwrap();
            s.finished || s.cancelled
        }).unwrap_or(false);

        if finished {
            self.active_transfer = None;
            self.left.refresh();
            self.right.refresh();
        }
    }

    pub(crate) fn exec_delete(entries: &[crate::panel::FileEntry]) {
        for entry in entries {
            let _ = trash::delete(&entry.path);
        }
    }

    pub(crate) fn confirm_pending_op(&mut self, ctx: &egui::Context) {
        match &self.pending_op {
            Some(PendingOp::Delete { .. }) => {
                if let Some(PendingOp::Delete { entries, .. }) = self.pending_op.take() {
                    Self::exec_delete(&entries);
                    self.left.refresh();
                    self.right.refresh();
                }
            }
            Some(PendingOp::Copy { .. }) | Some(PendingOp::Move { .. }) => {
                self.start_transfer(ctx);
            }
            None => {}
        }
    }

    pub(crate) fn set_pending_policy(&mut self, new_policy: OverwritePolicy) {
        match &mut self.pending_op {
            Some(PendingOp::Copy { policy, .. }) => *policy = new_policy,
            Some(PendingOp::Move { policy, .. }) => *policy = new_policy,
            _ => {}
        }
    }

    pub(crate) fn create_dir(&mut self) {
        let panel = self.active_panel();
        let new_dir = panel.current_path.join("New Folder");
        let mut path = new_dir.clone();
        let mut i = 1;
        while path.exists() {
            path = panel.current_path.join(format!("New Folder {}", i));
            i += 1;
        }
        let _ = std::fs::create_dir(&path);
        self.left.refresh();
        self.right.refresh();
    }

    pub(crate) fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }
}
