//! Lazy provider activation and the process boundary for optional providers.

use crate::ports::{
    ContextMenuAction, ContextMenuCommand, ContextMenuFailure, ContextMenuPort, ContextMenuResult,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuNoticeLevel {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuUiEffect {
    RefreshPanels,
    Open(crate::ports::OpenRequest),
    CopyPath(PathBuf),
    MoveToTrash(PathBuf),
    Perform(ContextMenuAction),
    Notice {
        level: ContextMenuNoticeLevel,
        message: String,
    },
}

pub fn reduce_context_menu_result(
    result: ContextMenuResult,
    path: &Path,
) -> Option<ContextMenuUiEffect> {
    match result {
        ContextMenuResult::Dismissed => None,
        ContextMenuResult::RefreshRequested => Some(ContextMenuUiEffect::RefreshPanels),
        ContextMenuResult::OpenRequested => Some(ContextMenuUiEffect::Open(
            crate::ports::OpenRequest::OpenPath(path.to_path_buf()),
        )),
        ContextMenuResult::OpenWithRequested { application } => Some(ContextMenuUiEffect::Open(
            crate::ports::OpenRequest::OpenWith {
                path: path.to_path_buf(),
                application,
            },
        )),
        ContextMenuResult::QuickLookRequested => Some(ContextMenuUiEffect::Open(
            crate::ports::OpenRequest::QuickLook(path.to_path_buf()),
        )),
        ContextMenuResult::GetInfoRequested => Some(ContextMenuUiEffect::Open(
            crate::ports::OpenRequest::GetInfo(path.to_path_buf()),
        )),
        ContextMenuResult::RevealRequested => Some(ContextMenuUiEffect::Open(
            crate::ports::OpenRequest::Reveal(path.to_path_buf()),
        )),
        ContextMenuResult::CopyPathRequested => {
            Some(ContextMenuUiEffect::CopyPath(path.to_path_buf()))
        }
        ContextMenuResult::MoveToTrashRequested => {
            Some(ContextMenuUiEffect::MoveToTrash(path.to_path_buf()))
        }
        ContextMenuResult::DeferredActionRequested(action) => {
            Some(ContextMenuUiEffect::Perform(action))
        }
        ContextMenuResult::Unsupported { reason } => Some(ContextMenuUiEffect::Notice {
            level: ContextMenuNoticeLevel::Info,
            message: format!("Context menu unavailable: {reason}"),
        }),
        ContextMenuResult::Failed(ContextMenuFailure::MainThreadRequired) => {
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Error,
                message: "Context menu must run on the main thread".to_string(),
            })
        }
        ContextMenuResult::Failed(ContextMenuFailure::StaleInvocation) => {
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Error,
                message: "The context menu selection expired; open the menu again".to_string(),
            })
        }
        ContextMenuResult::Failed(ContextMenuFailure::InvalidSelection) => {
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Error,
                message: "The context menu returned an invalid selection".to_string(),
            })
        }
        ContextMenuResult::Failed(ContextMenuFailure::TargetUnavailable { message }) => {
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Error,
                message: format!("The context menu target is unavailable: {message}"),
            })
        }
        ContextMenuResult::Failed(ContextMenuFailure::Action { command, message }) => {
            let action = match command {
                ContextMenuCommand::OpenWith => "open item with the selected application",
                ContextMenuCommand::QuickLook => "preview item",
                ContextMenuCommand::GetInfo => "show item information",
                ContextMenuCommand::Duplicate => "duplicate item",
                ContextMenuCommand::Compress => "start compression",
                ContextMenuCommand::ToggleTag => "update Finder tags",
                ContextMenuCommand::Share => "share item",
                ContextMenuCommand::MoveToTrash => "move item to Trash",
            };
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Error,
                message: format!("Could not {action}: {message}"),
            })
        }
    }
}

pub fn reduce_deferred_context_menu_result(
    result: ContextMenuResult,
    path: &Path,
) -> Option<ContextMenuUiEffect> {
    match reduce_context_menu_result(result, path) {
        Some(ContextMenuUiEffect::Perform(action)) => Some(ContextMenuUiEffect::Notice {
            level: ContextMenuNoticeLevel::Error,
            message: format!(
                "Could not {:?}: the context-menu adapter returned a nested deferred action",
                action.command()
            ),
        }),
        terminal => terminal,
    }
}

pub fn request_context_menu(
    port: &dyn ContextMenuPort,
    invocation: &crate::ports::ContextMenuInvocation,
) -> Option<ContextMenuUiEffect> {
    reduce_context_menu_result(port.show_context_menu(invocation), &invocation.target.path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderCapability {
    PreviewImage,
    PreviewText,
    SearchLive,
    SearchIndex,
    IndexBuild,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationRequest<'a> {
    pub capability: ProviderCapability,
    pub root: &'a Path,
    pub extension: Option<&'a str>,
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationRule {
    pub capabilities: Vec<ProviderCapability>,
    pub root: Option<PathBuf>,
    pub extensions: Vec<String>,
    pub max_bytes: Option<u64>,
}

impl ActivationRule {
    fn matches(&self, request: &ActivationRequest<'_>) -> bool {
        if !self.capabilities.contains(&request.capability) {
            return false;
        }
        if self
            .root
            .as_ref()
            .is_some_and(|root| !request.root.starts_with(root))
        {
            return false;
        }
        if !self.extensions.is_empty() {
            let Some(extension) = request.extension else {
                return false;
            };
            if !self
                .extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
            {
                return false;
            }
        }
        !matches!((self.max_bytes, request.bytes), (Some(max), Some(bytes)) if bytes > max)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderIsolation {
    TrustedBuiltin,
    ExternalProcess {
        executable: PathBuf,
        args: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub id: String,
    pub optional: bool,
    pub isolation: ProviderIsolation,
    pub activation: ActivationRule,
    pub startup_cost_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivationBudget {
    pub max_activations: usize,
    pub max_total_startup_ms: u64,
}

impl Default for ActivationBudget {
    fn default() -> Self {
        Self {
            max_activations: 8,
            max_total_startup_ms: 250,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationError {
    UnknownProvider(String),
    CapabilityMismatch(String),
    StartupBudgetExceeded(String),
    OptionalProviderMustBeExternal(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivatedProvider {
    pub id: String,
    pub isolation: ProviderIsolation,
    pub newly_activated: bool,
}

pub struct ProviderRegistry {
    descriptors: Vec<ProviderDescriptor>,
    active: HashSet<String>,
    startup_spent_ms: u64,
    budget: ActivationBudget,
}

impl ProviderRegistry {
    pub fn new(budget: ActivationBudget) -> Self {
        Self {
            descriptors: Vec::new(),
            active: HashSet::new(),
            startup_spent_ms: 0,
            budget,
        }
    }

    pub fn register(&mut self, descriptor: ProviderDescriptor) -> Result<(), ActivationError> {
        if descriptor.optional && descriptor.isolation == ProviderIsolation::TrustedBuiltin {
            return Err(ActivationError::OptionalProviderMustBeExternal(
                descriptor.id,
            ));
        }
        if let Some(existing) = self
            .descriptors
            .iter_mut()
            .find(|candidate| candidate.id == descriptor.id)
        {
            *existing = descriptor;
        } else {
            self.descriptors.push(descriptor);
        }
        Ok(())
    }

    pub fn activate(
        &mut self,
        id: &str,
        request: &ActivationRequest<'_>,
    ) -> Result<ActivatedProvider, ActivationError> {
        let descriptor = self
            .descriptors
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or_else(|| ActivationError::UnknownProvider(id.to_string()))?;
        if !descriptor.activation.matches(request) {
            return Err(ActivationError::CapabilityMismatch(id.to_string()));
        }
        if self.active.contains(id) {
            return Ok(ActivatedProvider {
                id: id.to_string(),
                isolation: descriptor.isolation.clone(),
                newly_activated: false,
            });
        }
        let next_spend = self
            .startup_spent_ms
            .saturating_add(descriptor.startup_cost_ms);
        if self.active.len() >= self.budget.max_activations
            || next_spend > self.budget.max_total_startup_ms
        {
            return Err(ActivationError::StartupBudgetExceeded(id.to_string()));
        }
        self.startup_spent_ms = next_spend;
        self.active.insert(id.to_string());
        Ok(ActivatedProvider {
            id: id.to_string(),
            isolation: descriptor.isolation.clone(),
            newly_activated: true,
        })
    }

    pub fn startup_spent_ms(&self) -> u64 {
        self.startup_spent_ms
    }
}

fn builtin_descriptor(id: &str, capabilities: Vec<ProviderCapability>) -> ProviderDescriptor {
    ProviderDescriptor {
        id: id.to_string(),
        optional: false,
        isolation: ProviderIsolation::TrustedBuiltin,
        activation: ActivationRule {
            capabilities,
            root: None,
            extensions: Vec::new(),
            max_bytes: None,
        },
        startup_cost_ms: 10,
    }
}

fn builtin_registry() -> &'static Mutex<ProviderRegistry> {
    static REGISTRY: OnceLock<Mutex<ProviderRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = ProviderRegistry::new(ActivationBudget::default());
        for descriptor in [
            builtin_descriptor(
                "native-preview",
                vec![
                    ProviderCapability::PreviewImage,
                    ProviderCapability::PreviewText,
                ],
            ),
            builtin_descriptor("live-search", vec![ProviderCapability::SearchLive]),
            builtin_descriptor("archive-search", vec![ProviderCapability::SearchLive]),
            builtin_descriptor("indexed-search", vec![ProviderCapability::SearchIndex]),
            builtin_descriptor("content-index", vec![ProviderCapability::IndexBuild]),
        ] {
            registry
                .register(descriptor)
                .expect("built-in provider descriptors are valid");
        }
        Mutex::new(registry)
    })
}

pub fn activate_builtin(id: &str, request: &ActivationRequest<'_>) -> bool {
    let enabled = match request.capability {
        ProviderCapability::PreviewImage | ProviderCapability::PreviewText => {
            crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ImagePreview)
        }
        ProviderCapability::SearchIndex | ProviderCapability::IndexBuild => {
            crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ContentIndex)
        }
        ProviderCapability::SearchLive => true,
    };
    if !enabled {
        return false;
    }
    crate::lock_util::recover(builtin_registry())
        .activate(id, request)
        .is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRpcRequest {
    pub capability: ProviderCapability,
    pub root: PathBuf,
    pub path: Option<PathBuf>,
    pub expression: Option<String>,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRpcResponse {
    pub ok: bool,
    pub payload: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalProviderClient {
    executable: PathBuf,
    args: Vec<String>,
}

impl ExternalProviderClient {
    pub fn new(executable: PathBuf, args: Vec<String>) -> Self {
        Self { executable, args }
    }

    pub fn request(
        &self,
        request: &ProviderRpcRequest,
        timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<ProviderRpcResponse, String> {
        if !crate::feature_flags::enabled(crate::feature_flags::RiskyFeature::ExternalProviders) {
            return Err("external providers disabled by runtime control".to_string());
        }
        let mut child = Command::new(&self.executable)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start provider process: {error}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "provider process has no stdin".to_string())?;
        serde_json::to_writer(&mut stdin, request)
            .map_err(|error| format!("could not encode provider request: {error}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|error| format!("could not send provider request: {error}"))?;
        drop(stdin);

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "provider process has no stdout".to_string())?;
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .by_ref()
                .take(max_response_bytes.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });

        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err("provider process timed out".to_string());
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = reader.join();
                    return Err(format!("provider process wait failed: {error}"));
                }
            }
        };
        let bytes = reader
            .join()
            .map_err(|_| "provider response reader panicked".to_string())?
            .map_err(|error| format!("provider response read failed: {error}"))?;
        if !status.success() {
            return Err(format!("provider process exited with {status}"));
        }
        if bytes.len() > max_response_bytes {
            return Err("provider response exceeded its byte budget".to_string());
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("provider response is invalid: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeContextMenuPort {
        result: ContextMenuResult,
        calls: Cell<usize>,
        path: RefCell<Option<PathBuf>>,
    }

    struct NestedDeferredContextMenuPort {
        calls: Cell<usize>,
    }

    impl FakeContextMenuPort {
        fn returning(result: ContextMenuResult) -> Self {
            Self {
                result,
                calls: Cell::new(0),
                path: RefCell::new(None),
            }
        }
    }

    impl ContextMenuPort for FakeContextMenuPort {
        fn show_context_menu(
            &self,
            invocation: &crate::ports::ContextMenuInvocation,
        ) -> ContextMenuResult {
            self.calls.set(self.calls.get() + 1);
            self.path.replace(Some(invocation.target.path.clone()));
            self.result.clone()
        }
    }

    impl ContextMenuPort for NestedDeferredContextMenuPort {
        fn show_context_menu(
            &self,
            _invocation: &crate::ports::ContextMenuInvocation,
        ) -> ContextMenuResult {
            ContextMenuResult::Dismissed
        }

        fn perform_deferred_action(&self, action: &ContextMenuAction) -> ContextMenuResult {
            self.calls.set(self.calls.get() + 1);
            ContextMenuResult::DeferredActionRequested(action.clone())
        }
    }

    fn descriptor(id: &str, capability: ProviderCapability, cost: u64) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            optional: false,
            isolation: ProviderIsolation::TrustedBuiltin,
            activation: ActivationRule {
                capabilities: vec![capability],
                root: Some(PathBuf::from("/projects")),
                extensions: vec!["md".to_string()],
                max_bytes: Some(1_000),
            },
            startup_cost_ms: cost,
        }
    }

    fn menu_invocation(path: &Path) -> crate::ports::ContextMenuInvocation {
        crate::ports::ContextMenuInvocation {
            target: crate::ports::ContextMenuTarget {
                path: path.to_path_buf(),
                expected: crate::path_identity::PathIdentity::missing(path),
            },
            trigger: crate::ports::ContextMenuTrigger::Keyboard,
            anchor: crate::ports::ContextMenuAnchor::ViewRect(crate::ports::ContextMenuViewRect {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 100.0,
                max_y: 24.0,
                native_points_per_ui_point: 1.0,
            }),
        }
    }

    #[test]
    fn providers_activate_lazily_by_root_capability_and_file_contract() {
        let mut registry = ProviderRegistry::new(ActivationBudget {
            max_activations: 2,
            max_total_startup_ms: 30,
        });
        registry
            .register(descriptor(
                "markdown-preview",
                ProviderCapability::PreviewText,
                12,
            ))
            .unwrap();
        assert_eq!(registry.startup_spent_ms(), 0);
        let request = ActivationRequest {
            capability: ProviderCapability::PreviewText,
            root: Path::new("/projects/commander"),
            extension: Some("MD"),
            bytes: Some(900),
        };
        assert!(registry.activate("markdown-preview", &request).is_ok());
        assert_eq!(registry.startup_spent_ms(), 12);
        assert!(
            !registry
                .activate("markdown-preview", &request)
                .unwrap()
                .newly_activated
        );
        let outside = ActivationRequest {
            root: Path::new("/other"),
            ..request
        };
        assert!(matches!(
            registry.activate("markdown-preview", &outside),
            Err(ActivationError::CapabilityMismatch(_))
        ));
    }

    #[test]
    fn startup_budget_applies_before_a_provider_is_started() {
        let mut registry = ProviderRegistry::new(ActivationBudget {
            max_activations: 1,
            max_total_startup_ms: 10,
        });
        registry
            .register(descriptor("expensive", ProviderCapability::PreviewText, 11))
            .unwrap();
        let request = ActivationRequest {
            capability: ProviderCapability::PreviewText,
            root: Path::new("/projects"),
            extension: Some("md"),
            bytes: Some(1),
        };
        assert!(matches!(
            registry.activate("expensive", &request),
            Err(ActivationError::StartupBudgetExceeded(_))
        ));
        assert_eq!(registry.startup_spent_ms(), 0);
    }

    #[test]
    fn optional_providers_cannot_be_registered_in_the_ui_process() {
        let mut registry = ProviderRegistry::new(ActivationBudget::default());
        let mut optional = descriptor("third-party", ProviderCapability::PreviewText, 1);
        optional.optional = true;
        assert!(matches!(
            registry.register(optional),
            Err(ActivationError::OptionalProviderMustBeExternal(_))
        ));
    }

    #[test]
    fn context_menu_request_uses_the_injected_main_thread_port() {
        let port = FakeContextMenuPort::returning(ContextMenuResult::RefreshRequested);
        let path = Path::new("/tmp/example");
        let invocation = menu_invocation(path);
        assert_eq!(
            request_context_menu(&port, &invocation),
            Some(ContextMenuUiEffect::RefreshPanels)
        );
        assert_eq!(port.calls.get(), 1);
        assert_eq!(port.path.borrow().as_deref(), Some(path));
    }

    #[test]
    fn unsupported_context_menu_reduces_to_an_explicit_info_notice() {
        let port = FakeContextMenuPort::returning(ContextMenuResult::Unsupported {
            reason: "AppKit is unavailable".to_string(),
        });
        let invocation = menu_invocation(Path::new("/tmp/example"));
        assert_eq!(
            request_context_menu(&port, &invocation),
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Info,
                message: "Context menu unavailable: AppKit is unavailable".to_string(),
            })
        );
        assert_eq!(port.calls.get(), 1);
    }

    #[test]
    fn context_menu_failures_reduce_to_errors_without_refresh() {
        for (command, action) in [
            (ContextMenuCommand::Duplicate, "duplicate item"),
            (ContextMenuCommand::Compress, "start compression"),
            (ContextMenuCommand::MoveToTrash, "move item to Trash"),
        ] {
            assert_eq!(
                reduce_context_menu_result(
                    ContextMenuResult::Failed(ContextMenuFailure::Action {
                        command,
                        message: "permission denied".to_string(),
                    }),
                    Path::new("/tmp/example")
                ),
                Some(ContextMenuUiEffect::Notice {
                    level: ContextMenuNoticeLevel::Error,
                    message: format!("Could not {action}: permission denied"),
                })
            );
        }
        assert_eq!(
            reduce_context_menu_result(
                ContextMenuResult::Failed(ContextMenuFailure::MainThreadRequired),
                Path::new("/tmp/example")
            ),
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Error,
                message: "Context menu must run on the main thread".to_string(),
            })
        );
    }

    #[test]
    fn dismissed_context_menu_has_no_ui_effect() {
        assert_eq!(
            reduce_context_menu_result(ContextMenuResult::Dismissed, Path::new("/tmp/example")),
            None
        );
    }

    #[test]
    fn context_menu_trash_is_a_policy_intent_not_a_mutation_result() {
        let path = Path::new("/tmp/example");
        assert_eq!(
            reduce_context_menu_result(ContextMenuResult::MoveToTrashRequested, path),
            Some(ContextMenuUiEffect::MoveToTrash(path.to_path_buf()))
        );
    }

    #[test]
    fn nested_deferred_action_from_a_bad_port_is_reduced_once_to_a_terminal_error() {
        let path = PathBuf::from("/tmp/commander-nested-deferred");
        let action = ContextMenuAction::Duplicate(crate::ports::ContextMenuTarget {
            expected: crate::path_identity::PathIdentity::missing(&path),
            path: path.clone(),
        });
        let port = NestedDeferredContextMenuPort {
            calls: Cell::new(0),
        };

        let result = port.perform_deferred_action(&action);
        assert_eq!(port.calls.get(), 1);
        assert!(matches!(
            reduce_deferred_context_menu_result(result, &path),
            Some(ContextMenuUiEffect::Notice {
                level: ContextMenuNoticeLevel::Error,
                message,
            }) if message.contains("nested deferred action")
        ));
        assert_eq!(port.calls.get(), 1);
    }

    #[test]
    fn external_provider_protocol_is_bounded_and_process_isolated() {
        let client = ExternalProviderClient::new(
            PathBuf::from("/bin/sh"),
            vec![
                "-c".to_string(),
                "read request; printf '{\"ok\":true,\"payload\":\"ready\",\"error\":null}'"
                    .to_string(),
            ],
        );
        let response = client
            .request(
                &ProviderRpcRequest {
                    capability: ProviderCapability::PreviewText,
                    root: PathBuf::from("/tmp"),
                    path: Some(PathBuf::from("/tmp/a.txt")),
                    expression: None,
                    max_bytes: 1_024,
                },
                Duration::from_secs(1),
                1_024,
            )
            .unwrap();
        assert!(response.ok);
        assert_eq!(response.payload.as_deref(), Some("ready"));
    }
}
