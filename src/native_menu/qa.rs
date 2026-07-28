use super::*;

pub(crate) fn native_qa_renderer_check()
-> Result<crate::native_release_qa::NativeMenuRendererEvidence, String> {
    if !is_main_thread() {
        return Err("native menu renderer inspection requires the main thread".to_string());
    }
    if !ensure_class() {
        return Err("AppKit context-menu handler could not be registered".to_string());
    }
    let Some(handler_class) = Class::get("CmdrMenuHandlerV2") else {
        return Err("AppKit context-menu handler is unavailable".to_string());
    };
    let path = std::env::current_exe()
        .map_err(|error| format!("could not identify QA target: {error}"))?;
    let expected = crate::path_identity::PathIdentity::observe(&path)
        .map_err(|error| format!("could not bind QA target: {error}"))?;
    let invocation = build_invocation(
        next_invocation_id(),
        ContextMenuTarget { path, expected },
        "native-menu-accessibility-label-with-a-deliberately-long-name.txt",
        DynamicMenuEntries::default(),
    );

    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let mut state = Box::new(HandlerState::new(&invocation));
        let handler: *mut Object = msg_send![handler_class, new];
        if handler.is_null() {
            let _: () = msg_send![pool, drain];
            return Err("AppKit could not allocate the QA menu handler".to_string());
        }
        (*handler).set_ivar("rustState", state.as_mut() as *mut HandlerState as usize);
        let menu = match render_menu(&invocation.tree, handler, &mut state) {
            Ok(menu) => menu,
            Err(error) => {
                (*handler).set_ivar("rustState", 0_usize);
                let _: () = msg_send![handler, release];
                let _: () = msg_send![pool, drain];
                return Err(error);
            }
        };
        let inspected = inspect_rendered_menu(&invocation.tree, menu);
        let menu_size: NSSize = msg_send![menu, size];
        let popup_size = crate::native_release_qa::PopupSize {
            width: menu_size.width,
            height: menu_size.height,
        };
        (*handler).set_ivar("rustState", 0_usize);
        let _: () = msg_send![menu, release];
        let _: () = msg_send![handler, release];
        let _: () = msg_send![pool, drain];
        inspected.map(
            |count| crate::native_release_qa::NativeMenuRendererEvidence {
                check: crate::native_release_qa::CheckEvidence::passed(
                    "native_menu_renderer",
                    format!(
                        "{count} actual NSMenu items match model; content size is {:.1}x{:.1}",
                        menu_size.width, menu_size.height
                    ),
                ),
                popup_size: popup_size.is_valid().then_some(popup_size),
            },
        )
    }
}

unsafe fn inspect_rendered_menu(nodes: &[MenuNode], menu: *mut Object) -> Result<usize, String> {
    if menu.is_null() {
        return Err("rendered NSMenu is null".to_string());
    }
    let actual_count: usize = msg_send![menu, numberOfItems];
    if actual_count != nodes.len() {
        return Err(format!(
            "NSMenu item count {actual_count} does not match model {}",
            nodes.len()
        ));
    }
    let mut inspected = 0_usize;
    for (index, node) in nodes.iter().enumerate() {
        let actual: *mut Object = msg_send![menu, itemAtIndex: index];
        if actual.is_null() {
            return Err(format!("NSMenu item {index} is null"));
        }
        match node {
            MenuNode::Separator => {
                let separator: bool = msg_send![actual, isSeparatorItem];
                if !separator {
                    return Err(format!("NSMenu item {index} should be a separator"));
                }
            }
            MenuNode::Item(expected) => {
                let title: *mut Object = msg_send![actual, title];
                let title = unsafe { string_from_nsstring(title) }.unwrap_or_default();
                let enabled: bool = msg_send![actual, isEnabled];
                let state: isize = msg_send![actual, state];
                let key: *mut Object = msg_send![actual, keyEquivalent];
                let key = unsafe { string_from_nsstring(key) }.unwrap_or_default();
                let mask: usize = msg_send![actual, keyEquivalentModifierMask];
                let label: *mut Object = msg_send![actual, accessibilityLabel];
                let label = unsafe { string_from_nsstring(label) }.unwrap_or_default();
                let identifier: *mut Object = msg_send![actual, accessibilityIdentifier];
                let identifier = unsafe { string_from_nsstring(identifier) }.unwrap_or_default();
                let expected_key = expected
                    .key_equivalent
                    .as_ref()
                    .map_or("", |equivalent| equivalent.key.as_str());
                let expected_mask = expected
                    .key_equivalent
                    .as_ref()
                    .map_or(0, key_equivalent_modifier_mask);
                if title != expected.title
                    || enabled != expected.enabled
                    || (state != 0) != (expected.state == MenuItemState::On)
                    || key != expected_key
                    || mask != expected_mask
                    || label != expected.accessible_label
                    || identifier != expected.accessibility_id
                {
                    return Err(format!(
                        "NSMenu item {} diverged from model (title={title:?}, id={identifier:?}, label={label:?}, enabled={enabled}, state={state}, key={key:?}, mask={mask})",
                        expected.accessibility_id
                    ));
                }
                let submenu: *mut Object = msg_send![actual, submenu];
                if expected.children.is_empty() {
                    if !submenu.is_null() {
                        return Err(format!(
                            "NSMenu item {} has an unexpected submenu",
                            expected.accessibility_id
                        ));
                    }
                } else {
                    inspected += unsafe { inspect_rendered_menu(&expected.children, submenu) }?;
                }
                inspected += 1;
            }
        }
    }
    Ok(inspected)
}
