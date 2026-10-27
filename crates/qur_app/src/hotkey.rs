//! `[keys] global`: a system-wide key that brings Qur to the front, or hides
//! it when it's already in front. Carbon's RegisterEventHotKey needs no
//! Accessibility permission (an NSEvent global monitor would).

use std::ffi::c_void;
use std::sync::Mutex;

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(target: *mut c_void, h: Handler, n: u32, list: *const EventTypeSpec, user: *mut c_void, out: *mut *mut c_void) -> i32;
    fn RegisterEventHotKey(code: u32, mods: u32, id: EventHotKeyID, target: *mut c_void, opts: u32, out: *mut *mut c_void) -> i32;
    fn UnregisterEventHotKey(r: *mut c_void) -> i32;
}

/// The registered key (an EventHotKeyRef) and whether the handler is installed.
static STATE: Mutex<(usize, bool)> = Mutex::new((0, false));

extern "C" fn on_hotkey(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> i32 {
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        if app.isActive() {
            app.hide(None);
        } else {
            app.unhide(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }
    0
}

/// Register `keys` (GPUI syntax, e.g. "ctrl-`" or "cmd-shift-space"), replacing
/// any earlier one; "" turns it off. Main thread only.
pub fn set(keys: &str) -> Result<(), String> {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if st.0 != 0 {
        unsafe { UnregisterEventHotKey(st.0 as *mut c_void) };
        st.0 = 0;
    }
    if keys.trim().is_empty() {
        return Ok(());
    }
    let (code, mods) = parse(keys).ok_or_else(|| format!("[keys] global: can't use {keys:?}"))?;
    if let Some(what) = reserved(code, mods) {
        return Err(format!("[keys] global: {keys:?} is macOS's {what}; pick another"));
    }
    unsafe {
        let target = GetApplicationEventTarget();
        if !st.1 {
            // kEventClassKeyboard 'keyb', kEventHotKeyPressed 5
            let spec = EventTypeSpec { class: u32::from_be_bytes(*b"keyb"), kind: 5 };
            InstallEventHandler(target, on_hotkey, 1, &spec, std::ptr::null_mut(), std::ptr::null_mut());
            st.1 = true;
        }
        let mut r = std::ptr::null_mut();
        let id = EventHotKeyID { signature: u32::from_be_bytes(*b"qurk"), id: 1 };
        if RegisterEventHotKey(code, mods, id, target, 0, &mut r) != 0 {
            return Err(format!("[keys] global: {keys:?} is taken by another app"));
        }
        st.0 = r as usize;
    }
    Ok(())
}

/// ANSI virtual key codes (Carbon's kVK_*), by GPUI key name.
const KEYS: &[(&str, u32)] = &[
    ("a", 0x00), ("s", 0x01), ("d", 0x02), ("f", 0x03), ("h", 0x04), ("g", 0x05), ("z", 0x06), ("x", 0x07),
    ("c", 0x08), ("v", 0x09), ("b", 0x0b), ("q", 0x0c), ("w", 0x0d), ("e", 0x0e), ("r", 0x0f), ("y", 0x10),
    ("t", 0x11), ("1", 0x12), ("2", 0x13), ("3", 0x14), ("4", 0x15), ("6", 0x16), ("5", 0x17), ("=", 0x18),
    ("9", 0x19), ("7", 0x1a), ("-", 0x1b), ("8", 0x1c), ("0", 0x1d), ("]", 0x1e), ("o", 0x1f), ("u", 0x20),
    ("[", 0x21), ("i", 0x22), ("p", 0x23), ("enter", 0x24), ("l", 0x25), ("j", 0x26), ("'", 0x27), ("k", 0x28),
    (";", 0x29), ("\\", 0x2a), (",", 0x2b), ("/", 0x2c), ("n", 0x2d), ("m", 0x2e), (".", 0x2f), ("tab", 0x30),
    ("space", 0x31), ("`", 0x32), ("escape", 0x35), ("left", 0x7b), ("right", 0x7c), ("down", 0x7d), ("up", 0x7e),
];

/// Keys macOS or nearly every Mac app already use, and what for. A global key
/// would take them from every app.
const RESERVED: &[(&str, &str)] = &[
    ("cmd-space", "Spotlight"), ("alt-cmd-space", "Finder search"), ("ctrl-space", "input source switch"),
    ("ctrl-cmd-space", "emoji picker"), ("cmd-tab", "app switcher"), ("cmd-shift-tab", "app switcher"),
    ("cmd-`", "window switcher"), ("cmd-q", "Quit"), ("cmd-w", "Close"), ("cmd-h", "Hide"), ("alt-cmd-h", "Hide Others"),
    ("cmd-m", "Minimize"), ("cmd-,", "Settings"), ("cmd-a", "Select All"), ("cmd-c", "Copy"), ("cmd-v", "Paste"),
    ("cmd-x", "Cut"), ("cmd-z", "Undo"), ("cmd-shift-z", "Redo"), ("cmd-s", "Save"), ("cmd-o", "Open"), ("cmd-n", "New"),
    ("cmd-p", "Print"), ("cmd-f", "Find"), ("cmd-t", "New Tab"), ("cmd-shift-3", "screenshot"), ("cmd-shift-4", "screenshot"),
    ("cmd-shift-5", "screenshot"), ("cmd-shift-6", "screenshot"), ("ctrl-cmd-q", "Lock Screen"), ("cmd-shift-q", "Log Out"),
    ("ctrl-cmd-f", "Full Screen"), ("alt-cmd-escape", "Force Quit"), ("alt-cmd-d", "Dock hiding"), ("cmd-shift-/", "Help menu"),
    ("ctrl-left", "Spaces"), ("ctrl-right", "Spaces"), ("ctrl-up", "Mission Control"), ("ctrl-down", "App Exposé"),
    ("f11", "Show Desktop"), ("cmd-shift-.", "show hidden files"),
];

fn reserved(code: u32, mods: u32) -> Option<&'static str> {
    RESERVED.iter().find(|(k, _)| parse(k) == Some((code, mods))).map(|(_, what)| *what)
}

/// GPUI-style keys to a macOS virtual key code and Carbon modifier mask.
fn parse(keys: &str) -> Option<(u32, u32)> {
    let mut parts: Vec<&str> = keys.trim().split('-').collect();
    // "cmd--" is cmd plus the minus key.
    let key = if keys.ends_with("--") {
        parts.truncate(parts.len() - 2);
        "-"
    } else {
        parts.pop()?
    };
    let mut mods = 0;
    for m in parts {
        mods |= match m {
            "cmd" => 0x100,
            "shift" => 0x200,
            "alt" | "opt" => 0x800,
            "ctrl" => 0x1000,
            _ => return None,
        };
    }
    // Need a modifier unless it's a function key, or the key is useless while Qur runs.
    let f = key.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=20).contains(n));
    if mods == 0 && f.is_none() {
        return None;
    }
    let code = match f {
        Some(n) => [0x7a, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6d, 0x67, 0x6f, 0x69, 0x6b, 0x71, 0x6a, 0x40, 0x4f, 0x50, 0x5a][n - 1],
        None => KEYS.iter().find(|(k, _)| *k == key)?.1,
    };
    Some((code, mods))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_gpui_keys() {
        assert_eq!(parse("ctrl-`"), Some((0x32, 0x1000)));
        assert_eq!(parse("cmd-shift-space"), Some((0x31, 0x300)));
        assert_eq!(parse("alt-q"), Some((0x0c, 0x800)));
        assert_eq!(parse("ctrl-a"), Some((0x00, 0x1000)));
        assert_eq!(parse("cmd-b"), Some((0x0b, 0x100)));
        assert_eq!(parse("cmd-l"), Some((0x25, 0x100)));
        assert_eq!(parse("cmd--"), Some((0x1b, 0x100)));
        assert_eq!(parse("ctrl-pageup"), None);
        assert_eq!(parse("f13"), Some((0x69, 0)));
        assert_eq!(parse("q"), None);
        assert_eq!(parse("cmd-\\"), Some((0x2a, 0x100)));
        let r = |k| parse(k).and_then(|(c, m)| super::reserved(c, m));
        assert_eq!(r("cmd-space"), Some("Spotlight"));
        assert_eq!(r("shift-cmd-4"), Some("screenshot"));
        assert_eq!(r("ctrl-left"), Some("Spaces"));
        assert_eq!(r("cmd-\\"), None);
        assert!(super::RESERVED.iter().all(|(k, _)| parse(k).is_some()), "every reserved key parses");
        assert_eq!(parse("hyper-q"), None);
    }
}
