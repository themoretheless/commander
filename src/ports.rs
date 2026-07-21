//! Narrow provider ports used at expensive or failure-prone boundaries.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_TEXT_PREVIEW_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuCommand {
    Duplicate,
    Compress,
    MoveToTrash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuFailure {
    MainThreadRequired,
    Action {
        command: ContextMenuCommand,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuResult {
    Dismissed,
    RefreshRequested,
    Unsupported { reason: String },
    Failed(ContextMenuFailure),
}

/// Main-thread desktop context-menu boundary. It intentionally has no
/// `Send`/`Sync` bounds: AppKit adapters are owned and invoked by the UI thread.
pub trait ContextMenuPort {
    fn show_context_menu(&self, path: &Path) -> ContextMenuResult;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewKind {
    Image,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRequest<'a> {
    pub path: &'a Path,
    pub kind: PreviewKind,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewArtifact {
    Image,
    Text(String),
}

pub trait PreviewProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn preview(&self, request: &PreviewRequest<'_>) -> Result<PreviewArtifact, String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativePreviewProvider;

impl PreviewProvider for NativePreviewProvider {
    fn id(&self) -> &'static str {
        "native-preview"
    }

    fn preview(&self, request: &PreviewRequest<'_>) -> Result<PreviewArtifact, String> {
        if request.kind == PreviewKind::Image {
            return Ok(PreviewArtifact::Image);
        }
        let mut file = std::fs::File::open(request.path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(request.max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > request.max_bytes {
            return Err(format!(
                "preview exceeds the {} byte limit",
                request.max_bytes
            ));
        }
        let text = String::from_utf8(bytes).map_err(|_| "preview is not UTF-8 text".to_string())?;
        Ok(PreviewArtifact::Text(text))
    }
}

/// The search engine owns its rich candidate model; providers only enumerate
/// candidates for one immutable request and never reach into UI state.
pub(crate) trait SearchProvider: Send {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn root(&self) -> &Path;
    fn visit(
        &mut self,
        request: &crate::search::ProviderRequest,
        emit: &mut dyn FnMut(crate::search::ProviderRecord) -> bool,
    );
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileSystemEffect {
    CreateDirectory {
        path: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    Rename {
        source: PathBuf,
        destination: PathBuf,
        replace: bool,
    },
    Remove {
        path: PathBuf,
    },
}

pub trait FileSystemProvider: Send + Sync {
    fn observe(&self, path: &Path) -> std::io::Result<crate::path_identity::PathIdentity>;
    fn apply(&self, effect: &FileSystemEffect) -> std::io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFileSystemProvider;

impl FileSystemProvider for NativeFileSystemProvider {
    fn observe(&self, path: &Path) -> std::io::Result<crate::path_identity::PathIdentity> {
        crate::path_identity::PathIdentity::observe_deep(path)
    }

    fn apply(&self, effect: &FileSystemEffect) -> std::io::Result<()> {
        match effect {
            FileSystemEffect::CreateDirectory { path } => std::fs::create_dir_all(path),
            FileSystemEffect::WriteFile { path, bytes } => {
                let parent = path.parent().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "file effect has no parent directory",
                    )
                })?;
                std::fs::create_dir_all(parent)?;
                let mut file = std::fs::File::create(path)?;
                file.write_all(bytes)
            }
            FileSystemEffect::Rename {
                source,
                destination,
                replace,
            } => {
                if *replace {
                    std::fs::rename(source, destination)
                } else {
                    crate::native_copy::rename_noreplace(source, destination)
                }
            }
            FileSystemEffect::Remove { path } => match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
                Ok(_) => std::fs::remove_file(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
    }
}

pub type ContentDigest = [u8; 32];

pub trait Hasher: Send + Sync {
    fn hash(&self, path: &Path) -> std::io::Result<ContentDigest>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VerifiedBlake3Hasher;

impl Hasher for VerifiedBlake3Hasher {
    fn hash(&self, path: &Path) -> std::io::Result<ContentDigest> {
        crate::verified_hash::file(path)
    }
}

pub fn default_hasher() -> &'static dyn Hasher {
    static HASHER: VerifiedBlake3Hasher = VerifiedBlake3Hasher;
    &HASHER
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn native_preview_reads_through_a_hard_byte_limit() {
        let temp = TempDir::new();
        let small = temp.file("small.txt", "hello");
        let large = temp.file("large.txt", "too long");
        let provider = NativePreviewProvider;
        assert_eq!(
            provider
                .preview(&PreviewRequest {
                    path: &small,
                    kind: PreviewKind::Text,
                    max_bytes: 5,
                })
                .unwrap(),
            PreviewArtifact::Text("hello".to_string())
        );
        assert!(
            provider
                .preview(&PreviewRequest {
                    path: &large,
                    kind: PreviewKind::Text,
                    max_bytes: 4,
                })
                .is_err()
        );
    }

    #[test]
    fn filesystem_port_applies_typed_effects_without_hidden_ui_state() {
        let temp = TempDir::new();
        let source = temp.path().join("source.txt");
        let destination = temp.path().join("nested/destination.txt");
        let provider = NativeFileSystemProvider;
        provider
            .apply(&FileSystemEffect::WriteFile {
                path: source.clone(),
                bytes: b"value".to_vec(),
            })
            .unwrap();
        provider
            .apply(&FileSystemEffect::CreateDirectory {
                path: destination.parent().unwrap().to_path_buf(),
            })
            .unwrap();
        provider
            .apply(&FileSystemEffect::Rename {
                source,
                destination: destination.clone(),
                replace: false,
            })
            .unwrap();
        assert!(provider.observe(&destination).unwrap().exists);
    }
}
