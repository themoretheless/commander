mod model;
#[cfg(feature = "visual-qa")]
mod qa;

#[cfg(feature = "visual-qa")]
pub(crate) use qa::native_qa_renderer_check;

use crate::ports::{
    ContextMenuAction, ContextMenuAnchor, ContextMenuCommand, ContextMenuFailure,
    ContextMenuInvocation as ContextMenuPresentation, ContextMenuPort, ContextMenuResult,
    ContextMenuTarget, ContextMenuViewPoint, ContextMenuViewRect,
};
use model::{
    DynamicMenuEntries, MenuInvocation, MenuItemState, MenuNode, MenuSelection, build_invocation,
    tracking_result,
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

#[repr(C)]
#[derive(Copy, Clone)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct NSRect {
    origin: NSPoint,
    size: NSSize,
}

unsafe fn release_object(object: *mut Object) {
    let _: () = unsafe { msg_send![object, release] };
}

struct OwnedObject {
    object: *mut Object,
    releaser: unsafe fn(*mut Object),
}

impl OwnedObject {
    unsafe fn new(object: *mut Object) -> Option<Self> {
        (!object.is_null()).then_some(Self {
            object,
            releaser: release_object,
        })
    }

    fn as_ptr(&self) -> *mut Object {
        self.object
    }

    fn into_raw(mut self) -> *mut Object {
        let object = self.object;
        self.object = std::ptr::null_mut();
        object
    }

    #[cfg(test)]
    unsafe fn with_releaser(object: *mut Object, releaser: unsafe fn(*mut Object)) -> Self {
        Self { object, releaser }
    }
}

impl Drop for OwnedObject {
    fn drop(&mut self) {
        if !self.object.is_null() {
            unsafe { (self.releaser)(self.object) };
        }
    }
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
    let Some(menu) = (unsafe { OwnedObject::new(menu) }) else {
        return Err("AppKit could not allocate NSMenu".to_string());
    };
    let _: () = msg_send![menu.as_ptr(), setAutoenablesItems: NO];

    for node in nodes {
        match node {
            MenuNode::Separator => {
                let separator: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
                let _: () = msg_send![menu.as_ptr(), addItem: separator];
            }
            MenuNode::Item(item) => {
                let title = unsafe { nsstring(&item.title) }?;
                let key = unsafe {
                    nsstring(
                        item.key_equivalent
                            .as_ref()
                            .map_or("", |equivalent| equivalent.key.as_str()),
                    )
                }?;
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
                let menu_item = unsafe { OwnedObject::new(menu_item) }
                    .ok_or_else(|| "AppKit could not allocate NSMenuItem".to_string())?;
                let _: () = msg_send![menu_item.as_ptr(), setTarget: handler];
                let enabled = if item.enabled { YES } else { NO };
                let _: () = msg_send![menu_item.as_ptr(), setEnabled: enabled];
                if item.state == MenuItemState::On {
                    let _: () = msg_send![menu_item.as_ptr(), setState: 1_isize];
                }
                if let Ok(label) = unsafe { nsstring(&item.accessible_label) } {
                    let responds: bool = msg_send![
                        menu_item.as_ptr(),
                        respondsToSelector: sel!(setAccessibilityLabel:)
                    ];
                    if responds {
                        let _: () = msg_send![menu_item.as_ptr(), setAccessibilityLabel: label];
                    }
                }
                if let Ok(identifier) = unsafe { nsstring(&item.accessibility_id) } {
                    let responds: bool = msg_send![
                        menu_item.as_ptr(),
                        respondsToSelector: sel!(setAccessibilityIdentifier:)
                    ];
                    if responds {
                        let _: () = msg_send![
                            menu_item.as_ptr(),
                            setAccessibilityIdentifier: identifier
                        ];
                    }
                }
                let mask = item
                    .key_equivalent
                    .as_ref()
                    .map_or(0, key_equivalent_modifier_mask);
                let _: () = msg_send![menu_item.as_ptr(), setKeyEquivalentModifierMask: mask];
                if item.intent.is_some() {
                    let index = state.item_ids.len();
                    state.item_ids.push(item.id.clone());
                    let represented: *mut Object =
                        msg_send![class!(NSNumber), numberWithUnsignedInteger: index];
                    let _: () = msg_send![menu_item.as_ptr(), setRepresentedObject: represented];
                }
                if !item.children.is_empty() {
                    let submenu = unsafe { render_menu(&item.children, handler, state) }?;
                    let submenu = unsafe { OwnedObject::new(submenu) }
                        .ok_or_else(|| "AppKit returned a null submenu".to_string())?;
                    let _: () = msg_send![menu_item.as_ptr(), setSubmenu: submenu.as_ptr()];
                }
                let _: () = msg_send![menu.as_ptr(), addItem: menu_item.as_ptr()];
            }
        }
    }
    Ok(menu.into_raw())
}

fn key_equivalent_modifier_mask(equivalent: &model::KeyEquivalent) -> usize {
    const SHIFT: usize = 1 << 17;
    const CONTROL: usize = 1 << 18;
    const OPTION: usize = 1 << 19;
    const COMMAND: usize = 1 << 20;
    (usize::from(equivalent.shift) * SHIFT)
        | (usize::from(equivalent.control) * CONTROL)
        | (usize::from(equivalent.option) * OPTION)
        | (usize::from(equivalent.command) * COMMAND)
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
    fn show_context_menu(&self, invocation: &ContextMenuPresentation) -> ContextMenuResult {
        show_native(invocation)
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

fn view_anchor(rect: ContextMenuViewRect, view_height: f64) -> Option<NSPoint> {
    if ![
        rect.min_x,
        rect.min_y,
        rect.max_x,
        rect.max_y,
        rect.native_points_per_ui_point,
        view_height,
    ]
    .into_iter()
    .all(f64::is_finite)
        || rect.max_x <= rect.min_x
        || rect.max_y <= rect.min_y
        || rect.native_points_per_ui_point <= 0.0
        || view_height <= 0.0
    {
        return None;
    }
    let scale = rect.native_points_per_ui_point;
    let inset = ((rect.max_x - rect.min_x) / 2.0).min(24.0);
    let point = NSPoint {
        x: (rect.min_x + inset) * scale,
        y: view_height - rect.max_y * scale,
    };
    (point.x.is_finite() && point.y.is_finite()).then_some(point)
}

fn view_point_anchor(point: ContextMenuViewPoint, view_height: f64) -> Option<NSPoint> {
    if ![
        point.x,
        point.y,
        point.native_points_per_ui_point,
        view_height,
    ]
    .into_iter()
    .all(f64::is_finite)
        || point.native_points_per_ui_point <= 0.0
        || view_height <= 0.0
    {
        return None;
    }
    let converted = NSPoint {
        x: point.x * point.native_points_per_ui_point,
        y: view_height - point.y * point.native_points_per_ui_point,
    };
    (converted.x.is_finite() && converted.y.is_finite()).then_some(converted)
}

unsafe fn global_anchor(anchor: ContextMenuAnchor) -> Option<NSPoint> {
    match anchor {
        ContextMenuAnchor::GlobalScreen(point) if point.x.is_finite() && point.y.is_finite() => {
            return Some(NSPoint {
                x: point.x,
                y: point.y,
            });
        }
        ContextMenuAnchor::GlobalScreen(_) => return None,
        ContextMenuAnchor::ViewPoint(_) | ContextMenuAnchor::ViewRect(_) => {}
    }

    let application: *mut Object = unsafe { msg_send![class!(NSApplication), sharedApplication] };
    let window: *mut Object = unsafe { msg_send![application, keyWindow] };
    if window.is_null() {
        return None;
    }
    let view: *mut Object = unsafe { msg_send![window, contentView] };
    if view.is_null() {
        return None;
    }
    let bounds: NSRect = unsafe { msg_send![view, bounds] };
    let point_in_view = match anchor {
        ContextMenuAnchor::ViewPoint(point) => view_point_anchor(point, bounds.size.height)?,
        ContextMenuAnchor::ViewRect(rect) => view_anchor(rect, bounds.size.height)?,
        ContextMenuAnchor::GlobalScreen(_) => unreachable!(),
    };
    let point_in_window: NSPoint = unsafe {
        msg_send![view, convertPoint: point_in_view toView: std::ptr::null_mut::<Object>()]
    };
    Some(unsafe { msg_send![window, convertPointToScreen: point_in_window] })
}

fn show_native(presentation: &ContextMenuPresentation) -> ContextMenuResult {
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
        let Some(invocation_anchor) = global_anchor(presentation.anchor) else {
            let _: () = msg_send![pool, drain];
            return ContextMenuResult::Unsupported {
                reason: "AppKit could not resolve the captured context-menu anchor".to_string(),
            };
        };
        if let Err(failure) = validate_action_target(&presentation.target) {
            let _: () = msg_send![pool, drain];
            return ContextMenuResult::Failed(failure);
        }
        let path = &presentation.target.path;
        let invocation = build_invocation(
            next_invocation_id(),
            presentation.target.clone(),
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
        let menu_size: NSSize = msg_send![menu, size];
        // Screen layout can change while dynamic providers are queried. Keep
        // the invocation point, but place it against the topology at show time.
        let topology = crate::native_release_qa::capture_display_topology();
        let Some(placement) = crate::native_release_qa::place_popup(
            crate::native_release_qa::ScreenPoint {
                x: invocation_anchor.x,
                y: invocation_anchor.y,
            },
            crate::native_release_qa::PopupSize {
                width: menu_size.width,
                height: menu_size.height,
            },
            &topology,
        ) else {
            (*handler).set_ivar("rustState", 0_usize);
            let _: () = msg_send![menu, release];
            let _: () = msg_send![handler, release];
            let _: () = msg_send![pool, drain];
            return ContextMenuResult::Unsupported {
                reason: "AppKit menu content does not fit any NSScreen visibleFrame".to_string(),
            };
        };
        let location = NSPoint {
            x: placement.top_left.x,
            y: placement.top_left.y,
        };
        let nil: *mut Object = std::ptr::null_mut();
        let appkit_reported_selection: bool = msg_send![menu,
            popUpMenuPositioningItem: nil
            atLocation: location
            inView: nil
        ];

        (*handler).set_ivar("rustState", 0_usize);
        let selection = state.selection.take();
        let _: () = msg_send![menu, release];
        let _: () = msg_send![handler, release];
        let result = tracking_result(&invocation, appkit_reported_selection, selection);
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
    fn owned_object_releases_on_drop_but_not_after_transfer() {
        static RELEASES: AtomicU64 = AtomicU64::new(0);

        unsafe fn count_release(_object: *mut Object) {
            RELEASES.fetch_add(1, Ordering::Relaxed);
        }

        let object = std::ptr::NonNull::<Object>::dangling().as_ptr();
        RELEASES.store(0, Ordering::Relaxed);
        drop(unsafe { OwnedObject::with_releaser(object, count_release) });
        assert_eq!(RELEASES.load(Ordering::Relaxed), 1);

        let transferred = unsafe { OwnedObject::with_releaser(object, count_release) }.into_raw();
        assert_eq!(transferred, object);
        assert_eq!(RELEASES.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn owned_objects_release_every_parent_scope_on_nested_error() {
        static RELEASES: AtomicU64 = AtomicU64::new(0);

        unsafe fn count_release(_object: *mut Object) {
            RELEASES.fetch_add(1, Ordering::Relaxed);
        }

        fn fail_like_recursive_render(
            object: *mut Object,
            releaser: unsafe fn(*mut Object),
        ) -> Result<(), ()> {
            let _menu = unsafe { OwnedObject::with_releaser(object, releaser) };
            let _menu_item = unsafe { OwnedObject::with_releaser(object, releaser) };
            Err(())
        }

        let object = std::ptr::NonNull::<Object>::dangling().as_ptr();
        RELEASES.store(0, Ordering::Relaxed);
        assert_eq!(fail_like_recursive_render(object, count_release), Err(()));
        assert_eq!(RELEASES.load(Ordering::Relaxed), 2);
    }

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

    #[test]
    fn keyboard_anchor_converts_top_left_ui_coordinates_to_appkit_points() {
        let anchor = view_anchor(
            ContextMenuViewRect {
                min_x: 10.0,
                min_y: 20.0,
                max_x: 110.0,
                max_y: 44.0,
                native_points_per_ui_point: 2.0,
            },
            1_000.0,
        )
        .unwrap();
        assert_eq!(anchor.x, 68.0);
        assert_eq!(anchor.y, 912.0);
        assert!(
            view_anchor(
                ContextMenuViewRect {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 0.0,
                    max_y: 24.0,
                    native_points_per_ui_point: 1.0,
                },
                500.0
            )
            .is_none()
        );
        let pointer = view_point_anchor(
            ContextMenuViewPoint {
                x: 10.0,
                y: 20.0,
                native_points_per_ui_point: 2.0,
            },
            1_000.0,
        )
        .unwrap();
        assert_eq!(pointer.x, 20.0);
        assert_eq!(pointer.y, 960.0);
    }
}
