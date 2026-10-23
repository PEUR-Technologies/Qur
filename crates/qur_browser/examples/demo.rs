//! Embeds a Browser in a small window, loads the fixture page, simulates an inspect
//! pick and a text selection, snapshots the picked rect, and quits after ~5 s.
//!
//! `cargo run -p qur_browser --example demo -- [out_dir]`

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::Instant;

    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSTimer};
    use qur_browser::{Browser, BrowserEvent, Events};

    #[derive(PartialEq, Eq, Clone, Copy, Debug)]
    enum Step {
        WaitLoad,
        WaitPick,
        WaitText,
        WaitSpa,
        WaitEsc,
        WaitError,
        Done,
    }

    struct State {
        browser: Option<Browser>,
        events: Events,
        step: Step,
        start: Instant,
        out: PathBuf,
        _window: Retained<NSWindow>,
    }

    const PICK_JS: &str = r#"(function(){
        var el = document.querySelector('.actions .btn.primary');
        var r = el.getBoundingClientRect(), x = r.left + 4, y = r.top + 4;
        var o = {bubbles:true, cancelable:true, composed:true, clientX:x, clientY:y, view:window};
        el.dispatchEvent(new MouseEvent('mousemove', o));
        el.dispatchEvent(new MouseEvent('mousedown', o));
        el.dispatchEvent(new MouseEvent('mouseup', o));
        el.dispatchEvent(new MouseEvent('click', o));
    })();"#;

    const SELECT_JS: &str = r#"(function(){
        var p = document.getElementById('lead'), r = document.createRange();
        r.setStart(p.firstChild, 0); r.setEnd(p.firstChild, 26);
        var s = getSelection(); s.removeAllRanges(); s.addRange(r);
        p.dispatchEvent(new MouseEvent('mouseup', {bubbles:true, composed:true}));
        s.removeAllRanges();
    })();"#;

    pub fn run() {
        // Like GPUI's event handlers, do all setup inside an autorelease pool; otherwise
        // objects WebKit autoreleases during init (the web view included) never get released.
        let app = objc2::rc::autoreleasepool(|_| setup());
        app.run();
    }

    fn setup() -> Retained<NSApplication> {
        let mtm = MainThreadMarker::new().unwrap();
        let app = NSApplication::sharedApplication(mtm);
        // Accessory: no Dock icon, never steals focus from the owner's work.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let rect = NSRect::new(NSPoint::new(40.0, 40.0), NSSize::new(420.0, 320.0));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("qur_browser demo"));
        let content = window.contentView().unwrap();
        println!("contentView isFlipped = {}", content.isFlipped());

        let (browser, events) =
            unsafe { Browser::new(Retained::as_ptr(&content) as *mut std::ffi::c_void) };
        // Inset by 10 pt on all sides, top-left origin.
        browser.set_frame(10.0, 10.0, 400.0, 300.0);
        browser.set_visible(true);
        window.orderFrontRegardless();

        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/page.html");
        browser.load(fixture);

        let out = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let state = Rc::new(RefCell::new(State {
            browser: Some(browser),
            events,
            step: Step::WaitLoad,
            start: Instant::now(),
            out,
            _window: window,
        }));

        let block = block2::RcBlock::new(move |_t: std::ptr::NonNull<NSTimer>| {
            objc2::rc::autoreleasepool(|_| tick(&state))
        });
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.03, true, &block) };
        app
    }

    fn tick(state: &Rc<RefCell<State>>) {
        let mut st = state.borrow_mut();
        let elapsed = st.start.elapsed().as_secs_f64();
        // Linger so the WebKit helpers can be observed: drop the Browser at ~3 s, exit at ~5 s.
        if (st.step == Step::Done && elapsed > 3.0) || elapsed > 4.5 {
            if let Some(b) = st.browser.take() {
                println!("[{elapsed:.2}s] final title = {:?} (pick must not run onclick)", b.title());
                drop(b);
                println!("[{elapsed:.2}s] browser dropped; quitting");
                return;
            }
            if elapsed > 5.0 {
                std::process::exit(if st.step == Step::Done { 0 } else { 1 });
            }
            return;
        }
        while let Ok(ev) = st.events.try_recv() {
            print_event(elapsed, &ev);
            let Some(b) = st.browser.as_ref() else { break };
            let next = match (st.step, &ev) {
                (Step::WaitLoad, BrowserEvent::Loading(false)) => {
                    println!("  can_go_back={} url={:?}", b.can_go_back(), b.url());
                    b.set_inspect(true, "#ff7a45");
                    b.evaluate_js(PICK_JS);
                    Some(Step::WaitPick)
                }
                (Step::WaitPick, BrowserEvent::Picked(p)) => {
                    let path = st.out.join("qur_browser_pick.png");
                    b.snapshot(Some(p.rect), move |png| match png {
                        Some(bytes) => {
                            let n = bytes.len();
                            let dims = png_dims(&bytes);
                            std::fs::write(&path, bytes).unwrap();
                            println!("  snapshot: {n} bytes, {dims:?} px -> {}", path.display());
                        }
                        None => println!("  snapshot: FAILED"),
                    });
                    // Whole viewport, with the hover outline + label drawn by the inspector.
                    let page = st.out.join("qur_browser_page.png");
                    b.snapshot(None, move |png| {
                        let n = png.as_ref().map_or(0, Vec::len);
                        if let Some(bytes) = png {
                            std::fs::write(&page, bytes).unwrap();
                        }
                        println!("  page snapshot: {n} bytes -> {}", page.display());
                    });
                    b.evaluate_js(SELECT_JS);
                    Some(Step::WaitText)
                }
                (Step::WaitText, BrowserEvent::Text { .. }) => {
                    b.evaluate_js("history.pushState({}, '', '#spa-route')");
                    Some(Step::WaitSpa)
                }
                (Step::WaitSpa, BrowserEvent::Navigated { .. }) => {
                    b.evaluate_js(
                        "document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))",
                    );
                    Some(Step::WaitEsc)
                }
                (Step::WaitEsc, BrowserEvent::InspectEnded) => {
                    b.load("127.0.0.1:59999");
                    Some(Step::WaitError)
                }
                (Step::WaitError, BrowserEvent::Error(_)) => Some(Step::Done),
                _ => None,
            };
            if let Some(n) = next {
                st.step = n;
            }
        }
    }

    fn print_event(t: f64, ev: &BrowserEvent) {
        match ev {
            BrowserEvent::Picked(p) => {
                println!("[{t:.2}s] Picked");
                println!("    selector  = {}", p.selector);
                println!("    tag/id    = {} {:?} classes={:?}", p.tag, p.id, p.classes);
                println!("    text      = {:?}", p.text);
                println!("    outerHTML = {:?}", p.outer_html);
                println!("    rect      = {:?}", p.rect);
                println!("    url       = {}", p.url);
                println!("    source    = {:?}", p.source);
                println!("    styles    = {:?}", p.styles);
            }
            other => println!("[{t:.2}s] {other:?}"),
        }
    }

    fn png_dims(b: &[u8]) -> Option<(u32, u32)> {
        let w = u32::from_be_bytes(b.get(16..20)?.try_into().ok()?);
        let h = u32::from_be_bytes(b.get(20..24)?.try_into().ok()?);
        Some((w, h))
    }
}
