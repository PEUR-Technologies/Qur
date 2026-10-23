//! Alerts (handoff section 11, M6): a macOS notification, the Dock badge and
//! a soft chime when an agent finishes a turn, errors, or needs approval
//! while you're not looking at it. Notifications go through GPUI, which skips
//! them when Qur isn't running from an app bundle (dev builds).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{App, SystemNotification};

/// Bundled chime: two soft notes, 0.4 s (generated for Qur; see THIRD_PARTY).
static CHIME: &[u8] = include_bytes!("../../../assets/sounds/chime.wav");

thread_local! {
    /// Agents needing the user, per window (the badge is the sum).
    static NEEDS: RefCell<HashMap<u64, usize>> = RefCell::new(HashMap::new());
    static LAST_CHIME: Cell<Option<Instant>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Finished,
    NeedsApproval,
    Failed,
}

/// One alert for one agent. The caller has already decided it's warranted
/// (Qur isn't focused on that agent).
pub fn fire(cx: &mut App, cfg: &qur_config::AlertsConfig, tag: String, agent: &str, kind: Kind, detail: Option<&str>) {
    let (title, body) = match kind {
        Kind::Finished => (agent.to_string(), detail.map(str::to_string).unwrap_or_else(|| "Finished".into())),
        Kind::NeedsApproval => (format!("{agent} needs you"), detail.map(str::to_string).unwrap_or_else(|| "Waiting for your approval".into())),
        Kind::Failed => (format!("{agent} stopped"), detail.map(str::to_string).unwrap_or_else(|| "Error".into())),
    };
    if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
        println!("ALERT {kind:?} {title}: {body}");
    }
    if cfg.notify {
        cx.show_system_notification(SystemNotification { tag: tag.into(), title: title.into(), body: body.into(), actions: Vec::new() });
    }
    if cfg.sound {
        chime();
    }
}

/// The window's count of agents that need the user; the Dock shows the total.
pub fn set_needs(window: u64, n: usize, badge: bool) {
    let total = NEEDS.with(|m| {
        let mut m = m.borrow_mut();
        if n == 0 {
            m.remove(&window);
        } else {
            m.insert(window, n);
        }
        m.values().sum::<usize>()
    });
    set_badge(if badge && total > 0 { Some(total.to_string()) } else { None });
}

pub fn dismiss(cx: &App, tag: &str) {
    cx.dismiss_system_notification(tag);
}

fn chime() {
    // Several agents finishing together make one sound.
    let now = Instant::now();
    if LAST_CHIME.with(|l| l.get().is_some_and(|t| now.duration_since(t) < Duration::from_secs(2))) {
        return;
    }
    LAST_CHIME.with(|l| l.set(Some(now)));
    #[cfg(target_os = "macos")]
    {
        use objc2::AllocAnyThread;
        use objc2_app_kit::NSSound;
        use objc2_foundation::NSData;
        thread_local! {
            static SOUND: Option<objc2::rc::Retained<NSSound>> = {
                let data = NSData::with_bytes(CHIME);
                NSSound::initWithData(NSSound::alloc(), &data)
            };
        }
        SOUND.with(|s| {
            if let Some(s) = s {
                s.stop();
                s.play();
            }
        });
    }
}

fn set_badge(label: Option<String>) {
    if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
        println!("BADGE {}", label.as_deref().unwrap_or("-"));
    }
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        use objc2_foundation::NSString;
        let Some(mtm) = MainThreadMarker::new() else { return };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        tile.setBadgeLabel(label.map(|l| NSString::from_str(&l)).as_deref());
    }
}
