//! Built when the `terminal` feature is off, to measure what the terminal costs.
//! Same API as `terminal.rs`, no PTY.

use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Shell,
    Claude(usize),
}

pub enum PanelEvent {
    Activity,
    Closed,
}

pub struct Session {
    pub id: usize,
    pub first_line: Option<String>,
    pub last_output: Option<Instant>,
}

impl Session {
    pub fn is_busy(&self) -> bool {
        false
    }
}

pub struct TerminalPanel {
    pub focus: FocusHandle,
    pub sessions: Vec<Session>,
    pub active: usize,
    pub message: Option<String>,
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl TerminalPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        TerminalPanel { focus: cx.focus_handle(), sessions: Vec::new(), active: 0, message: None }
    }
    pub fn open(&mut self, _: &Path, _: Kind, _: String, cx: &mut Context<Self>) -> Option<usize> {
        self.message = Some("terminal not built (feature `terminal` off)".into());
        cx.notify();
        None
    }
    pub fn label_for(&self, _: Kind) -> String {
        String::new()
    }
    pub fn close_all_sync(&mut self) {}
    pub fn close(&mut self, _: usize, _: &mut Context<Self>) {}
    pub fn session_by_id(&self, _: usize) -> Option<&Session> {
        None
    }
    pub fn activate_id(&mut self, _: usize, _: &mut Context<Self>) -> bool {
        false
    }
    pub fn cycle(&mut self, _: isize, _: &mut Context<Self>) {}
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().track_focus(&self.focus).size_full().children(self.message.clone())
    }
}

pub fn qur_home() -> PathBuf {
    std::env::var_os("QUR_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../out/qur-home"))
}
