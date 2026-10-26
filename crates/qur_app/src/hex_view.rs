//! The hex view for binary files, in the file viewer: offset | hex | ASCII,
//! 16 bytes a row. Only the rows on screen are read from disk (seek + read),
//! so a file of any size opens at once and memory stays flat. Scroll, Go to
//! Offset (`Ctrl+G`), select with the mouse and copy as hex (`Cmd+C`).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use gpui::{
    ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, ScrollEvent, Subscription, Window, div, prelude::*, px,
};

use crate::commands::{edit, qur};
use crate::grid::{Grid, GridRow, GridSource, Metrics};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;

pub const ROW: u64 = 16;
/// Most bytes Copy puts on the clipboard (as hex text, three times that).
const MAX_COPY: u64 = 4 << 20;
/// Hex column width: 16 × "XX " plus the gap after byte 8.
const HEX_COLS: usize = 16 * 3 + 1;

pub enum HexEvent {
    Message(String),
}

pub struct HexView {
    pub focus: FocusHandle,
    path: PathBuf,
    len: u64,
    file: Option<File>,
    /// First row on screen.
    top: u64,
    scroll_acc: Pixels,
    metrics: Metrics,
    /// Bytes read around the screen: (offset, bytes).
    cache: (u64, Vec<u8>),
    /// Selected bytes (anchor, head), inclusive.
    sel: Option<(u64, u64)>,
    dragging: bool,
    goto: Option<(Entity<TextInput>, Subscription)>,
    error: Option<String>,
}

impl EventEmitter<HexEvent> for HexView {}

impl Focusable for HexView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// One row as text: (offset, hex, ASCII). `width` is the offset's digits.
pub fn format_row(offset: u64, bytes: &[u8], width: usize) -> (String, String, String) {
    let mut hex = String::with_capacity(HEX_COLS);
    for i in 0..ROW as usize {
        if i == 8 {
            hex.push(' ');
        }
        match bytes.get(i) {
            Some(b) => hex.push_str(&format!("{b:02X} ")),
            None => hex.push_str("   "),
        }
    }
    let ascii = bytes.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
    (format!("{offset:0width$X}"), hex, ascii)
}

/// Offset digits for a file of `len` bytes (at least 8).
pub fn offset_width(len: u64) -> usize {
    let digits = (64 - len.max(1).leading_zeros()).div_ceil(4) as usize;
    digits.max(8)
}

/// Which byte of a row a text column is on: the hex part, then the ASCII.
pub fn byte_at_col(col: usize) -> Option<usize> {
    if col < HEX_COLS {
        let c = if col >= 8 * 3 { col.saturating_sub(1) } else { col };
        Some((c / 3).min(ROW as usize - 1))
    } else if col > HEX_COLS && col <= HEX_COLS + ROW as usize {
        Some(col - HEX_COLS - 1)
    } else {
        None
    }
}

/// Parse "0x1F", "1Fh", "$1F" (hex) or "31" (decimal).
pub fn parse_offset(s: &str) -> Option<u64> {
    let s = s.trim().replace(['_', ' '], "");
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).or_else(|| s.strip_prefix('$')).or_else(|| s.strip_suffix(['h', 'H'])) {
        return u64::from_str_radix(h, 16).ok();
    }
    s.parse().ok().or_else(|| u64::from_str_radix(&s, 16).ok().filter(|_| s.chars().any(|c| c.is_ascii_alphabetic())))
}

/// Bytes as "DE AD BE EF".
pub fn hex_text(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

impl HexView {
    pub fn new(path: &Path, cx: &mut Context<Self>) -> Self {
        let file = File::open(path);
        let (file, error) = match file {
            Ok(f) => (Some(f), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let len = file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        HexView {
            focus: cx.focus_handle(),
            path: path.to_path_buf(),
            len,
            file,
            top: 0,
            scroll_acc: px(0.),
            metrics: Metrics::default(),
            cache: (0, Vec::new()),
            sel: None,
            dragging: false,
            goto: None,
            error,
        }
    }

    fn rows_total(&self) -> u64 {
        self.len.div_ceil(ROW).max(1)
    }

    fn max_top(&self) -> u64 {
        self.rows_total().saturating_sub(self.metrics.page() as u64 / 2).min(self.rows_total() - 1)
    }

    fn scroll_to(&mut self, top: u64, cx: &mut Context<Self>) {
        self.top = top.min(self.max_top());
        cx.notify();
    }

    /// Read `n` bytes at `at` (fewer at the end of the file).
    fn read_at(&mut self, at: u64, n: u64) -> std::io::Result<Vec<u8>> {
        let Some(f) = self.file.as_mut() else { return Ok(Vec::new()) };
        let n = n.min(self.len.saturating_sub(at));
        let mut buf = vec![0u8; n as usize];
        f.seek(SeekFrom::Start(at))?;
        let mut got = 0;
        while got < buf.len() {
            match f.read(&mut buf[got..])? {
                0 => break,
                k => got += k,
            }
        }
        buf.truncate(got);
        Ok(buf)
    }

    /// The bytes from `at` for `n`, from the cache, re-reading a page either
    /// side of the screen when it doesn't cover them.
    fn bytes(&mut self, at: u64, n: u64) -> &[u8] {
        let (c0, ref c) = self.cache;
        let end = (at + n).min(self.len);
        if !(at >= c0 && end <= c0 + c.len() as u64) {
            let page = n.max(ROW * 64);
            let start = at.saturating_sub(page);
            match self.read_at(start, page * 3) {
                Ok(b) => self.cache = (start, b),
                Err(e) => {
                    self.error = Some(e.to_string());
                    self.cache = (at, Vec::new());
                }
            }
        }
        let (c0, ref c) = self.cache;
        let from = (at.saturating_sub(c0) as usize).min(c.len());
        let to = (end.saturating_sub(c0) as usize).clamp(from, c.len());
        &c[from..to]
    }

    fn sel_range(&self) -> Option<(u64, u64)> {
        self.sel.map(|(a, h)| (a.min(h), a.max(h)))
    }

    /// The byte under a window position.
    fn hit(&self, p: gpui::Point<Pixels>) -> Option<u64> {
        let (row, col) = self.metrics.cell_at(p);
        let i = byte_at_col(col)?;
        let off = (self.top + row as u64) * ROW + i as u64;
        (self.len > 0).then(|| off.min(self.len - 1))
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(off) = self.hit(ev.position) {
            self.sel = match (ev.modifiers.shift, self.sel) {
                (true, Some((a, _))) => Some((a, off)),
                _ => Some((off, off)),
            };
            self.dragging = true;
            cx.notify();
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging || ev.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        let b = self.metrics.bounds;
        if ev.position.y < b.top() {
            self.top = self.top.saturating_sub(1);
        } else if ev.position.y > b.bottom() {
            self.top = (self.top + 1).min(self.max_top());
        }
        if let (Some(off), Some((a, _))) = (self.hit(ev.position), self.sel) {
            self.sel = Some((a, off));
        }
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lh = self.metrics.line_h;
        self.scroll_acc -= ev.delta.pixel_delta(lh).y;
        let rows = (self.scroll_acc / lh).trunc();
        if rows != 0. {
            self.scroll_acc -= lh * rows;
            let top = if rows < 0. { self.top.saturating_sub((-rows) as u64) } else { self.top + rows as u64 };
            self.scroll_to(top, cx);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let page = self.metrics.page() as u64;
        let top = match (k.key.as_str(), k.modifiers.platform) {
            ("up", false) => self.top.saturating_sub(1),
            ("down", false) => self.top + 1,
            ("pageup", _) => self.top.saturating_sub(page),
            ("pagedown", _) | ("space", false) => self.top + page,
            ("home", _) | ("up", true) => 0,
            ("end", _) | ("down", true) => u64::MAX,
            _ => return,
        };
        self.scroll_to(top, cx);
        cx.stop_propagation();
    }

    fn copy(&mut self, _: &edit::Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some((a, b)) = self.sel_range() else { return };
        let n = (b - a + 1).min(MAX_COPY);
        match self.read_at(a, n) {
            Ok(bytes) => {
                cx.write_to_clipboard(ClipboardItem::new_string(hex_text(&bytes)));
                let more = if b - a + 1 > MAX_COPY { " (the first 4 MB)" } else { "" };
                cx.emit(HexEvent::Message(format!("Copied {} byte{} as hex{more}", bytes.len(), if bytes.len() == 1 { "" } else { "s" })));
            }
            Err(e) => cx.emit(HexEvent::Message(format!("Couldn't read {}: {e}", self.path.display()))),
        }
    }

    fn select_all(&mut self, _: &edit::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.len > 0 {
            self.sel = Some((0, self.len - 1));
            cx.notify();
        }
    }

    /// Go to Line (`Ctrl+G`) here goes to an offset.
    fn goto_action(&mut self, _: &qur::GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        self.open_goto(window, cx);
    }

    pub fn open_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new("Go to offset (0x1F00 or 7936)", cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let text = input.read(cx).text.clone();
                match parse_offset(&text) {
                    Some(off) => {
                        this.go_to(off, cx);
                        this.close_bar(window, cx);
                    }
                    None => cx.emit(HexEvent::Message(format!("Not an offset: {text}"))),
                }
            }
            InputEvent::Cancel => this.close_bar(window, cx),
            _ => {}
        });
        let f = input.read(cx).focus.clone();
        self.goto = Some((input, sub));
        window.focus(&f, cx);
        cx.notify();
    }

    /// Show byte `off` a few rows from the top and select it.
    pub fn go_to(&mut self, off: u64, cx: &mut Context<Self>) {
        let off = off.min(self.len.saturating_sub(1));
        self.sel = Some((off, off));
        self.scroll_to((off / ROW).saturating_sub(self.metrics.page() as u64 / 3), cx);
    }

    /// Esc with the Go to Offset bar open closes just the bar.
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.goto.is_none() {
            return false;
        }
        self.close_bar(window, cx);
        true
    }

    fn close_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goto = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Scripted runs' `state`: "hex top=0x… sel=0x…-0x… len=…".
    pub fn debug_state(&self) -> String {
        let sel = self.sel_range().map_or_else(|| "-".into(), |(a, b)| format!("{a:#X}-{b:#X}"));
        format!("hex len={} top={:#X} sel={sel} goto={}", self.len, self.top * ROW, self.goto.is_some())
    }
}

impl GridSource for HexView {
    fn gutter_cols(&self) -> usize {
        offset_width(self.len)
    }

    fn grid_rows(&mut self, m: Metrics, look: &Look, _: &mut Context<Self>) -> Vec<GridRow> {
        self.metrics = m;
        self.top = self.top.min(self.max_top());
        let ui = &look.theme.ui;
        let (text, muted, dim) = (look.c(look.theme.editor.text), look.c(ui.text_muted), look.c(ui.text_secondary));
        let sel_bg = look.c(ui.selection);
        let width = offset_width(self.len);
        let first = self.top * ROW;
        let n = (m.rows as u64) * ROW;
        let sel = self.sel_range();
        let bytes = self.bytes(first, n).to_vec();
        let mut out = Vec::with_capacity(m.rows);
        for (r, chunk) in bytes.chunks(ROW as usize).enumerate() {
            let off = first + r as u64 * ROW;
            let (o, _, _) = format_row(off, chunk, width);
            let mut row = GridRow::new(Some(o.into()));
            let in_sel = |i: usize| sel.is_some_and(|(a, b)| (a..=b).contains(&(off + i as u64)));
            for i in 0..ROW as usize {
                if i == 8 {
                    row.push(" ", text, (in_sel(7) && in_sel(8)).then_some(sel_bg));
                }
                match chunk.get(i) {
                    Some(&b) => {
                        let color = if b == 0 { muted } else if (0x20..0x7F).contains(&b) { text } else { dim };
                        let bg = in_sel(i).then_some(sel_bg);
                        row.push(&format!("{b:02X}"), color, bg);
                        let gap_bg = (in_sel(i) && in_sel(i + 1) && i != 7 && chunk.get(i + 1).is_some()).then_some(sel_bg);
                        row.push(" ", text, gap_bg);
                    }
                    None => row.push("   ", text, None),
                }
            }
            row.push(" ", text, None);
            for (i, &b) in chunk.iter().enumerate() {
                let printable = (0x20..0x7F).contains(&b);
                let c = if printable { (b as char).to_string() } else { ".".to_string() };
                row.push(&c, if printable { text } else { muted }, in_sel(i).then_some(sel_bg));
            }
            out.push(row);
        }
        if out.is_empty() {
            let mut row = GridRow::new(None);
            row.push(if self.len == 0 { "(empty file)" } else { "" }, muted, None);
            out.push(row);
        }
        out
    }

    fn scroll_thumb(&self) -> Option<(f32, f32)> {
        let total = self.rows_total();
        let page = self.metrics.page() as u64;
        (total > page).then(|| (self.top as f32 / self.max_top().max(1) as f32, page as f32 / total as f32))
    }
}

impl Render for HexView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let sel = self.sel_range();
        let status = match (&self.error, sel) {
            (Some(e), _) => e.clone(),
            (None, Some((a, b))) if a == b => format!("offset {a:#X} ({a})"),
            (None, Some((a, b))) => format!("{a:#X}–{b:#X} · {} bytes selected · ⌘C copies as hex", b - a + 1),
            (None, None) => format!("{} bytes · ⌃G go to offset", self.len),
        };
        div()
            .key_context("HexView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::goto_action))
            .children(self.goto.as_ref().map(|(input, _)| {
                div()
                    .flex_none()
                    .px(look.z(8.))
                    .py(look.z(6.))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(div().px(look.z(6.)).border_1().border_color(look.c(ui.border)).rounded(look.z(4.)).child(input.clone()))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .child(Grid { view: cx.entity() }),
            )
            .child(
                div()
                    .flex_none()
                    .px(look.z(10.))
                    .py(look.z(3.))
                    .border_t_1()
                    .border_color(look.c(ui.border))
                    .text_size(look.text(-2.))
                    .text_color(look.c(ui.text_muted))
                    .child(status),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_format_like_xxd() {
        let (o, hex, ascii) = format_row(0x10, b"Hello, world!\n\0\xFF", 8);
        assert_eq!(o, "00000010");
        assert_eq!(hex, "48 65 6C 6C 6F 2C 20 77  6F 72 6C 64 21 0A 00 FF ");
        assert_eq!(ascii, "Hello, world!...");
        // A short last row keeps the columns lined up.
        let (_, hex, ascii) = format_row(0, b"AB", 8);
        assert_eq!(hex.len(), HEX_COLS);
        assert_eq!(hex.trim_end(), "41 42");
        assert_eq!(ascii, "AB");
    }

    #[test]
    fn offset_digits_grow_with_the_file() {
        assert_eq!(offset_width(0), 8);
        assert_eq!(offset_width(0xFFFF_FFFF), 8);
        assert_eq!(offset_width(0x1_0000_0000), 9);
    }

    #[test]
    fn columns_map_to_bytes() {
        assert_eq!(byte_at_col(0), Some(0));
        assert_eq!(byte_at_col(2), Some(0));
        assert_eq!(byte_at_col(3), Some(1));
        assert_eq!(byte_at_col(23), Some(7));
        // After the mid-row gap.
        assert_eq!(byte_at_col(25), Some(8));
        assert_eq!(byte_at_col(HEX_COLS - 1), Some(15));
        assert_eq!(byte_at_col(HEX_COLS), None);
        assert_eq!(byte_at_col(HEX_COLS + 1), Some(0));
        assert_eq!(byte_at_col(HEX_COLS + 16), Some(15));
        assert_eq!(byte_at_col(HEX_COLS + 17), None);
    }

    #[test]
    fn offsets_parse() {
        assert_eq!(parse_offset("0x1F"), Some(31));
        assert_eq!(parse_offset("1Fh"), Some(31));
        assert_eq!(parse_offset("$ff"), Some(255));
        assert_eq!(parse_offset("31"), Some(31));
        assert_eq!(parse_offset("1_000"), Some(1000));
        assert_eq!(parse_offset("beef"), Some(0xBEEF));
        assert_eq!(parse_offset("zz"), None);
        assert_eq!(hex_text(&[0xDE, 0xAD, 0x01]), "DE AD 01");
    }
}
