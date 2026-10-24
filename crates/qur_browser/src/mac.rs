use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use block2::RcBlock;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSView,
};
use objc2_foundation::{
    NSDictionary, NSError, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
    NSURLRequest,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationDelegate, WKScriptMessage, WKScriptMessageHandler,
    WKSnapshotConfiguration, WKUserContentController, WKUserScript, WKUserScriptInjectionTime,
    WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};

use crate::event::parse_message;
use crate::{BrowserEvent, Events, normalize_url};

const HANDLER: &str = "qur";
const SCRIPT: &str = include_str!("inspect.js");

struct Ivars {
    tx: UnboundedSender<BrowserEvent>,
    /// Accent colour while inspect mode is on; re-applied after each full load.
    inspect: RefCell<Option<String>>,
    loading: Cell<bool>,
    /// Last Navigated sent, to drop duplicates from commit / script / finish.
    last_nav: RefCell<(String, String)>,
}

define_class!(
    // Script-message handler and navigation delegate in one. It holds no reference to
    // the web view: the content controller retains it, the web view only weakly
    // references it as its navigation delegate.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "QurBrowserDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKScriptMessageHandler for Delegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _ucc: &WKUserContentController, msg: &WKScriptMessage) {
            let body = unsafe { msg.body() };
            if let Ok(s) = body.downcast::<NSString>()
                && let Some(ev) = parse_message(&s.to_string())
            {
                if matches!(ev, BrowserEvent::InspectEnded) {
                    self.ivars().inspect.replace(None);
                }
                self.send(ev);
            }
        }
    }

    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start(&self, _wv: &WKWebView, _nav: Option<&WKNavigation>) {
            self.set_loading(true);
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit(&self, wv: &WKWebView, _nav: Option<&WKNavigation>) {
            self.send_navigated(wv);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, wv: &WKWebView, _nav: Option<&WKNavigation>) {
            if let Some(accent) = self.ivars().inspect.borrow().as_deref() {
                eval(wv, &inspect_js(true, accent));
            }
            self.send_navigated(wv);
            self.set_loading(false);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _wv: &WKWebView, _nav: Option<&WKNavigation>, err: &NSError) {
            self.failed(err);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _wv: &WKWebView, _nav: Option<&WKNavigation>, err: &NSError) {
            self.failed(err);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn did_terminate(&self, _wv: &WKWebView) {
            self.set_loading(false);
            self.send(BrowserEvent::Error("The page's web content process stopped.".into()));
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, tx: UnboundedSender<BrowserEvent>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            tx,
            inspect: RefCell::new(None),
            loading: Cell::new(false),
            last_nav: RefCell::default(),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, ev: BrowserEvent) {
        if let BrowserEvent::Navigated { url, title } = &ev {
            let now = (url.clone(), title.clone());
            if *self.ivars().last_nav.borrow() == now {
                return;
            }
            self.ivars().last_nav.replace(now);
        }
        let _ = self.ivars().tx.unbounded_send(ev);
    }

    fn set_loading(&self, on: bool) {
        if self.ivars().loading.replace(on) != on {
            self.send(BrowserEvent::Loading(on));
        }
    }

    fn send_navigated(&self, wv: &WKWebView) {
        let (url, title) = url_title(wv);
        if let Some(url) = url {
            let mut title = title.unwrap_or_default();
            // WKWebView's title can lag behind the script's report; keep the known one.
            let last = self.ivars().last_nav.borrow().clone();
            if title.is_empty() && last.0 == url {
                title = last.1;
            }
            self.send(BrowserEvent::Navigated { url, title });
        }
    }

    fn failed(&self, err: &NSError) {
        self.set_loading(false);
        let domain = err.domain().to_string();
        let code = err.code();
        // Cancelled (a newer navigation replaced this one) and WebKit's
        // "frame load interrupted" (e.g. a download/policy change) aren't user-facing.
        if (domain == "NSURLErrorDomain" && code == -999)
            || (domain == "WebKitErrorDomain" && code == 102)
        {
            return;
        }
        self.send(BrowserEvent::Error(err.localizedDescription().to_string()));
    }
}

fn url_title(wv: &WKWebView) -> (Option<String>, Option<String>) {
    unsafe {
        (
            wv.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string()),
            wv.title().map(|s| s.to_string()),
        )
    }
}

fn eval(wv: &WKWebView, js: &str) {
    unsafe { wv.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
}

fn inspect_js(on: bool, accent: &str) -> String {
    let accent = serde_json::to_string(accent).unwrap_or_else(|_| "null".into());
    format!("window.__qur && window.__qur.inspect({on}, {accent});")
}

/// An embedded WKWebView. Main thread only. Dropping it removes the view from its
/// superview and stops loading.
pub struct Browser {
    web_view: Retained<WKWebView>,
    delegate: Retained<Delegate>,
    ucc: Retained<WKUserContentController>,
}

impl Browser {
    /// Add a WKWebView as a subview of `parent` (an `NSView*`, e.g. from GPUI's raw
    /// AppKit window handle). Hidden until [`set_frame`](Self::set_frame) /
    /// [`set_visible`](Self::set_visible).
    ///
    /// # Safety
    /// `parent` must be a valid `NSView*`, and this must be called on the main thread.
    pub unsafe fn new(parent: *mut c_void) -> (Browser, Events) {
        let mtm = MainThreadMarker::new().expect("qur_browser: Browser::new off the main thread");
        // SAFETY: the caller guarantees `parent` is a live NSView.
        let parent: &NSView = unsafe { &*parent.cast::<NSView>() };
        let (tx, rx) = unbounded();
        let delegate = Delegate::new(mtm, tx);
        // WebKit autoreleases objects (the web view among them) during setup; drain them
        // here so a caller without a pool in place doesn't leak the view.
        autoreleasepool(|_| unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::defaultDataStore(mtm));
            let ucc = config.userContentController();
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(SCRIPT),
                WKUserScriptInjectionTime::AtDocumentEnd,
                true,
            );
            ucc.addUserScript(&script);
            ucc.addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*delegate),
                &NSString::from_str(HANDLER),
            );

            let zero = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            let web_view =
                WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), zero, &config);
            web_view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web_view.setHidden(true);
            if std::env::var_os("QUR_WEB_DEVTOOLS").is_some_and(|v| v == "1")
                && web_view.respondsToSelector(sel!(setInspectable:))
            {
                web_view.setInspectable(true);
            }
            // Keep the top edge pinned between set_frame calls when the parent resizes.
            if !parent.isFlipped() {
                web_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            }
            parent.addSubview(&web_view);
            (Browser { web_view, delegate, ucc }, rx)
        })
    }

    /// Position in the parent's coordinate space, top-left origin, in points.
    pub fn set_frame(&self, x: f64, y: f64, w: f64, h: f64) {
        autoreleasepool(|_| {
            let (w, h) = (w.max(0.0), h.max(0.0));
            let y = match unsafe { self.web_view.superview() } {
                Some(p) if !p.isFlipped() => p.bounds().size.height - y - h,
                _ => y,
            };
            self.web_view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)));
        })
    }

    pub fn set_visible(&self, on: bool) {
        autoreleasepool(|_| {
            self.web_view.setHidden(!on);
        })
    }

    /// Load what the user typed; see [`normalize_url`] for the rules.
    pub fn load(&self, url: &str) {
        autoreleasepool(|_| {
            let url = normalize_url(url);
            unsafe {
                let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(&url)) else {
                    self.delegate.send(BrowserEvent::Error(format!("Invalid address: {url}")));
                    return;
                };
                if nsurl.isFileURL() {
                    // Allow the page's own folder (sibling CSS/JS/images).
                    let dir = nsurl.URLByDeletingLastPathComponent().unwrap_or_else(|| nsurl.clone());
                    self.web_view.loadFileURL_allowingReadAccessToURL(&nsurl, &dir);
                } else {
                    self.web_view.loadRequest(&NSURLRequest::requestWithURL(&nsurl));
                }
            }
        })
    }

    pub fn reload(&self) {
        autoreleasepool(|_| {
            unsafe { self.web_view.reload() };
        })
    }
    pub fn go_back(&self) {
        autoreleasepool(|_| {
            unsafe { self.web_view.goBack() };
        })
    }
    pub fn go_forward(&self) {
        autoreleasepool(|_| {
            unsafe { self.web_view.goForward() };
        })
    }
    pub fn stop(&self) {
        autoreleasepool(|_| {
            unsafe { self.web_view.stopLoading() };
            self.delegate.set_loading(false);
        })
    }
    pub fn url(&self) -> Option<String> {
        autoreleasepool(|_| {
            url_title(&self.web_view).0
        })
    }
    pub fn title(&self) -> Option<String> {
        autoreleasepool(|_| {
            url_title(&self.web_view).1
        })
    }
    pub fn can_go_back(&self) -> bool {
        autoreleasepool(|_| {
            unsafe { self.web_view.canGoBack() }
        })
    }
    pub fn can_go_forward(&self) -> bool {
        autoreleasepool(|_| {
            unsafe { self.web_view.canGoForward() }
        })
    }

    /// Inspect mode: hover outlines elements in `accent_css`, click emits
    /// [`BrowserEvent::Picked`], a text selection emits [`BrowserEvent::Text`], Esc ends it
    /// ([`BrowserEvent::InspectEnded`]). Stays on across navigations until turned off.
    pub fn set_inspect(&self, on: bool, accent_css: &str) {
        autoreleasepool(|_| {
            self.delegate.ivars().inspect.replace(on.then(|| accent_css.to_string()));
            eval(&self.web_view, &inspect_js(on, accent_css));
        })
    }

    /// Run JavaScript in the page (fire and forget). Used by tests/demos; handy for the app.
    pub fn evaluate_js(&self, js: &str) {
        autoreleasepool(|_| {
            eval(&self.web_view, js);
        })
    }

    /// PNG of the visible page, or of `rect` (CSS px, viewport-relative). `done(None)` on failure.
    pub fn snapshot(&self, rect: Option<[f64; 4]>, done: impl FnOnce(Option<Vec<u8>>) + 'static) {
        autoreleasepool(|_| {
            let rect = match rect.map(|r| self.points_rect(r)) {
                Some(None) => return done(None),
                Some(Some(r)) => Some(r),
                None => None,
            };
            let mtm = MainThreadMarker::from(&*self.web_view);
            let done = Cell::new(Some(done));
            let block = RcBlock::new(move |img: *mut NSImage, _err: *mut NSError| {
                let png = unsafe { img.as_ref() }.and_then(png_from_image);
                if let Some(f) = done.take() {
                    f(png);
                }
            });
            unsafe {
                let config = WKSnapshotConfiguration::new(mtm);
                if let Some(r) = rect {
                    config.setRect(r);
                }
                self.web_view.takeSnapshotWithConfiguration_completionHandler(Some(&config), &block);
            }
        })
    }

    /// CSS px (viewport-relative) to the web view's own point space, clamped to its
    /// bounds. WKWebView is flipped (top-left origin, like CSS) and one point is one CSS
    /// px times page zoom and magnification. None if nothing visible is left.
    fn points_rect(&self, [x, y, w, h]: [f64; 4]) -> Option<NSRect> {
        let k = unsafe { self.web_view.pageZoom() * self.web_view.magnification() };
        let b = self.web_view.bounds().size;
        let x0 = (x * k).clamp(0.0, b.width);
        let y0 = (y * k).clamp(0.0, b.height);
        let x1 = ((x + w) * k).clamp(0.0, b.width);
        let y1 = ((y + h) * k).clamp(0.0, b.height);
        (x1 - x0 >= 1.0 && y1 - y0 >= 1.0)
            .then(|| NSRect::new(NSPoint::new(x0, y0), NSSize::new(x1 - x0, y1 - y0)))
    }

    /// If the page has the keyboard, give it back to the view the web view sits
    /// in. Clicking that view doesn't take it back by itself, so typing kept
    /// going to the page.
    pub fn release_focus(&self) {
        autoreleasepool(|_| {
            let Some(window) = self.web_view.window() else { return };
            let Some(fr) = window.firstResponder() else { return };
            let inside = fr.downcast_ref::<NSView>().is_some_and(|v| v.isDescendantOf(&self.web_view));
            if inside && let Some(parent) = unsafe { self.web_view.superview() } {
                window.makeFirstResponder(Some(&parent));
            }
        });
    }

    /// Copy, paste… in the page. Sent down the responder chain from the web
    /// view (to the page's focused field or selection), giving the page the
    /// keyboard first if it hasn't got it.
    pub fn edit(&self, cmd: crate::EditCommand) {
        use crate::EditCommand::*;
        let action = match cmd {
            Copy => sel!(copy:),
            Cut => sel!(cut:),
            Paste => sel!(paste:),
            SelectAll => sel!(selectAll:),
            Undo => sel!(undo:),
            Redo => sel!(redo:),
        };
        autoreleasepool(|_| unsafe {
            let Some(window) = self.web_view.window() else { return };
            let inside = window.firstResponder().and_then(|r| r.downcast_ref::<NSView>().map(|v| v.isDescendantOf(&self.web_view))).unwrap_or(false);
            if !inside {
                window.makeFirstResponder(Some(&self.web_view));
            }
            // nil target: the first responder (inside the page) and up.
            let app: *mut objc2::runtime::AnyObject = msg_send![objc2::class!(NSApplication), sharedApplication];
            let none: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
            let _: objc2::runtime::Bool = msg_send![app, sendAction: action, to: none, from: none];
        });
    }

    /// Page zoom: `delta` steps of 10 % (0 resets to 100 %).
    pub fn zoom(&self, delta: i32) {
        autoreleasepool(|_| unsafe {
            let z = if delta == 0 { 1.0 } else { (self.web_view.pageZoom() + 0.1 * delta as f64).clamp(0.3, 3.0) };
            self.web_view.setPageZoom(z);
        });
    }

    /// Make the web view first responder (keyboard into the page).
    pub fn focus(&self) {
        autoreleasepool(|_| {
            if let Some(window) = self.web_view.window() {
                window.makeFirstResponder(Some(&self.web_view));
            }
        });
    }
}

fn png_from_image(img: &NSImage) -> Option<Vec<u8>> {
    let tiff = img.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let props = NSDictionary::new();
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(data.to_vec())
}

impl Drop for Browser {
    fn drop(&mut self) {
        autoreleasepool(|_| self.teardown());
    }
}

impl Browser {
    fn teardown(&self) {
        unsafe {
            self.web_view.stopLoading();
            self.web_view.setNavigationDelegate(None);
            self.ucc.removeScriptMessageHandlerForName(&NSString::from_str(HANDLER));
            self.ucc.removeAllUserScripts();
            self.web_view.removeFromSuperview();
        }
    }
}
