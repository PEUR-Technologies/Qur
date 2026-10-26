//! Crash reports, kept on this Mac. A panic writes `crash-<UTC time>.txt`
//! next to qur.log: Qur's and macOS's versions, the panic and where, a
//! backtrace, the agents' states and the last log lines. A fatal signal
//! (SIGSEGV, SIGBUS, SIGABRT) writes a shorter note from inside its handler.
//! The next launch offers the newest unseen report once, in the top bar.
//! Nothing is ever sent: Report a Problem opens a prefilled GitHub issue in
//! the browser that only names the file.
//!
//! Nothing runs at idle: the hook and handlers wait for a crash, and the
//! agents' states are noted only when one changes.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Reports kept (the oldest go first).
const KEEP: usize = 10;
/// Log lines a panic report ends with.
const LOG_LINES: usize = 200;
const ISSUES: &str = "https://github.com/PEUR-Technologies/Qur/issues/new";

/// Set by the first panic or fatal signal: later ones (the abort that ends a
/// panic, a panic while unwinding) add nothing.
static CRASHED: AtomicBool = AtomicBool::new(false);
/// Each ACP agent's "kind: state" by entity id, written when its state
/// changes. The panic hook only ever `try_lock`s it.
static AGENTS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());

/// The panic hook and the signal handlers; first thing in `main`.
pub fn install() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = backtrace();
        let first = !CRASHED.swap(true, Ordering::SeqCst);
        let saved = first.then(|| write_panic_report(info.payload_as_str().unwrap_or("(no message)"), info.location(), &bt)).flatten();
        // A bundled app has no terminal: qur.log has every panic (Help > Show Logs).
        match saved {
            Some(p) => crate::log(&format!("panic: {info} (crash report: {})", p.display())),
            None => crate::log(&format!("panic: {info}\n{bt}")),
        }
        default_hook(info);
    }));
    signals::install();
}

/// The panicking thread's backtrace. Release builds are stripped, so std
/// names none of Qur's frames; then each frame is given as macOS sees it:
/// its image and, for Qur's own, an offset that `atos` turns into a name
/// with an unstripped build of the same version.
fn backtrace() -> String {
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    if !bt.contains("__mh_execute_header") {
        return bt;
    }
    let mut frames = [std::ptr::null_mut(); 128];
    // SAFETY: `frames` holds 128 entries; backtrace_symbols returns one
    // malloc'd block of `n` C strings (freed below), or null.
    unsafe {
        let n = libc::backtrace(frames.as_mut_ptr(), frames.len() as libc::c_int);
        let names = libc::backtrace_symbols(frames.as_ptr(), n);
        if names.is_null() {
            return bt;
        }
        let mut out = String::from("(Qur's own frames are `qur + offset`, in decimal: `atos -o <unstripped qur of this version> -l 0x100000000 <0x100000000 + offset in hex>` names them.)\n");
        for i in 0..n.max(0) as usize {
            out.push_str(&std::ffi::CStr::from_ptr(*names.add(i)).to_string_lossy());
            out.push('\n');
        }
        libc::free(names.cast());
        out
    }
}

/// An ACP agent's state changed (`kind` is its roster label).
pub fn note_agent(id: u64, kind: &str, state: &str) {
    let mut v = AGENTS.lock().unwrap_or_else(|e| e.into_inner());
    let line = format!("{kind}: {state}");
    match v.iter_mut().find(|(i, _)| *i == id) {
        Some(e) => e.1 = line,
        None => v.push((id, line)),
    }
}

/// The agent's view is gone.
pub fn forget_agent(id: u64) {
    AGENTS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(i, _)| *i != id);
}

/// What a panic report says.
struct Report<'a> {
    version: &'a str,
    os: &'a str,
    secs: u64,
    thread: &'a str,
    message: &'a str,
    location: &'a str,
    /// None when the list was locked (a panic while it was being updated).
    agents: Option<&'a [String]>,
    backtrace: &'a str,
    log_tail: &'a str,
}

fn format_report(r: &Report) -> String {
    let agents = match r.agents {
        None => "  (not available)\n".to_string(),
        Some([]) => "  none\n".to_string(),
        Some(list) => list.iter().map(|a| format!("  {a}\n")).collect(),
    };
    format!(
        "Qur crash report\nQur {} · {} · {}\nTime: {} UTC\nThread: {}\nPanic: {}\nAt: {}\n\nAgents:\n{agents}\nBacktrace:\n{}\n\nLast log lines (qur.log):\n{}\n",
        r.version,
        r.os,
        std::env::consts::ARCH,
        date(r.secs),
        r.thread,
        r.message,
        r.location,
        r.backtrace.trim_end(),
        r.log_tail.trim_end(),
    )
}

fn write_panic_report(message: &str, location: Option<&std::panic::Location>, backtrace: &str) -> Option<PathBuf> {
    let dir = crate::log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let secs = now();
    let agents: Option<Vec<String>> = match AGENTS.try_lock() {
        Ok(v) => Some(v.iter().map(|(_, a)| a.clone()).collect()),
        Err(TryLockError::Poisoned(e)) => Some(e.into_inner().iter().map(|(_, a)| a.clone()).collect()),
        Err(TryLockError::WouldBlock) => None,
    };
    let thread = std::thread::current();
    let text = format_report(&Report {
        version: env!("CARGO_PKG_VERSION"),
        os: &os_version(),
        secs,
        thread: thread.name().unwrap_or("unnamed"),
        message,
        location: &location.map(|l| l.to_string()).unwrap_or_default(),
        agents: agents.as_deref(),
        backtrace,
        log_tail: &log_tail(&dir.join("qur.log"), LOG_LINES),
    });
    let path = create_report(&dir, secs, text.as_bytes())?;
    prune(&dir, KEEP);
    Some(path)
}

/// A new report file; a second one in the same second (two Qur processes) gets the pid.
fn create_report(dir: &Path, secs: u64, text: &[u8]) -> Option<PathBuf> {
    use std::io::Write;
    let stamp = stamp(secs);
    for name in [format!("crash-{stamp}.txt"), format!("crash-{stamp}-{}.txt", std::process::id())] {
        let path = dir.join(name);
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            return f.write_all(text).is_ok().then_some(path);
        }
    }
    None
}

/// The last `n` lines of a log file (it reads at most its last 64 KB).
fn log_tail(path: &Path, n: usize) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return "(no log)".into() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(64 << 10);
    let mut bytes = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut bytes).is_err() {
        return "(log unreadable)".into();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    // Reading from the middle starts in the middle of a line.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Crash reports in `dir`, newest first (their names sort by time).
fn reports(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".txt")))
        .collect();
    v.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    v
}

/// Keep the newest `keep` reports.
fn prune(dir: &Path, keep: usize) {
    for p in reports(dir).into_iter().skip(keep) {
        std::fs::remove_file(p).ok();
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The newest report if it's newer than the one last seen (`state.toml`).
fn unseen(dir: &Path, seen: &str) -> Option<PathBuf> {
    reports(dir).into_iter().next().filter(|p| name_of(p).as_str() > seen)
}

/// The newest report the user hasn't been told about. Only the first
/// window that asks gets it; this also drops reports beyond the newest ten.
pub fn take_unseen() -> Option<PathBuf> {
    static ASKED: AtomicBool = AtomicBool::new(false);
    if ASKED.swap(true, Ordering::SeqCst) {
        return None;
    }
    let dir = crate::log_dir();
    prune(&dir, KEEP);
    unseen(&dir, &qur_config::State::load().crash_seen)
}

/// The user has seen `report` (View Report, Report a Problem, Dismiss).
pub fn acknowledge(report: &Path) {
    let mut state = qur_config::State::load();
    let name = name_of(report);
    if name > state.crash_seen {
        state.crash_seen = name;
        state.save().ok();
    }
}

/// The newest report from the last week, for Report a Problem.
pub fn recent() -> Option<PathBuf> {
    let p = reports(&crate::log_dir()).into_iter().next()?;
    let age = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?.elapsed().unwrap_or_default();
    (age < Duration::from_secs(7 * 86_400)).then_some(p)
}

/// A new GitHub issue, prefilled with the versions and, given a crash
/// report, its name and what it says happened (the file stays local; the
/// user attaches it if they want to). Opened in the browser: Qur itself
/// makes no network call.
pub fn issue_url(report: Option<&Path>) -> String {
    let mut body = format!(
        "**What happened**\n\n\n**Steps to reproduce**\n\n\n---\nQur {} · {} · {}\n",
        env!("CARGO_PKG_VERSION"),
        os_version(),
        std::env::consts::ARCH
    );
    let mut title = String::new();
    if let Some(p) = report {
        let text = std::fs::read_to_string(p).unwrap_or_default();
        let summary: Vec<&str> = text.lines().filter(|l| ["Panic: ", "At: ", "Signal: ", "Time: "].iter().any(|k| l.starts_with(k))).collect();
        body.push_str(&format!(
            "\nCrash report: `{}` (in ~/Library/Logs/Qur; Help > Show Logs in Finder). Attach it here if you're happy to share it.\n```\n{}\n```\n",
            name_of(p),
            summary.join("\n")
        ));
        if let Some(what) = summary.iter().find_map(|l| l.strip_prefix("Panic: ").or_else(|| l.strip_prefix("Signal: "))) {
            title = format!("Crash: {}", what.chars().take(80).collect::<String>());
        }
    }
    let mut url = format!("{ISSUES}?body={}", encode(&body));
    if !title.is_empty() {
        url.push_str(&format!("&title={}", encode(&title)));
    }
    url
}

/// Percent-encoding for a URL query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// "macOS 26.1 (25B78)", from sysctl (no process spawned).
pub fn os_version() -> String {
    fn sysctl(name: &std::ffi::CStr) -> Option<String> {
        let mut buf = [0u8; 64];
        let mut len = buf.len();
        // SAFETY: `buf` and `len` describe a writable buffer; no new value is set.
        let r = unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
        (r == 0).then(|| String::from_utf8_lossy(&buf[..len.min(buf.len())]).trim_end_matches('\0').to_string())
    }
    let v = sysctl(c"kern.osproductversion").unwrap_or_else(|| "?".into());
    match sysctl(c"kern.osversion") {
        Some(b) => format!("macOS {v} ({b})"),
        None => format!("macOS {v}"),
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Seconds since 1970 as UTC [year, month, day, hour, minute, second].
/// Plain arithmetic (no allocation, no locks), so the signal handler uses it too.
const fn utc(secs: u64) -> [u64; 6] {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days, for days >= 0.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    [y, m, d, rem / 3_600, rem % 3_600 / 60, rem % 60]
}

/// "20261026-153012": report names sort by time.
fn stamp(secs: u64) -> String {
    let [y, mo, d, h, mi, s] = utc(secs);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// "2026-10-26 15:30:12".
fn date(secs: u64) -> String {
    let [y, mo, d, h, mi, s] = utc(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Dev/test: `QUR_DEBUG_PANIC=<how>[@<ms>]` crashes Qur once its first
/// window has drawn (after `ms`): `1` or `panic` panics on the main thread,
/// `thread` in a background thread (Qur keeps running), `segv` and `abort`
/// raise those signals.
pub fn debug_crash(cx: &gpui::App) {
    let Ok(v) = std::env::var("QUR_DEBUG_PANIC") else { return };
    let (how, ms) = match v.split_once('@') {
        Some((h, ms)) => (h.to_string(), ms.parse().unwrap_or(0)),
        None => (v, 0),
    };
    let bg = cx.background_executor().clone();
    cx.spawn(async move |_| {
        bg.timer(Duration::from_millis(ms)).await;
        crate::log(&format!("QUR_DEBUG_PANIC={how}"));
        match how.as_str() {
            "thread" => drop(std::thread::Builder::new().name("qur-debug".into()).spawn(|| panic!("QUR_DEBUG_PANIC in a background thread"))),
            // SAFETY: raising a signal on purpose, to test the handlers.
            "segv" => {
                let _ = unsafe { libc::raise(libc::SIGSEGV) };
            }
            "abort" => std::process::abort(),
            _ => panic!("QUR_DEBUG_PANIC on the main thread"),
        }
    })
    .detach();
}

/// Fatal signals. A signal handler may only call async-signal-safe functions
/// (no allocating, no locks, no Rust formatting), so its note is written with
/// `open`/`write`/`read`/`close` from buffers prepared at startup, and it
/// takes no backtrace: macOS's own crash reporter saves a full one
/// (~/Library/Logs/DiagnosticReports) once the process dies of the signal,
/// so the handler puts the previous handler back and lets the signal finish
/// the process. Rust's own handler (stack overflow) stays chained behind it.
mod signals {
    use std::ffi::{CString, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;

    use super::{CRASHED, utc};

    const SIGNALS: [c_int; 3] = [libc::SIGSEGV, libc::SIGBUS, libc::SIGABRT];
    /// `si_code` of a signal sent by `kill`/`raise` (<sys/signal.h>; not in the libc crate for macOS).
    const SI_USER: c_int = 0x10001;
    /// Bytes of qur.log a signal note ends with (about 200 lines).
    const TAIL: i64 = 16 << 10;
    const EXPLAIN: &[u8] = b"Qur stopped on a fatal signal. A backtrace can't be taken safely inside a signal\n\
handler; macOS keeps a full crash report in ~/Library/Logs/DiagnosticReports (qur-*.ips,\n\
also in Console.app under Crash Reports).\n\nLast log lines (qur.log):\n";

    struct Note {
        /// The logs folder, without a trailing NUL.
        dir: Vec<u8>,
        log: CString,
        header: Vec<u8>,
    }

    static NOTE: OnceLock<Note> = OnceLock::new();
    static OLD: OnceLock<[libc::sigaction; 3]> = OnceLock::new();

    pub(super) fn install() {
        let dir = crate::log_dir();
        let Ok(log) = CString::new(dir.join("qur.log").as_os_str().as_bytes()) else { return };
        let header = format!("Qur crash report (fatal signal)\nQur {} · {} · {}\n", env!("CARGO_PKG_VERSION"), super::os_version(), std::env::consts::ARCH);
        if NOTE.set(Note { dir: dir.as_os_str().as_bytes().to_vec(), log, header: header.into_bytes() }).is_err() {
            return;
        }
        // SAFETY: plain sigaction calls with zeroed, then filled, structs.
        unsafe {
            let mut old: [libc::sigaction; 3] = std::mem::zeroed();
            for (i, &sig) in SIGNALS.iter().enumerate() {
                libc::sigaction(sig, std::ptr::null(), &mut old[i]);
            }
            if OLD.set(old).is_err() {
                return;
            }
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void) as libc::sighandler_t;
            // SA_ONSTACK: on a stack overflow, run on the alternate stack Rust sets up.
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut sa.sa_mask);
            for &sig in &SIGNALS {
                libc::sigaction(sig, &sa, std::ptr::null_mut());
            }
        }
    }

    extern "C" fn on_signal(sig: c_int, info: *mut libc::siginfo_t, _: *mut c_void) {
        if !CRASHED.swap(true, Ordering::SeqCst)
            && let Some(note) = NOTE.get()
        {
            // SAFETY: async-signal-safe calls on buffers owned by this frame or `note`.
            unsafe { write_note(note, sig) };
        }
        let i = SIGNALS.iter().position(|&s| s == sig).unwrap_or(0);
        // SAFETY: sigaction, signal and raise are async-signal-safe; `info` is the kernel's.
        unsafe {
            match OLD.get() {
                Some(old) => libc::sigaction(sig, &old[i], std::ptr::null_mut()),
                None => {
                    libc::signal(sig, libc::SIG_DFL);
                    0
                }
            };
            // A fault happens again on return, now to the previous handler; a
            // sent signal (kill, raise, abort) has to be sent again.
            if sig == libc::SIGABRT || info.is_null() || (*info).si_code == SI_USER {
                libc::raise(sig);
            }
        }
    }

    /// A fixed buffer to build bytes in without allocating.
    struct Buf<const N: usize> {
        b: [u8; N],
        len: usize,
    }

    impl<const N: usize> Buf<N> {
        fn new() -> Self {
            Buf { b: [0; N], len: 0 }
        }
        fn push(&mut self, s: &[u8]) {
            let n = s.len().min(N - self.len);
            self.b[self.len..self.len + n].copy_from_slice(&s[..n]);
            self.len += n;
        }
        fn num(&mut self, mut v: u64, width: usize) {
            let mut d = [b'0'; 20];
            let mut i = d.len();
            while v > 0 || d.len() - i < width {
                i -= 1;
                d[i] = b'0' + (v % 10) as u8;
                v /= 10;
                if i == 0 {
                    break;
                }
            }
            self.push(&d[i..]);
        }
        /// "20261026-153012" or "2026-10-26 15:30:12".
        fn time(&mut self, secs: u64, stamp: bool) {
            let [y, mo, d, h, mi, s] = utc(secs);
            let (date_sep, mid, time_sep): (&[u8], &[u8], &[u8]) = if stamp { (b"", b"-", b"") } else { (b"-", b" ", b":") };
            self.num(y, 4);
            self.push(date_sep);
            self.num(mo, 2);
            self.push(date_sep);
            self.num(d, 2);
            self.push(mid);
            self.num(h, 2);
            self.push(time_sep);
            self.num(mi, 2);
            self.push(time_sep);
            self.num(s, 2);
        }
        fn bytes(&self) -> &[u8] {
            &self.b[..self.len]
        }
    }

    fn name(sig: c_int) -> &'static [u8] {
        match sig {
            libc::SIGSEGV => b"SIGSEGV (bad memory access)",
            libc::SIGBUS => b"SIGBUS (bad memory access)",
            libc::SIGABRT => b"SIGABRT (abort)",
            _ => b"unknown",
        }
    }

    unsafe fn write_all(fd: c_int, mut s: &[u8]) {
        while !s.is_empty() {
            let n = unsafe { libc::write(fd, s.as_ptr().cast(), s.len()) };
            if n <= 0 {
                return;
            }
            s = &s[n as usize..];
        }
    }

    unsafe fn write_note(note: &Note, sig: c_int) {
        let secs = unsafe { libc::time(std::ptr::null_mut()) }.max(0) as u64;
        let mut path = Buf::<1024>::new();
        path.push(&note.dir);
        path.push(b"/crash-");
        path.time(secs, true);
        path.push(b".txt\0");
        if path.len == 1024 {
            return;
        }
        let fd = unsafe { libc::open(path.b.as_ptr().cast(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC, 0o644 as libc::c_uint) };
        if fd < 0 {
            return;
        }
        let mut head = Buf::<256>::new();
        head.push(b"Time: ");
        head.time(secs, false);
        head.push(b" UTC\nSignal: ");
        head.push(name(sig));
        head.push(b"\n\n");
        unsafe {
            write_all(fd, &note.header);
            write_all(fd, head.bytes());
            write_all(fd, EXPLAIN);
            let log = libc::open(note.log.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            if log >= 0 {
                let end = libc::lseek(log, 0, libc::SEEK_END);
                let start = (end - TAIL).max(0);
                libc::lseek(log, start, libc::SEEK_SET);
                let mut buf = [0u8; 4096];
                // Reading from the middle starts in the middle of a line.
                let mut skip = start > 0;
                loop {
                    let n = libc::read(log, buf.as_mut_ptr().cast(), buf.len());
                    if n <= 0 {
                        break;
                    }
                    let mut chunk = &buf[..n as usize];
                    if skip {
                        match chunk.iter().position(|&c| c == b'\n') {
                            Some(p) => {
                                chunk = &chunk[p + 1..];
                                skip = false;
                            }
                            None => continue,
                        }
                    }
                    write_all(fd, chunk);
                }
                libc::close(log);
            }
            libc::close(fd);
        }
    }

    #[cfg(test)]
    pub(super) fn time_bytes(secs: u64, stamp: bool) -> Vec<u8> {
        let mut b = Buf::<32>::new();
        b.time(secs, stamp);
        b.bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-crash-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn utc_dates() {
        assert_eq!(date(0), "1970-01-01 00:00:00");
        assert_eq!(date(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(date(1_000_000_000), "2001-09-09 01:46:40");
        assert_eq!(stamp(1_793_028_612), "20261026-153012");
        // The signal handler's own formatting agrees.
        for secs in [0, 951_782_400, 1_000_000_000, 1_793_028_612, 4_102_444_799] {
            assert_eq!(signals::time_bytes(secs, true), stamp(secs).into_bytes());
            assert_eq!(signals::time_bytes(secs, false), date(secs).into_bytes());
        }
    }

    #[test]
    fn report_lists_everything() {
        let agents = vec!["Claude: Working".to_string(), "Codex: NeedsYou".to_string()];
        let r = Report {
            version: "0.1.5",
            os: "macOS 26.1 (25B78)",
            secs: 1_793_028_612,
            thread: "main",
            message: "index out of bounds",
            location: "crates/qur_app/src/dock.rs:10:5",
            agents: Some(&agents),
            backtrace: "   0: std::backtrace\n",
            log_tail: "[1] start: Qur 0.1.5\n[2] ACTION qur::Quit\n",
        };
        let text = format_report(&r);
        assert!(text.starts_with("Qur crash report\nQur 0.1.5 · macOS 26.1 (25B78) · "));
        for line in [
            "Time: 2026-10-26 15:30:12 UTC",
            "Thread: main",
            "Panic: index out of bounds",
            "At: crates/qur_app/src/dock.rs:10:5",
            "  Claude: Working",
            "  Codex: NeedsYou",
            "   0: std::backtrace",
            "[2] ACTION qur::Quit",
        ] {
            assert!(text.lines().any(|l| l == line), "missing {line:?} in\n{text}");
        }
        assert!(format_report(&Report { agents: Some(&[]), ..r }).contains("Agents:\n  none\n"));
        assert!(format_report(&Report { agents: None, ..r }).contains("Agents:\n  (not available)\n"));
    }

    #[test]
    fn keeps_the_newest_ten() {
        let d = tmp("prune");
        for i in 0..13u64 {
            create_report(&d, 1_793_028_612 + i * 61, b"x").unwrap();
        }
        std::fs::write(d.join("qur.log"), "log").unwrap();
        prune(&d, KEEP);
        let left = reports(&d);
        assert_eq!(left.len(), 10);
        assert_eq!(name_of(&left[0]), format!("crash-{}.txt", stamp(1_793_028_612 + 12 * 61)));
        assert_eq!(name_of(&left[9]), format!("crash-{}.txt", stamp(1_793_028_612 + 3 * 61)));
        assert!(d.join("qur.log").exists());
        // The same second twice: the second report gets the pid.
        let again = create_report(&d, 1_793_028_612 + 12 * 61, b"y").unwrap();
        assert!(name_of(&again).ends_with(&format!("-{}.txt", std::process::id())));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unseen_is_newer_than_seen() {
        let d = tmp("unseen");
        assert_eq!(unseen(&d, ""), None);
        let a = create_report(&d, 1_793_028_612, b"a").unwrap();
        assert_eq!(unseen(&d, ""), Some(a.clone()));
        assert_eq!(unseen(&d, &name_of(&a)), None);
        let b = create_report(&d, 1_793_028_700, b"b").unwrap();
        assert_eq!(unseen(&d, &name_of(&a)), Some(b));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn log_tail_takes_the_last_lines() {
        let d = tmp("tail");
        let log = d.join("qur.log");
        let text: String = (0..5_000).map(|i| format!("[{i}] line {i}\n")).collect();
        std::fs::write(&log, text).unwrap();
        let tail = log_tail(&log, 200);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 200);
        assert_eq!(lines[0], "[4800] line 4800");
        assert_eq!(lines[199], "[4999] line 4999");
        assert_eq!(log_tail(&d.join("missing.log"), 5), "(no log)");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn issue_names_the_report() {
        let d = tmp("issue");
        let p = d.join("crash-20261026-153012.txt");
        std::fs::write(&p, "Qur crash report\nTime: 2026-10-26 15:30:12 UTC\nPanic: oh no & more\nAt: a.rs:1:2\n\nBacktrace:\nsecret frames\n").unwrap();
        let url = issue_url(Some(&p));
        assert!(url.starts_with("https://github.com/PEUR-Technologies/Qur/issues/new?body="));
        assert!(url.contains(&encode("crash-20261026-153012.txt")));
        assert!(url.contains(&encode("Panic: oh no & more")));
        assert!(url.ends_with(&format!("&title={}", encode("Crash: oh no & more"))));
        // Only the summary lines go in; the backtrace stays in the file.
        assert!(!url.contains("secret"));
        assert!(!issue_url(None).contains("title="));
        assert_eq!(encode("a b&c/é"), "a%20b%26c%2F%C3%A9");
        std::fs::remove_dir_all(&d).ok();
    }
}
