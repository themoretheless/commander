//! Desktop adapters for Linux and Windows. macOS uses the AppKit adapters in
//! `native_effect` and `native_menu`; these cover the same ports with
//! cross-platform primitives. The context menu is drawn by egui instead.

use crate::ports::{
    ClipboardOutcome, ClipboardPort, ContextMenuInvocation, ContextMenuPort, ContextMenuResult,
    NativeFailure, NativeFailureKind, OpenOutcome, OpenRequest, OpenerPort,
};

#[derive(Debug, Default)]
pub struct PortableClipboard;

impl ClipboardPort for PortableClipboard {
    fn write_text(&self, text: &str) -> ClipboardOutcome {
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
            Ok(()) => ClipboardOutcome::Committed,
            Err(error) => ClipboardOutcome::Failed(NativeFailure {
                kind: NativeFailureKind::Unknown,
                message: format!("Clipboard unavailable: {error}"),
            }),
        }
    }
}

#[derive(Debug, Default)]
pub struct PortableOpener;

impl OpenerPort for PortableOpener {
    fn open(&self, request: &OpenRequest) -> OpenOutcome {
        let result = match request {
            OpenRequest::OpenPath(path) | OpenRequest::QuickLook(path) => open::that(path),
            OpenRequest::Reveal(path) => reveal(path),
            OpenRequest::OpenWith { path, application } => std::process::Command::new(application)
                .arg(path)
                .spawn()
                .map(|_| ()),
            OpenRequest::GetInfo(_) => {
                return OpenOutcome::Failed(NativeFailure {
                    kind: NativeFailureKind::Unsupported,
                    message: "Get Info is only available on macOS".to_string(),
                });
            }
        };
        match result {
            Ok(()) => OpenOutcome::Accepted,
            Err(error) => OpenOutcome::Failed(NativeFailure::from_io(&error)),
        }
    }
}

#[cfg(windows)]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    let mut arg = std::ffi::OsString::from("/select,");
    arg.push(path.as_os_str());
    std::process::Command::new("explorer")
        .arg(arg)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    open::that(path.parent().unwrap_or(path))
}

/// The egui shell renders the fallback menu; the port only declines.
#[derive(Debug, Default)]
pub struct PortableContextMenu;

impl ContextMenuPort for PortableContextMenu {
    fn show_context_menu(&self, _invocation: &ContextMenuInvocation) -> ContextMenuResult {
        ContextMenuResult::Unsupported {
            reason: "native context menus are macOS-only".to_string(),
        }
    }
}
