//! Secure Keyboard Entry (`[terminal] secure_input`) and the bell's Dock bounce.
//! macOS keeps secure input to the app that turned it on while it's in front.

use std::sync::atomic::{AtomicBool, Ordering};

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn EnableSecureEventInput() -> i32;
    fn DisableSecureEventInput() -> i32;
}

static ON: AtomicBool = AtomicBool::new(false);

/// Turn secure input on or off. The calls are counted by macOS, so only a change calls.
pub fn set(on: bool) {
    if ON.swap(on, Ordering::Relaxed) != on {
        unsafe {
            if on { EnableSecureEventInput() } else { DisableSecureEventInput() };
        }
    }
}

/// Bounce the Dock icon once when Qur isn't the active app. Main thread only.
pub fn bounce_if_inactive() {
    use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        if !app.isActive() {
            app.requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
        }
    }
}
