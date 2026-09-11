use super::contract::DisplayTopology;
#[cfg(target_os = "macos")]
use super::contract::ScreenRect;
#[cfg(feature = "visual-qa")]
use super::contract::{CapabilityState, NativeCapabilities};
#[cfg(feature = "visual-qa")]
use std::process::Command;

#[cfg(feature = "visual-qa")]
pub(super) fn command_stdout(program: &str, arguments: &[&str]) -> Option<String> {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(all(target_os = "macos", feature = "visual-qa"))]
pub(super) fn detect_capabilities(topology: &[DisplayTopology]) -> NativeCapabilities {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    let accessibility = if unsafe { AXIsProcessTrusted() } {
        CapabilityState::Available
    } else {
        CapabilityState::Denied
    };
    let screen_recording = if unsafe { CGPreflightScreenCaptureAccess() } {
        CapabilityState::Available
    } else {
        CapabilityState::Denied
    };
    let voice_over = match Command::new("/usr/bin/pgrep")
        .args(["-x", "VoiceOver"])
        .output()
    {
        Ok(output) if output.status.success() => CapabilityState::Available,
        Ok(output) if output.status.code() == Some(1) => CapabilityState::NotRunning,
        Ok(_) | Err(_) => CapabilityState::Unavailable,
    };
    NativeCapabilities {
        window_server: if topology.is_empty() {
            CapabilityState::Unavailable
        } else {
            CapabilityState::Available
        },
        accessibility,
        screen_recording,
        voice_over,
    }
}

#[cfg(all(not(target_os = "macos"), feature = "visual-qa"))]
pub(super) fn detect_capabilities(_topology: &[DisplayTopology]) -> NativeCapabilities {
    NativeCapabilities {
        window_server: CapabilityState::Unavailable,
        accessibility: CapabilityState::Unavailable,
        screen_recording: CapabilityState::Unavailable,
        voice_over: CapabilityState::Unavailable,
    }
}

#[cfg(target_os = "macos")]
pub fn capture_display_topology() -> Vec<DisplayTopology> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSRect {
        origin: NSPoint,
        size: NSSize,
    }

    unsafe {
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        let main: *mut Object = msg_send![class!(NSScreen), mainScreen];
        if screens.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![screens, count];
        let mut topology = Vec::with_capacity(count);
        for index in 0..count {
            let screen: *mut Object = msg_send![screens, objectAtIndex: index];
            if screen.is_null() {
                continue;
            }
            let frame: NSRect = msg_send![screen, frame];
            let visible: NSRect = msg_send![screen, visibleFrame];
            let backing_scale: f64 = msg_send![screen, backingScaleFactor];
            let main_screen: bool = !main.is_null() && msg_send![screen, isEqual: main];
            let description: *mut Object = msg_send![screen, deviceDescription];
            let screen_number_key: *mut Object = msg_send![
                class!(NSString),
                stringWithUTF8String: c"NSScreenNumber".as_ptr()
            ];
            let screen_number: *mut Object = if description.is_null() {
                std::ptr::null_mut()
            } else {
                msg_send![description, objectForKey: screen_number_key]
            };
            let frame = ScreenRect::new(
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            );
            let visible_frame = ScreenRect::new(
                visible.origin.x,
                visible.origin.y,
                visible.size.width,
                visible.size.height,
            );
            topology.push(DisplayTopology {
                id: if screen_number.is_null() {
                    format!("nsscreen-fallback-{index}")
                } else {
                    let display_id: u32 = msg_send![screen_number, unsignedIntValue];
                    format!("cgdisplay-{display_id}")
                },
                main: main_screen,
                frame,
                visible_frame,
                backing_scale,
            });
        }
        topology
    }
}

#[cfg(not(target_os = "macos"))]
pub fn capture_display_topology() -> Vec<DisplayTopology> {
    Vec::new()
}
