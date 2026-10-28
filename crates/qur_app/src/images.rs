//! Inline images in terminals: iTerm2's `ESC ] 1337 ; File=…:<base64> BEL`,
//! as `imgcat` prints. alacritty_terminal has no images, so the PTY's bytes
//! pass through `Scan` first: an image becomes rows of blank cells tagged with
//! an OSC 8 link `qurimg:<id>:<row>`, and the grid paints the image over them.
//! The image scrolls, clears and is overwritten exactly as text is.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite};
use base64::Engine;
use polling::{Event, PollMode, Poller};

/// What an image's size in cells is worked out from: a pane's cell size in
/// points, its width in columns, and the screen's pixels per point.
#[derive(Clone, Copy)]
pub struct Geo {
    pub cw: f32,
    pub lh: f32,
    pub cols: usize,
    pub scale: f32,
}

pub type GeoCell = Arc<Mutex<Geo>>;

pub fn geo_cell() -> GeoCell {
    Arc::new(Mutex::new(Geo { cw: 8., lh: 16., cols: 80, scale: 2. }))
}

pub struct Image {
    pub data: Arc<gpui::RenderImage>,
    pub cols: usize,
    pub rows: usize,
}

/// Images still in some scrollback. shortcut: the newest 64 are kept, an older
/// one turns into blank space; count references from the grid if that bites.
type Store = (u32, VecDeque<(u32, Arc<Image>)>);
static STORE: Mutex<Store> = Mutex::new((0, VecDeque::new()));
const KEEP: usize = 64;

pub fn get(id: u32) -> Option<Arc<Image>> {
    STORE.lock().unwrap_or_else(|e| e.into_inner()).1.iter().find(|(i, _)| *i == id).map(|(_, img)| img.clone())
}

fn put(img: Image) -> u32 {
    let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
    st.0 = st.0.wrapping_add(1);
    let id = st.0;
    st.1.push_back((id, Arc::new(img)));
    if st.1.len() > KEEP {
        st.1.pop_front();
    }
    id
}

/// The link a cell of image `id`'s `row` carries, read back by `tag`.
pub fn tag(uri: &str) -> Option<(u32, usize)> {
    let (id, row) = uri.strip_prefix("qurimg:")?.split_once(':')?;
    Some((id.parse().ok()?, row.parse().ok()?))
}

/// The PTY with its reads passed through `Scan`.
pub struct ImgPty {
    pty: tty::Pty,
    reader: Scan,
}

impl ImgPty {
    pub fn new(mut pty: tty::Pty, geo: GeoCell) -> io::Result<ImgPty> {
        let file = pty.reader().try_clone()?;
        Ok(ImgPty { pty, reader: Scan { file, geo, state: State::Text, held: Vec::new(), out: VecDeque::new() } })
    }
}

impl EventedReadWrite for ImgPty {
    type Reader = Scan;
    type Writer = File;

    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        unsafe { self.pty.register(poll, interest, mode) }
    }
    fn reregister(&mut self, poll: &Arc<Poller>, interest: Event, mode: PollMode) -> io::Result<()> {
        self.pty.reregister(poll, interest, mode)
    }
    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }
    fn reader(&mut self) -> &mut Scan {
        &mut self.reader
    }
    fn writer(&mut self) -> &mut File {
        self.pty.writer()
    }
}

impl EventedPty for ImgPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl OnResize for ImgPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.pty.on_resize(size)
    }
}

const START: &[u8] = b"\x1b]1337;File=";
/// Bigger images are dropped (base64 text, about 48 MB of image).
const MAX: usize = 64 << 20;

enum State {
    Text,
    /// `held` is a prefix of START so far.
    Start,
    /// Inside the image's escape; `held` has what follows START.
    Body,
    /// Body, after an ESC (ST is ESC \).
    BodyEsc,
}

pub struct Scan {
    file: File,
    geo: GeoCell,
    state: State,
    held: Vec<u8>,
    out: VecDeque<u8>,
}

impl Read for Scan {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut raw = [0u8; 0x4000];
        while self.out.is_empty() {
            let n = self.file.read(&mut raw)?;
            if n == 0 {
                return Ok(0);
            }
            let geo = *self.geo.lock().unwrap_or_else(|e| e.into_inner());
            for &b in &raw[..n] {
                self.feed(b, geo);
            }
        }
        let n = buf.len().min(self.out.len());
        for (dst, src) in buf.iter_mut().zip(self.out.drain(..n)) {
            *dst = src;
        }
        Ok(n)
    }
}

impl Scan {
    fn feed(&mut self, b: u8, geo: Geo) {
        match self.state {
            State::Text if b == 0x1b => {
                self.held.push(b);
                self.state = State::Start;
            }
            State::Text => self.out.push_back(b),
            State::Start => {
                self.held.push(b);
                if self.held.len() == START.len() && self.held == START {
                    self.held.clear();
                    self.state = State::Body;
                } else if !START.starts_with(&self.held) {
                    // Not ours: pass it on, and look at this byte again (it may start one).
                    self.held.pop();
                    self.out.extend(self.held.drain(..));
                    self.state = State::Text;
                    self.feed(b, geo);
                }
            }
            State::Body | State::BodyEsc if b == 0x07 || (matches!(self.state, State::BodyEsc) && b == b'\\') => {
                let body = std::mem::take(&mut self.held);
                self.state = State::Text;
                if let Some(cells) = place(&body, geo) {
                    self.out.extend(cells);
                }
            }
            State::Body | State::BodyEsc => {
                self.state = if b == 0x1b { State::BodyEsc } else { State::Body };
                if self.held.len() < MAX {
                    if b != 0x1b {
                        self.held.push(b);
                    }
                } else {
                    // Too big: drop it, keep reading past it.
                    self.held.clear();
                    self.held.shrink_to_fit();
                }
            }
        }
    }
}

/// `args:base64` to the bytes that leave room for it: blank rows tagged with the image.
fn place(body: &[u8], geo: Geo) -> Option<Vec<u8>> {
    let colon = body.iter().position(|&c| c == b':')?;
    let args = String::from_utf8_lossy(&body[..colon]);
    let arg = |k: &str| args.split(';').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('=')).map(str::to_string);
    // inline=0 (the default) is a download in iTerm2; Qur only shows images.
    if arg("inline").as_deref() != Some("1") {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(body[colon + 1..].iter().copied().filter(|c| !c.is_ascii_whitespace()).collect::<Vec<u8>>()).ok()?;
    let (iw, ih) = image::ImageReader::new(io::Cursor::new(&bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
    // In points: an image pixel is a screen pixel, as in iTerm2.
    let (pw, ph) = (iw as f32 / geo.scale, ih as f32 / geo.scale);
    let span = |v: Option<String>, cell: f32, full: f32, natural: f32| -> Option<f32> {
        let v = v.filter(|v| v != "auto")?;
        Some(if let Some(p) = v.strip_suffix("px") {
            p.parse::<f32>().ok()? / geo.scale
        } else if let Some(p) = v.strip_suffix('%') {
            full * p.parse::<f32>().ok()? / 100.
        } else {
            v.parse::<f32>().ok()? * cell
        })
        .map(|x: f32| x.max(1.).min(natural.max(1.) * 64.))
    };
    let full_w = geo.cw * geo.cols as f32;
    let (w, h) = match (span(arg("width"), geo.cw, full_w, pw), span(arg("height"), geo.lh, geo.lh * 1000., ph)) {
        (Some(w), Some(h)) if arg("preserveAspectRatio").as_deref() == Some("0") => (w, h),
        (Some(w), Some(h)) => {
            let k = (w / pw).min(h / ph);
            (pw * k, ph * k)
        }
        (Some(w), None) => (w, ph * w / pw),
        (None, Some(h)) => (pw * h / ph, h),
        (None, None) => (pw, ph),
    };
    // Never wider than the pane.
    let k = (full_w / w).min(1.);
    let (w, h) = (w * k, h * k);
    let cols = ((w / geo.cw).ceil() as usize).clamp(1, geo.cols.max(1));
    let rows = ((h / geo.lh).ceil() as usize).max(1);
    let data = crate::files::thumbnail(&bytes, (w * geo.scale).ceil() as u32, (h * geo.scale).ceil() as u32)?;
    let id = put(Image { data, cols, rows });
    let mut out = Vec::new();
    for row in 0..rows {
        out.extend(format!("\x1b]8;;qurimg:{id}:{row}\x1b\\").bytes());
        out.extend(std::iter::repeat_n(b' ', cols));
        out.extend(b"\x1b]8;;\x1b\\\r\n");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(input: &[u8]) -> Vec<u8> {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(b);
        let file = File::from(std::os::fd::OwnedFd::from(a));
        let mut s = Scan { file, geo: geo_cell(), state: State::Text, held: Vec::new(), out: VecDeque::new() };
        let geo = Geo { cw: 8., lh: 16., cols: 80, scale: 1. };
        for &b in input {
            s.feed(b, geo);
        }
        s.out.into_iter().collect()
    }

    #[test]
    fn other_escapes_pass_through() {
        let text = b"a\x1b]0;title\x07b\x1b[1mc\x1b]133;A\x1b\\";
        assert_eq!(scan(text), text);
    }

    #[test]
    fn image_becomes_tagged_rows() {
        // A 16x32 PNG at 1 pixel per point: 2 columns, 2 rows of 8x16 cells.
        let mut png = Vec::new();
        image::RgbaImage::new(16, 32).write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let input = [b"x".as_slice(), START, b"inline=1:", b64.as_bytes(), b"\x07y"].concat();
        let out = String::from_utf8(scan(&input)).unwrap();
        assert!(out.starts_with('x') && out.ends_with('y'), "{out:?}");
        let rows: Vec<_> = out.match_indices("qurimg:").collect();
        assert_eq!(rows.len(), 2);
        assert!(out.contains(":0\x1b\\  \x1b]8;;\x1b\\\r\n"));
        // Not inline: dropped.
        let input = [START, b"name=eA==:", b64.as_bytes(), b"\x1b\\z"].concat();
        assert_eq!(scan(&input), b"z");
    }

    #[test]
    fn tags_read_back() {
        assert_eq!(tag("qurimg:7:3"), Some((7, 3)));
        assert_eq!(tag("https://x"), None);
    }
}
