use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel, NO};
use objc::{class, msg_send, sel, sel_impl};
use std::cell::Cell;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Once;

/// NSPoint / NSSize — same layout {f64, f64}
#[repr(C)]
#[derive(Copy, Clone)]
struct NSPoint {
    x: f64,
    y: f64,
}

thread_local! {
    static MENU_PATH: std::cell::RefCell<PathBuf> = std::cell::RefCell::new(PathBuf::new());
    static NEEDS_REFRESH: Cell<bool> = Cell::new(false);
}

unsafe fn nsstring(s: &str) -> *mut Object {
    let c = CString::new(s).unwrap_or_else(|_| CString::new("").unwrap());
    msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()]
}

fn with_path<F: FnOnce(&Path)>(f: F) {
    MENU_PATH.with(|p| f(&p.borrow()));
}

unsafe fn add_item(menu: *mut Object, title: &str, target: *mut Object, action: Sel) {
    let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
    let title_ns = nsstring(title);
    let key_ns = nsstring("");
    let item: *mut Object =
        msg_send![item, initWithTitle: title_ns action: action keyEquivalent: key_ns];
    let _: () = msg_send![item, setTarget: target];
    let _: () = msg_send![menu, addItem: item];
}

unsafe fn add_separator(menu: *mut Object) {
    let sep: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
    let _: () = msg_send![menu, addItem: sep];
}

static REGISTER: Once = Once::new();

fn ensure_class() {
    REGISTER.call_once(|| {
        let superclass = Class::get("NSObject").unwrap();
        let mut decl = ClassDecl::new("CmdrMenuHandler", superclass).unwrap();

        extern "C" fn action_open(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                let _ = open::that(p);
            });
        }

        extern "C" fn action_open_with(_: &Object, _: Sel, sender: *mut Object) {
            if sender.is_null() {
                return;
            }
            unsafe {
                let app_url: *mut Object = msg_send![sender, representedObject];
                if app_url.is_null() {
                    return;
                }
                let app_path_obj: *mut Object = msg_send![app_url, path];
                if app_path_obj.is_null() {
                    return;
                }
                let utf8: *const std::os::raw::c_char = msg_send![app_path_obj, UTF8String];
                if utf8.is_null() {
                    return;
                }
                let app_path = std::ffi::CStr::from_ptr(utf8)
                    .to_string_lossy()
                    .to_string();
                MENU_PATH.with(|p| {
                    let path = p.borrow();
                    let _ = std::process::Command::new("open")
                        .arg("-a")
                        .arg(&app_path)
                        .arg(path.as_os_str())
                        .spawn();
                });
            }
        }

        extern "C" fn action_quick_look(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                let _ = std::process::Command::new("qlmanage")
                    .arg("-p")
                    .arg(p)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            });
        }

        extern "C" fn action_get_info(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                let script = format!(
                    "tell application \"Finder\" to open information window of (POSIX file \"{}\" as alias)",
                    p.display()
                );
                let _ = std::process::Command::new("osascript")
                    .arg("-e")
                    .arg(&script)
                    .spawn();
            });
        }

        extern "C" fn action_duplicate(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                let parent = p.parent().unwrap_or(Path::new("/"));
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let ext = p
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                let dest = crate::fs_util::first_available(|i| {
                    if i == 0 {
                        parent.join(format!("{} copy{}", stem, ext))
                    } else {
                        parent.join(format!("{} copy {}{}", stem, i + 1, ext))
                    }
                });
                if p.is_dir() {
                    let _ = crate::fs_util::copy_dir_all(p, &dest);
                } else {
                    let _ = std::fs::copy(p, &dest);
                }
                NEEDS_REFRESH.with(|r| r.set(true));
            });
        }

        extern "C" fn action_compress(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                let name = p.file_name().unwrap().to_string_lossy();
                let parent = p.parent().unwrap_or(Path::new("/"));
                let archive = parent.join(format!("{}.zip", name));
                let _ = std::process::Command::new("ditto")
                    .arg("-c")
                    .arg("-k")
                    .arg("--sequesterRsrc")
                    .arg(p)
                    .arg(&archive)
                    .spawn();
                NEEDS_REFRESH.with(|r| r.set(true));
            });
        }

        extern "C" fn action_copy_path(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| unsafe {
                let pb: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
                let _: () = msg_send![pb, clearContents];
                let s = nsstring(&p.display().to_string());
                let arr: *mut Object = msg_send![class!(NSArray), arrayWithObject: s];
                let _: bool = msg_send![pb, writeObjects: arr];
            });
        }

        extern "C" fn action_show_in_finder(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
            });
        }

        extern "C" fn action_trash(_: &Object, _: Sel, _: *mut Object) {
            with_path(|p| {
                let _ = trash::delete(p);
                NEEDS_REFRESH.with(|r| r.set(true));
            });
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
                    let url: *mut Object =
                        msg_send![class!(NSURL), fileURLWithPath: path_ns];

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
                    let new_arr: *mut Object =
                        msg_send![class!(NSMutableArray), array];
                    let mut found = false;

                    if !tags_val.is_null() {
                        let count: usize = msg_send![tags_val, count];
                        for i in 0..count {
                            let t: *mut Object =
                                msg_send![tags_val, objectAtIndex: i];
                            let eq: bool =
                                msg_send![t, isEqualToString: tag_name];
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
                    let url: *mut Object =
                        msg_send![class!(NSURL), fileURLWithPath: path_ns];
                    let items: *mut Object =
                        msg_send![class!(NSArray), arrayWithObject: url];
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
    });
}

/// Build "Open With" submenu by querying NSWorkspace.
/// Returns the submenu NSMenu* (or null if unavailable).
unsafe fn build_open_with_submenu(handler: *mut Object, path: &Path) -> *mut Object {
    let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
    if workspace.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = path.display().to_string();
    let file_str = nsstring(&path_str);
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

        let key_ns = nsstring("");
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
    let path_ns = nsstring(&path.display().to_string());
    let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: path_ns];

    let mut current_tags: *mut Object = std::ptr::null_mut();
    let tags_ptr: *mut *mut Object = &mut current_tags;
    let nil_err: *mut Object = std::ptr::null_mut();
    let _: bool = msg_send![url,
        getResourceValue: tags_ptr
        forKey: NSURLTagNamesKey
        error: nil_err
    ];

    let tags: &[(&str, &str)] = &[
        ("Red",    "🔴"),
        ("Orange", "🟠"),
        ("Yellow", "🟡"),
        ("Green",  "🟢"),
        ("Blue",   "🔵"),
        ("Purple", "🟣"),
        ("Gray",   "⚪"),
    ];

    for &(name, dot) in tags {
        let title = format!("{}  {}", dot, name);
        let title_ns = nsstring(&title);
        let key_ns = nsstring("");
        let tag_ns = nsstring(name);

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
    let path_ns = nsstring(&path.display().to_string());
    let file_url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: path_ns];
    if file_url.is_null() {
        return std::ptr::null_mut();
    }

    let items: *mut Object = msg_send![class!(NSArray), arrayWithObject: file_url];
    let services: *mut Object =
        msg_send![class!(NSSharingService), sharingServicesForItems: items];
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

        let key_ns = nsstring("");
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

/// Show a native macOS NSMenu context menu for a file.
/// Returns `true` if the panel should be refreshed.
pub fn show(path: &Path) -> bool {
    ensure_class();
    MENU_PATH.with(|p| *p.borrow_mut() = path.to_path_buf());
    NEEDS_REFRESH.with(|r| r.set(false));

    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let handler_cls = Class::get("CmdrMenuHandler").unwrap();
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

    NEEDS_REFRESH.with(|r| r.get())
}
