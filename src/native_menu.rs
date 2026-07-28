mod model;

use crate::ports::{
    ContextMenuAction, ContextMenuCommand, ContextMenuFailure, ContextMenuPort, ContextMenuResult,
    ContextMenuTarget,
};
use model::{
    DynamicMenuEntries, MenuInvocation, MenuItemState, MenuNode, MenuSelection, build_invocation,
    selection_result,
};
use objc::declare::ClassDecl;
use objc::runtime::{Class, NO, Object, Sel, YES};
use objc::{class, msg_send, sel, sel_impl};
use std::collections::BTreeSet;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
#[derive(Copy, Clone)]
struct NSPoint {
    x: f64,
    y: f64,
}

struct HandlerState {
    invocation_id: u64,
    bound_target: PathBuf,
    item_ids: Vec<model::MenuItemId>,
    selection: Option<MenuSelection>,
}

impl HandlerState {
    fn new(invocation: &MenuInvocation) -> Self {
        Self {
            invocation_id: invocation.id,
            bound_target: invocation.bound_target.path.clone(),
            item_ids: Vec::new(),
            selection: None,
        }
    }

    fn select(&mut self, index: usize) {
        if self.selection.is_some() {
            return;
        }
        let Some(item_id) = self.item_ids.get(index).cloned() else {
            return;
        };
        self.selection = Some(MenuSelection {
            invocation_id: self.invocation_id,
            bound_target: self.bound_target.clone(),
            item_id,
        });
    }
}

fn is_main_thread() -> bool {
    unsafe { msg_send![class!(NSThread), isMainThread] }
}

unsafe fn nsstring(value: &str) -> Result<*mut Object, String> {
    let value = CString::new(value).map_err(|_| "menu text contains a NUL byte".to_string())?;
    let string: *mut Object = msg_send![class!(NSString), stringWithUTF8String: value.as_ptr()];
    if string.is_null() {
        Err("AppKit could not create menu text".to_string())
    } else {
        Ok(string)
    }
}

unsafe fn string_from_nsstring(value: *mut Object) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let utf8: *const std::os::raw::c_char = msg_send![value, UTF8String];
    if utf8.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(utf8) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(unix)]
unsafe fn file_url(path: &Path) -> Option<*mut Object> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = CString::new(path.as_os_str().as_bytes()).ok()?;
    let nil: *mut Object = std::ptr::null_mut();
    let url: *mut Object = msg_send![class!(NSURL),
        fileURLWithFileSystemRepresentation: bytes.as_ptr()
        isDirectory: path.is_dir()
        relativeToURL: nil
    ];
    (!url.is_null()).then_some(url)
}

#[cfg(not(unix))]
unsafe fn file_url(path: &Path) -> Option<*mut Object> {
    let value = nsstring(&path.display().to_string()).ok()?;
    let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: value];
    (!url.is_null()).then_some(url)
}

#[cfg(unix)]
unsafe fn path_from_file_url(url: *mut Object) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    if url.is_null() {
        return None;
    }
    let bytes: *const std::os::raw::c_char = msg_send![url, fileSystemRepresentation];
    if bytes.is_null() {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(
        unsafe { CStr::from_ptr(bytes) }.to_bytes(),
    )))
}

#[cfg(not(unix))]
unsafe fn path_from_file_url(url: *mut Object) -> Option<PathBuf> {
    if url.is_null() {
        return None;
    }
    let path: *mut Object = msg_send![url, path];
    string_from_nsstring(path).map(PathBuf::from)
}

static REGISTERED: OnceLock<bool> = OnceLock::new();

fn ensure_class() -> bool {
    *REGISTERED.get_or_init(|| {
        if Class::get("CmdrMenuHandlerV2").is_some() {
            return true;
        }
        let Some(superclass) = Class::get("NSObject") else {
            return false;
        };
        let Some(mut decl) = ClassDecl::new("CmdrMenuHandlerV2", superclass) else {
            return false;
        };
        decl.add_ivar::<usize>("rustState");

        extern "C" fn action_select(this: &Object, _: Sel, sender: *mut Object) {
            if sender.is_null() {
                return;
            }
            unsafe {
                let state_ptr = *this.get_ivar::<usize>("rustState");
                if state_ptr == 0 {
                    return;
                }
                let represented: *mut Object = msg_send![sender, representedObject];
                if represented.is_null() {
                    return;
                }
                let index: usize = msg_send![represented, unsignedIntegerValue];
                let state = &mut *(state_ptr as *mut HandlerState);
                state.select(index);
            }
        }

        extern "C" fn action_noop(_: &Object, _: Sel, _: *mut Object) {}

        unsafe {
            type Action = extern "C" fn(&Object, Sel, *mut Object);
            decl.add_method(sel!(actionSelect:), action_select as Action);
            decl.add_method(sel!(actionNoop:), action_noop as Action);
        }
        decl.register();
        true
    })
}

unsafe fn render_menu(
    nodes: &[MenuNode],
    handler: *mut Object,
    state: &mut HandlerState,
) -> Result<*mut Object, String> {
    let menu: *mut Object = msg_send![class!(NSMenu), new];
    if menu.is_null() {
        return Err("AppKit could not allocate NSMenu".to_string());
    }
    let _: () = msg_send![menu, setAutoenablesItems: NO];

    for node in nodes {
        let result = match node {
            MenuNode::Separator => {
                let separator: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
                let _: () = msg_send![menu, addItem: separator];
                Ok(())
            }
            MenuNode::Item(item) => {
                let title = match unsafe { nsstring(&item.title) } {
                    Ok(title) => title,
                    Err(error) => {
                        let _: () = msg_send![menu, release];
                        return Err(error);
                    }
                };
                let key = match unsafe {
                    nsstring(
                        item.key_equivalent
                            .as_ref()
                            .map_or("", |equivalent| equivalent.key.as_str()),
                    )
                } {
                    Ok(key) => key,
                    Err(error) => {
                        let _: () = msg_send![menu, release];
                        return Err(error);
                    }
                };
                let action = if item.intent.is_some() {
                    sel!(actionSelect:)
                } else {
                    sel!(actionNoop:)
                };
                let menu_item: *mut Object = msg_send![class!(NSMenuItem), alloc];
                let menu_item: *mut Object = msg_send![menu_item,
                    initWithTitle: title
                    action: action
                    keyEquivalent: key
                ];
                if menu_item.is_null() {
                    Err("AppKit could not allocate NSMenuItem".to_string())
                } else {
                    let _: () = msg_send![menu_item, setTarget: handler];
                    let enabled = if item.enabled { YES } else { NO };
                    let _: () = msg_send![menu_item, setEnabled: enabled];
                    if item.state == MenuItemState::On {
                        let _: () = msg_send![menu_item, setState: 1_isize];
                    }
                    if let Ok(label) = unsafe { nsstring(&item.accessible_label) } {
                        let responds: bool =
                            msg_send![menu_item, respondsToSelector: sel!(setAccessibilityLabel:)];
                        if responds {
                            let _: () = msg_send![menu_item, setAccessibilityLabel: label];
                        }
                    }
                    if item.intent.is_some() {
                        let index = state.item_ids.len();
                        state.item_ids.push(item.id.clone());
                        let represented: *mut Object =
                            msg_send![class!(NSNumber), numberWithUnsignedInteger: index];
                        let _: () = msg_send![menu_item, setRepresentedObject: represented];
                    }
                    if !item.children.is_empty() {
                        match unsafe { render_menu(&item.children, handler, state) } {
                            Ok(submenu) => {
                                let _: () = msg_send![menu_item, setSubmenu: submenu];
                                let _: () = msg_send![submenu, release];
                            }
                            Err(error) => {
                                let _: () = msg_send![menu_item, release];
                                return Err(error);
                            }
                        }
                    }
                    let _: () = msg_send![menu, addItem: menu_item];
                    // NSMenu retains inserted items; balance alloc/init here.
                    let _: () = msg_send![menu_item, release];
                    Ok(())
                }
            }
        };
        if let Err(error) = result {
            let _: () = msg_send![menu, release];
            return Err(error);
        }
    }
    Ok(menu)
}

unsafe fn open_with_entries(path: &Path) -> Vec<(String, PathBuf)> {
    let Some(url) = (unsafe { file_url(path) }) else {
        return Vec::new();
    };
    let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
    if workspace.is_null() {
        return Vec::new();
    }
    let responds: bool =
        msg_send![workspace, respondsToSelector: sel!(urlsForApplicationsToOpenURL:)];
    if !responds {
        return Vec::new();
    }
    let urls: *mut Object = msg_send![workspace, urlsForApplicationsToOpenURL: url];
    if urls.is_null() {
        return Vec::new();
    }
    let count: usize = msg_send![urls, count];
    let mut applications = Vec::with_capacity(count);
    for index in 0..count {
        let app_url: *mut Object = msg_send![urls, objectAtIndex: index];
        let Some(path) = (unsafe { path_from_file_url(app_url) }) else {
            continue;
        };
        let component: *mut Object = msg_send![app_url, lastPathComponent];
        let title: *mut Object = msg_send![component, stringByDeletingPathExtension];
        if let Some(title) = unsafe { string_from_nsstring(title) } {
            applications.push((title, path));
        }
    }
    applications
}

unsafe fn read_tags(path: &Path) -> Result<BTreeSet<String>, String> {
    unsafe extern "C" {
        static NSURLTagNamesKey: *mut Object;
    }
    let Some(url) = (unsafe { file_url(path) }) else {
        return Err("the target path could not be represented by NSURL".to_string());
    };
    let mut value: *mut Object = std::ptr::null_mut();
    let mut error: *mut Object = std::ptr::null_mut();
    let ok: bool = msg_send![url,
        getResourceValue: &mut value
        forKey: unsafe { NSURLTagNamesKey }
        error: &mut error
    ];
    if !ok {
        return Err(unsafe { ns_error_message(error, "could not read Finder tags") });
    }
    if value.is_null() {
        return Ok(BTreeSet::new());
    }
    let count: usize = msg_send![value, count];
    let mut tags = BTreeSet::new();
    for index in 0..count {
        let tag: *mut Object = msg_send![value, objectAtIndex: index];
        if let Some(tag) = unsafe { string_from_nsstring(tag) } {
            tags.insert(tag);
        }
    }
    Ok(tags)
}

unsafe fn share_services(path: &Path) -> Vec<(String, String)> {
    let Some(url) = (unsafe { file_url(path) }) else {
        return Vec::new();
    };
    let items: *mut Object = msg_send![class!(NSArray), arrayWithObject: url];
    let services: *mut Object = msg_send![class!(NSSharingService), sharingServicesForItems: items];
    if services.is_null() {
        return Vec::new();
    }
    let count: usize = msg_send![services, count];
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let service: *mut Object = msg_send![services, objectAtIndex: index];
        let title: *mut Object = msg_send![service, title];
        let name: *mut Object = msg_send![service, name];
        if let (Some(title), Some(name)) = (unsafe { string_from_nsstring(title) }, unsafe {
            string_from_nsstring(name)
        }) {
            entries.push((title, name));
        }
    }
    entries
}

unsafe fn dynamic_entries(path: &Path) -> DynamicMenuEntries {
    let tags = unsafe { read_tags(path) };
    DynamicMenuEntries {
        open_with: unsafe { open_with_entries(path) },
        applied_tags: tags.clone().unwrap_or_default(),
        tags_available: tags.is_ok(),
        share_services: unsafe { share_services(path) },
    }
}

fn next_invocation_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Debug)]
pub struct MacOsContextMenu {
    _main_thread_only: PhantomData<Rc<()>>,
}

impl MacOsContextMenu {
    pub fn new() -> Result<Self, ContextMenuFailure> {
        if !is_main_thread() {
            return Err(ContextMenuFailure::MainThreadRequired);
        }
        Ok(Self {
            _main_thread_only: PhantomData,
        })
    }
}

impl ContextMenuPort for MacOsContextMenu {
    fn show_context_menu(&self, path: &Path) -> ContextMenuResult {
        show_native(path)
    }

    fn perform_deferred_action(&self, action: &ContextMenuAction) -> ContextMenuResult {
        if !is_main_thread() {
            return ContextMenuResult::Failed(ContextMenuFailure::MainThreadRequired);
        }
        if let Err(failure) = validate_action_target(action.target()) {
            return ContextMenuResult::Failed(failure);
        }
        match action {
            ContextMenuAction::Duplicate(target) => reduce_action_result(
                ContextMenuCommand::Duplicate,
                crate::fs_util::duplicate(&target.path),
            ),
            ContextMenuAction::Compress(target) => reduce_launch_result(
                ContextMenuCommand::Compress,
                crate::fs_util::compress_to_zip(&target.path),
            ),
            ContextMenuAction::ToggleTag { target, tag } => {
                reduce_action_result(ContextMenuCommand::ToggleTag, toggle_tag(&target.path, tag))
            }
            ContextMenuAction::Share { target, service } => reduce_launch_result(
                ContextMenuCommand::Share,
                perform_share(&target.path, service),
            ),
        }
    }
}

fn validate_action_target(target: &ContextMenuTarget) -> Result<(), ContextMenuFailure> {
    let current = crate::path_identity::PathIdentity::observe(&target.path).map_err(|error| {
        ContextMenuFailure::TargetUnavailable {
            message: error.to_string(),
        }
    })?;
    if target.expected.same_binding(&current) {
        Ok(())
    } else {
        Err(ContextMenuFailure::StaleInvocation)
    }
}

fn show_native(path: &Path) -> ContextMenuResult {
    if !is_main_thread() {
        return ContextMenuResult::Failed(ContextMenuFailure::MainThreadRequired);
    }
    if !ensure_class() {
        return ContextMenuResult::Unsupported {
            reason: "AppKit context-menu handler could not be registered".to_string(),
        };
    }
    let Some(handler_class) = Class::get("CmdrMenuHandlerV2") else {
        return ContextMenuResult::Unsupported {
            reason: "AppKit context-menu handler is unavailable".to_string(),
        };
    };

    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let expected = match crate::path_identity::PathIdentity::observe(path) {
            Ok(identity) if identity.exists => identity,
            Ok(_) => {
                let _: () = msg_send![pool, drain];
                return ContextMenuResult::Failed(ContextMenuFailure::StaleInvocation);
            }
            Err(error) => {
                let _: () = msg_send![pool, drain];
                return ContextMenuResult::Failed(ContextMenuFailure::TargetUnavailable {
                    message: error.to_string(),
                });
            }
        };
        let invocation = build_invocation(
            next_invocation_id(),
            ContextMenuTarget {
                path: path.to_path_buf(),
                expected,
            },
            &display_name(path),
            dynamic_entries(path),
        );
        let mut state = Box::new(HandlerState::new(&invocation));
        let handler: *mut Object = msg_send![handler_class, new];
        if handler.is_null() {
            let _: () = msg_send![pool, drain];
            return ContextMenuResult::Unsupported {
                reason: "AppKit could not allocate the menu handler".to_string(),
            };
        }
        (*handler).set_ivar("rustState", state.as_mut() as *mut HandlerState as usize);

        let menu = match render_menu(&invocation.tree, handler, &mut state) {
            Ok(menu) => menu,
            Err(reason) => {
                (*handler).set_ivar("rustState", 0_usize);
                let _: () = msg_send![handler, release];
                let _: () = msg_send![pool, drain];
                return ContextMenuResult::Unsupported { reason };
            }
        };
        let location: NSPoint = msg_send![class!(NSEvent), mouseLocation];
        let nil: *mut Object = std::ptr::null_mut();
        let _: bool = msg_send![menu,
            popUpMenuPositioningItem: nil
            atLocation: location
            inView: nil
        ];

        (*handler).set_ivar("rustState", 0_usize);
        let selection = state.selection.take();
        let _: () = msg_send![menu, release];
        let _: () = msg_send![handler, release];
        let result = selection_result(&invocation, selection);
        let _: () = msg_send![pool, drain];
        result
    }
}

fn reduce_action_result<T, E: std::fmt::Display>(
    command: ContextMenuCommand,
    result: Result<T, E>,
) -> ContextMenuResult {
    match result {
        Ok(_) => ContextMenuResult::RefreshRequested,
        Err(error) => ContextMenuResult::Failed(ContextMenuFailure::Action {
            command,
            message: error.to_string(),
        }),
    }
}

fn reduce_launch_result<T, E: std::fmt::Display>(
    command: ContextMenuCommand,
    result: Result<T, E>,
) -> ContextMenuResult {
    match result {
        Ok(_) => ContextMenuResult::Dismissed,
        Err(error) => ContextMenuResult::Failed(ContextMenuFailure::Action {
            command,
            message: error.to_string(),
        }),
    }
}

unsafe fn ns_error_message(error: *mut Object, fallback: &str) -> String {
    if error.is_null() {
        return fallback.to_string();
    }
    let description: *mut Object = msg_send![error, localizedDescription];
    unsafe { string_from_nsstring(description) }.unwrap_or_else(|| fallback.to_string())
}

fn toggle_tag(path: &Path, tag: &str) -> Result<(), String> {
    unsafe {
        let Some(url) = file_url(path) else {
            return Err("the target path could not be represented by NSURL".to_string());
        };
        let current = read_tags(path)?;
        let mut next = current;
        if !next.remove(tag) {
            next.insert(tag.to_string());
        }
        let array: *mut Object = msg_send![class!(NSMutableArray), array];
        for value in next {
            let value = nsstring(&value)?;
            let _: () = msg_send![array, addObject: value];
        }
        unsafe extern "C" {
            static NSURLTagNamesKey: *mut Object;
        }
        let mut error: *mut Object = std::ptr::null_mut();
        let ok: bool = msg_send![url,
            setResourceValue: array
            forKey: NSURLTagNamesKey
            error: &mut error
        ];
        if ok {
            Ok(())
        } else {
            Err(ns_error_message(error, "could not update Finder tags"))
        }
    }
}

fn perform_share(path: &Path, expected_name: &str) -> Result<(), String> {
    unsafe {
        let Some(url) = file_url(path) else {
            return Err("the target path could not be represented by NSURL".to_string());
        };
        let items: *mut Object = msg_send![class!(NSArray), arrayWithObject: url];
        let services: *mut Object =
            msg_send![class!(NSSharingService), sharingServicesForItems: items];
        if services.is_null() {
            return Err("sharing services are unavailable".to_string());
        }
        let count: usize = msg_send![services, count];
        for index in 0..count {
            let service: *mut Object = msg_send![services, objectAtIndex: index];
            let name: *mut Object = msg_send![service, name];
            if string_from_nsstring(name).as_deref() == Some(expected_name) {
                let _: () = msg_send![service, performWithItems: items];
                return Ok(());
            }
        }
        Err(format!(
            "the selected sharing service \"{expected_name}\" is no longer available"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_non_mutating_launch_does_not_request_refresh() {
        assert_eq!(
            reduce_launch_result(ContextMenuCommand::QuickLook, Ok::<(), std::io::Error>(())),
            ContextMenuResult::Dismissed
        );
    }

    #[test]
    fn successful_mutating_actions_request_refresh() {
        assert_eq!(
            reduce_action_result(ContextMenuCommand::Duplicate, Ok::<(), std::io::Error>(())),
            ContextMenuResult::RefreshRequested
        );
    }

    #[test]
    fn failed_actions_preserve_command_and_message() {
        for command in [
            ContextMenuCommand::OpenWith,
            ContextMenuCommand::QuickLook,
            ContextMenuCommand::GetInfo,
            ContextMenuCommand::Duplicate,
            ContextMenuCommand::Compress,
            ContextMenuCommand::ToggleTag,
            ContextMenuCommand::Share,
            ContextMenuCommand::MoveToTrash,
        ] {
            assert_eq!(
                reduce_action_result(
                    command,
                    Err::<(), _>(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "permission denied",
                    )),
                ),
                ContextMenuResult::Failed(ContextMenuFailure::Action {
                    command,
                    message: "permission denied".to_string(),
                })
            );
        }
    }
}
