//! Embedded system WKWebView for Qur's Browser view, with an element-inspect mode.
//!
//! macOS only. On other platforms this crate builds as an empty shim: the event types
//! and pure helpers exist, `Browser` does not.
//!
//! Everything here is main-thread only (AppKit). Events leave through an unbounded
//! `futures` channel; there is no async runtime.

mod event;
mod url;

pub use event::{BrowserEvent, EditCommand, Picked};
pub use url::normalize_url;

/// Receiving end of a [`Browser`]'s event stream.
pub type Events = futures::channel::mpsc::UnboundedReceiver<BrowserEvent>;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::Browser;
