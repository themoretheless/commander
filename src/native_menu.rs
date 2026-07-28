use crate::ports::{ContextMenuCommand, ContextMenuFailure, ContextMenuPort, ContextMenuResult};
use objc::declare::ClassDecl;
use objc::runtime::{Class, NO, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};
use std::cell::RefCell;
use std::ffi::CString;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

/// NSPoint / NSSize — same layout {f64, f64}
#[repr(C)]
#[derive(Copy, Clone)]
struct NSPoint {
    x: f64,
    y: f64,
}

thread_local! {
    static MENU_PATH: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
    static MENU_RESULT: RefCell<ContextMenuResult> = const { RefCell::new(ContextMenuResult::Dismissed) };
}

unsafe fn nsstring(s: &str) -> *mut Object {
    let c = CString::new(s).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
}

fn with_path<F: FnOnce(&Path)>(f: F) {
    MENU_PATH.with(|p| f(&p.borrow()));
}

fn set_menu_result(result: ContextMenuResult) {
    MENU_RESULT.with(|slot| *slot.borrow_mut() = result);
}

fn take_menu_result() -> ContextMenuResult {
    MENU_RESULT.with(|slot| slot.replace(ContextMenuResult::Dismissed))
}

fn record_action_result<T, E: std::fmt::Display>(
    command: ContextMenuCommand,
    result: Result<T, E>,
) {
    set_menu_result(reduce_action_result(command, result));
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

fn record_launch_result<T, E: std::fmt::Display>(
    command: ContextMenuCommand,
    result: Result<T, E>,
) {
    set_menu_result(reduce_launch_result(command, result));
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

fn record_action_failure(command: ContextMenuCommand, message: impl Into<String>) {
    set_menu_result(ContextMenuResult::Failed(ContextMenuFailure::Action {
        command,
        message: message.into(),
    }));
}

fn is_main_thread() -> bool {
    unsafe { msg_send![class!(NSThread), isMainThread] }
}

unsafe fn add_item(menu: *mut Object, title: &str, target: *mut Object, action: Sel) {
    let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
    let title_ns = unsafe { nsstring(title) };
    let key_ns = unsafe { nsstring("") };
    let item: *mut Object =
        msg_send![item, initWithTitle: title_ns action: action keyEquivalent: key_ns];
    let _: () = msg_send![item, setTarget: target];
    let _: () = msg_send![menu, addItem: item];
}

unsafe fn add_separator(menu: *mut Object) {
    let sep: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
    let _: () = msg_send![menu, addItem: sep];
}

static REGISTERED: OnceLock<bool> = OnceLock::new();

fn ensure_class() -> bool {
    *REGISTERED.get_or_init(|| {
        if Class::get("CmdrMenuHandler").is_some() {
            return true;
        }
        let Some(superclass) = Class::get("NSObject") else {
            return false;
        };
        let Some(mut decl) = ClassDecl::new("CmdrMenuHandler", superclass) else {
            return false;
        };

        extern "C" fn action_open(_: &Object, _: Sel, _: *mut Object) {
            set_menu_result(ContextMenuResult::OpenRequested);
        }

        extern "C" fn action_open_with(_: &Object, _: Sel, sender: *mut Object) {
            if sender.is_null() {
                record_action_failure(
                    ContextMenuCommand::OpenWith,
                    "the selected application was unavailable",
                );
                return;
            }
            unsafe {
                let app_url: *mut Object = msg_send![sender, representedObject];
                if app_url.is_null() {
                    record_action_failure(
                        ContextMenuCommand::OpenWith,
                        "the selected application URL was unavailable",
                    );
                    return;
                }
                let app_path_obj: *mut Object = msg_send![app_url, path];
                if app_path_obj.is_null() {
                    record_action_failure(
                        ContextMenuCommand::OpenWith,
                        "the selected application path was unavailable",
                    );
                    return;
                }
                let utf8: *const std::os::raw::c_char = msg_send![app_path_obj, UTF8String];
                if utf8.is_null() {
                    record_action_failure(
                        ContextMenuCommand::OpenWith,
                        "the selected application path could not be represented",
                    );
                    return;
                }
                let app_path = std::ffi::CStr::from_ptr(utf8).to_string_lossy().to_string();
                MENU_PATH.with(|p| {
                    let path = p.borrow();
                    record_launch_result(
                        ContextMenuCommand::OpenWith,
                        std::process::Command::new("open")
                            .arg("-a")
                            .arg(&app_path)
                            .arg(path.as_os_str())
                            .spawn(),
                    );
                });
            }
        }

        extern "C" fn action_quick_look(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                record_launch_result(
                    ContextMenuCommand::QuickLook,
                    std::process::Command::new("qlmanage")
                        .arg("-p")
                        .arg(p)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn(),
                );
            });
        }

        extern "C" fn action_get_info(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                record_launch_result(
                    ContextMenuCommand::GetInfo,
                    std::process::Command::new("osascript")
                        .arg("-e")
                        .arg("on run argv")
                        .arg("-e")
                        .arg(
                            "tell application \"Finder\" to open information window of \
                             (POSIX file (item 1 of argv) as alias)",
                        )
                        .arg("-e")
                        .arg("end run")
                        .arg("--")
                        .arg(p)
                        .spawn(),
                );
            });
        }

        extern "C" fn action_duplicate(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                record_action_result(ContextMenuCommand::Duplicate, crate::fs_util::duplicate(p));
            });
        }

        extern "C" fn action_compress(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                record_launch_result(
                    ContextMenuCommand::Compress,
                    crate::fs_util::compress_to_zip(p),
                );
            });
        }

        extern "C" fn action_copy_path(_: &Object, _: Sel, _: *mut Object) {
            set_menu_result(ContextMenuResult::CopyPathRequested);
        }

        extern "C" fn action_show_in_finder(_: &Object, _: Sel, _: *mut Object) {
            set_menu_result(ContextMenuResult::RevealRequested);
        }

        extern "C" fn action_trash(_: &Object, _: Sel, _: *mut Object) {
            set_menu_result(ContextMenuResult::MoveToTrashRequested);
        }

        extern "C" fn action_toggle_tag(_: &Object, _: Sel, sender: *mut Object) {
            if sender.is_null() {
                return;
            }
            unsafe {
                let tag_name: *mut Object = msg_send![sender, representedObject];
                if tag_name.is_null() {
                    return;
                }

                MENU_PATH.with(|p| {
                    let path = p.borrow();
                    let path_ns = nsstring(&path.display().to_string());
                    let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: path_ns];

                    unsafe extern "C" {
                        static NSURLTagNamesKey: *mut Object;
                    }

                    // Read current tags
                    let mut tags_val: *mut Object = std::ptr::null_mut();
                    let tags_ptr: *mut *mut Object = &mut tags_val;
                    let nil_err: *mut Object = std::ptr::null_mut();
                    let _: bool = msg_send![url,
                        getResourceValue: tags_ptr
                        forKey: NSURLTagNamesKey
                        error: nil_err
                    ];

                    // Collect existing tags, toggling the selected one
                    let new_arr: *mut Object = msg_send![class!(NSMutableArray), array];
                    let mut found = false;

                    if !tags_val.is_null() {
                        let count: usize = msg_send![tags_val, count];
                        for i in 0..count {
                            let t: *mut Object = msg_send![tags_val, objectAtIndex: i];
                            let eq: bool = msg_send![t, isEqualToString: tag_name];
                            if eq {
                                found = true; // skip = remove
                            } else {
                                let _: () = msg_send![new_arr, addObject: t];
                            }
                        }
                    }

                    if !found {
                        let _: () = msg_send![new_arr, addObject: tag_name];
                    }

                    let _: bool = msg_send![url,
                        setResourceValue: new_arr
                        forKey: NSURLTagNamesKey
                        error: nil_err
                    ];
                });
            }
        }

        extern "C" fn action_share(_: &Object, _: Sel, sender: *mut Object) {
            if sender.is_null() {
                return;
            }
            unsafe {
                let service: *mut Object = msg_send![sender, representedObject];
                if service.is_null() {
                    return;
                }
                MENU_PATH.with(|p| {
                    let path = p.borrow();
                    let path_ns = nsstring(&path.display().to_string());
                    let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: path_ns];
                    let items: *mut Object = msg_send![class!(NSArray), arrayWithObject: url];
                    let _: () = msg_send![service, performWithItems: items];
                });
            }
        }

        extern "C" fn action_noop(_: &Object, _: Sel, _: *mut Object) {}

        unsafe {
            type Fn = extern "C" fn(&Object, Sel, *mut Object);
            decl.add_method(sel!(actionOpen:), action_open as Fn);
            decl.add_method(sel!(actionOpenWith:), action_open_with as Fn);
            decl.add_method(sel!(actionQuickLook:), action_quick_look as Fn);
            decl.add_method(sel!(actionGetInfo:), action_get_info as Fn);
            decl.add_method(sel!(actionDuplicate:), action_duplicate as Fn);
            decl.add_method(sel!(actionCompress:), action_compress as Fn);
            decl.add_method(sel!(actionCopyPath:), action_copy_path as Fn);
            decl.add_method(sel!(actionShowInFinder:), action_show_in_finder as Fn);
            decl.add_method(sel!(actionTrash:), action_trash as Fn);
            decl.add_method(sel!(actionToggleTag:), action_toggle_tag as Fn);
            decl.add_method(sel!(actionShare:), action_share as Fn);
            decl.add_method(sel!(actionNoop:), action_noop as Fn);
        }

        decl.register();
        true
    })
}

/// Build "Open With" submenu by querying NSWorkspace.
/// Returns the submenu NSMenu* (or null if unavailable).
unsafe fn build_open_with_submenu(handler: *mut Object, path: &Path) -> *mut Object {
    let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
    if workspace.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = path.display().to_string();
    let file_str = unsafe { nsstring(&path_str) };
    let file_url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: file_str];
    if file_url.is_null() {
        return std::ptr::null_mut();
    }

    // urlsForApplicationsToOpenURL: (macOS 12+)
    // Check if the workspace responds to this selector first
    let responds: bool =
        msg_send![workspace, respondsToSelector: sel!(urlsForApplicationsToOpenURL:)];
    if !responds {
        return std::ptr::null_mut();
    }

    let app_urls: *mut Object = msg_send![workspace, urlsForApplicationsToOpenURL: file_url];
    if app_urls.is_null() {
        return std::ptr::null_mut();
    }

    let count: usize = msg_send![app_urls, count];
    if count == 0 {
        return std::ptr::null_mut();
    }

    let submenu: *mut Object = msg_send![class!(NSMenu), new];

    for i in 0..count {
        let app_url: *mut Object = msg_send![app_urls, objectAtIndex: i];
        if app_url.is_null() {
            continue;
        }

        let last_comp: *mut Object = msg_send![app_url, lastPathComponent];
        if last_comp.is_null() {
            continue;
        }
        let app_name: *mut Object = msg_send![last_comp, stringByDeletingPathExtension];
        if app_name.is_null() {
            continue;
        }

        let key_ns = unsafe { nsstring("") };
        let sub_item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let sub_item: *mut Object = msg_send![sub_item,
            initWithTitle: app_name
            action: sel!(actionOpenWith:)
            keyEquivalent: key_ns
        ];
        let _: () = msg_send![sub_item, setTarget: handler];
        let _: () = msg_send![sub_item, setRepresentedObject: app_url];

        // App icon (best-effort)
        let app_path_ns: *mut Object = msg_send![app_url, path];
        if !app_path_ns.is_null() {
            let icon: *mut Object = msg_send![workspace, iconForFile: app_path_ns];
            if !icon.is_null() {
                let sz = NSPoint { x: 16.0, y: 16.0 };
                let _: () = msg_send![icon, setSize: sz];
                let _: () = msg_send![sub_item, setImage: icon];
            }
        }

        let _: () = msg_send![submenu, addItem: sub_item];
    }

    submenu
}

/// Build a "Tags" submenu with the 7 standard Finder tag colours.
/// Already-applied tags get a checkmark (NSOnState).
unsafe fn build_tags_submenu(handler: *mut Object, path: &Path) -> *mut Object {
    let submenu: *mut Object = msg_send![class!(NSMenu), new];

    // Read current tags via NSURL resource values
    unsafe extern "C" {
        static NSURLTagNamesKey: *mut Object;
    }
    let path_ns = unsafe { nsstring(&path.display().to_string()) };
    let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: path_ns];

    let mut current_tags: *mut Object = std::ptr::null_mut();
    let tags_ptr: *mut *mut Object = &mut current_tags;
    let nil_err: *mut Object = std::ptr::null_mut();
    let tag_names_key: *mut Object = unsafe { NSURLTagNamesKey };
    let _: bool = msg_send![url,
        getResourceValue: tags_ptr
        forKey: tag_names_key
        error: nil_err
    ];

    let tags: &[(&str, &str)] = &[
        ("Red", "🔴"),
        ("Orange", "🟠"),
        ("Yellow", "🟡"),
        ("Green", "🟢"),
        ("Blue", "🔵"),
        ("Purple", "🟣"),
        ("Gray", "⚪"),
    ];

    for &(name, dot) in tags {
        let title = format!("{}  {}", dot, name);
        let title_ns = unsafe { nsstring(&title) };
        let key_ns = unsafe { nsstring("") };
        let tag_ns = unsafe { nsstring(name) };

        let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let item: *mut Object = msg_send![item,
            initWithTitle: title_ns
            action: sel!(actionToggleTag:)
            keyEquivalent: key_ns
        ];
        let _: () = msg_send![item, setTarget: handler];
        let _: () = msg_send![item, setRepresentedObject: tag_ns];

        // Checkmark if tag is already applied
        if !current_tags.is_null() {
            let has: bool = msg_send![current_tags, containsObject: tag_ns];
            if has {
                let _: () = msg_send![item, setState: 1_isize]; // NSOnState
            }
        }

        let _: () = msg_send![submenu, addItem: item];
    }

    submenu
}

/// Build a "Share" submenu via NSSharingService.
unsafe fn build_share_submenu(handler: *mut Object, path: &Path) -> *mut Object {
    let path_ns = unsafe { nsstring(&path.display().to_string()) };
    let file_url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: path_ns];
    if file_url.is_null() {
        return std::ptr::null_mut();
    }

    let items: *mut Object = msg_send![class!(NSArray), arrayWithObject: file_url];
    let services: *mut Object = msg_send![class!(NSSharingService), sharingServicesForItems: items];
    if services.is_null() {
        return std::ptr::null_mut();
    }

    let count: usize = msg_send![services, count];
    if count == 0 {
        return std::ptr::null_mut();
    }

    let submenu: *mut Object = msg_send![class!(NSMenu), new];

    for i in 0..count {
        let service: *mut Object = msg_send![services, objectAtIndex: i];
        if service.is_null() {
            continue;
        }

        let title: *mut Object = msg_send![service, title];
        if title.is_null() {
            continue;
        }

        let key_ns = unsafe { nsstring("") };
        let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let item: *mut Object = msg_send![item,
            initWithTitle: title
            action: sel!(actionShare:)
            keyEquivalent: key_ns
        ];
        let _: () = msg_send![item, setTarget: handler];
        let _: () = msg_send![item, setRepresentedObject: service];

        // Service icon (best-effort)
        let icon: *mut Object = msg_send![service, image];
        if !icon.is_null() {
            let sz = NSPoint { x: 16.0, y: 16.0 };
            let _: () = msg_send![icon, setSize: sz];
            let _: () = msg_send![item, setImage: icon];
        }

        let _: () = msg_send![submenu, addItem: item];
    }

    submenu
}

#[derive(Debug)]
pub struct MacOsContextMenu {
    // An Rc marker makes the AppKit adapter statically !Send and !Sync.
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
}

/// Show a native macOS NSMenu context menu for a file.
fn show_native(path: &Path) -> ContextMenuResult {
    if !is_main_thread() {
        return ContextMenuResult::Failed(ContextMenuFailure::MainThreadRequired);
    }
    if !ensure_class() {
        return ContextMenuResult::Unsupported {
            reason: "AppKit context-menu handler could not be registered".to_string(),
        };
    }
    let Some(handler_cls) = Class::get("CmdrMenuHandler") else {
        return ContextMenuResult::Unsupported {
            reason: "AppKit context-menu handler is unavailable".to_string(),
        };
    };
    MENU_PATH.with(|p| *p.borrow_mut() = path.to_path_buf());
    set_menu_result(ContextMenuResult::Dismissed);

    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let handler: *mut Object = msg_send![handler_cls, new];

        let menu: *mut Object = msg_send![class!(NSMenu), new];
        let _: () = msg_send![menu, setAutoenablesItems: NO];

        // ── Open ──
        add_item(menu, "Open", handler, sel!(actionOpen:));

        // ── Open With ▸ ──
        let ow_submenu = build_open_with_submenu(handler, path);
        if !ow_submenu.is_null() {
            let key_ns = nsstring("");
            let title_ns = nsstring("Open With");
            let ow_item: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let ow_item: *mut Object = msg_send![ow_item,
                initWithTitle: title_ns
                action: sel!(actionNoop:)
                keyEquivalent: key_ns
            ];
            let _: () = msg_send![ow_item, setSubmenu: ow_submenu];
            let _: () = msg_send![menu, addItem: ow_item];
        }

        // ── Quick Look ──
        add_item(menu, "Quick Look", handler, sel!(actionQuickLook:));
        add_separator(menu);

        add_item(menu, "Get Info", handler, sel!(actionGetInfo:));

        // ── Tags ▸ ──
        {
            let tags_sub = build_tags_submenu(handler, path);
            let title_ns = nsstring("Tags");
            let key_ns = nsstring("");
            let tags_item: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let tags_item: *mut Object = msg_send![tags_item,
                initWithTitle: title_ns
                action: sel!(actionNoop:)
                keyEquivalent: key_ns
            ];
            let _: () = msg_send![tags_item, setSubmenu: tags_sub];
            let _: () = msg_send![menu, addItem: tags_item];
        }
        add_separator(menu);

        add_item(menu, "Duplicate", handler, sel!(actionDuplicate:));

        let compress_title = format!(
            "Compress \"{}\"",
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        add_item(menu, &compress_title, handler, sel!(actionCompress:));
        add_separator(menu);

        add_item(menu, "Copy Path", handler, sel!(actionCopyPath:));

        // ── Share ▸ ──
        let share_sub = build_share_submenu(handler, path);
        if !share_sub.is_null() {
            let title_ns = nsstring("Share");
            let key_ns = nsstring("");
            let share_item: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let share_item: *mut Object = msg_send![share_item,
                initWithTitle: title_ns
                action: sel!(actionNoop:)
                keyEquivalent: key_ns
            ];
            let _: () = msg_send![share_item, setSubmenu: share_sub];
            let _: () = msg_send![menu, addItem: share_item];
        }
        add_separator(menu);

        add_item(menu, "Show in Finder", handler, sel!(actionShowInFinder:));
        add_separator(menu);

        add_item(menu, "Move to Trash", handler, sel!(actionTrash:));

        // ── Pop up at mouse location ──
        let loc: NSPoint = msg_send![class!(NSEvent), mouseLocation];
        let nil: *mut Object = std::ptr::null_mut();
        let _: bool = msg_send![menu,
            popUpMenuPositioningItem: nil
            atLocation: loc
            inView: nil
        ];

        let _: () = msg_send![handler, release];
        let _: () = msg_send![menu, release];
        let _: () = msg_send![pool, drain];
    }

    take_menu_result()
}

#[cfg(test)]
mod tests {
    use super::{
        ContextMenuCommand, ContextMenuFailure, ContextMenuResult, reduce_action_result,
        reduce_launch_result,
    };

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
    fn accepted_compression_spawn_does_not_request_refresh() {
        assert_eq!(
            reduce_launch_result(ContextMenuCommand::Compress, Ok::<(), std::io::Error>(())),
            ContextMenuResult::Dismissed
        );
    }

    #[test]
    fn failed_mutating_actions_preserve_their_errors_without_refresh() {
        for command in [
            ContextMenuCommand::OpenWith,
            ContextMenuCommand::QuickLook,
            ContextMenuCommand::GetInfo,
            ContextMenuCommand::Duplicate,
            ContextMenuCommand::Compress,
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
