//! Memory readout (About, `qur doctor`, scripted `mem` steps): the physical
//! footprint of Qur's process and of the processes it started (ACP adapters,
//! terminal shells and what runs in them). Sampled on demand, never polled.
//!
//! Physical footprint is the number `footprint <pid>` and Activity Monitor's
//! Memory column show (the kernel's `phys_footprint` ledger, the same value
//! TASK_VM_INFO reports). `proc_pid_rusage` reads it for any process of this
//! user without a task port, so one call covers Qur and its children.

use std::collections::HashMap;

/// One process Qur started, with everything under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub pid: i32,
    /// What it is: an agent's or terminal's name when the caller knows the
    /// pid, otherwise the program's name.
    pub label: String,
    /// Footprint of the process and all its descendants.
    pub bytes: u64,
    /// How many processes that is.
    pub procs: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub pid: i32,
    /// Qur's own process.
    pub own: u64,
    /// Its direct children (each with its subtree), largest first.
    pub children: Vec<Child>,
}

impl Report {
    pub fn total(&self) -> u64 {
        self.own + self.children.iter().map(|c| c.bytes).sum::<u64>()
    }

    /// "Qur 74 MB · agents and terminals 312 MB (2) · total 386 MB".
    pub fn summary(&self) -> String {
        let n = self.children.len();
        if n == 0 {
            return format!("Qur {}", mb(self.own));
        }
        let kids: u64 = self.children.iter().map(|c| c.bytes).sum();
        format!("Qur {} · agents and terminals {} ({n}) · total {}", mb(self.own), mb(kids), mb(self.total()))
    }

    /// One line per child: "  Agent Terminal 1: 210 MB (3 processes)".
    pub fn lines(&self) -> Vec<String> {
        self.children
            .iter()
            .map(|c| {
                let procs = if c.procs == 1 { String::new() } else { format!(" ({} processes)", c.procs) };
                format!("{}: {}{procs}", c.label, mb(c.bytes))
            })
            .collect()
    }
}

/// Bytes as "74 MB" (MiB, as `footprint` prints them).
pub fn mb(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1_048_576.0)
}

/// A process's physical footprint, or None if it's gone or not this user's.
pub fn footprint(pid: i32) -> Option<u64> {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let got = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, (&raw mut info).cast()) };
    (got == 0).then_some(info.ri_phys_footprint)
}

fn child_pids(pid: i32) -> Vec<i32> {
    let mut buf = vec![0i32; 1024];
    let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr().cast(), (buf.len() * 4) as i32) };
    buf.truncate(n.max(0) as usize);
    buf.retain(|&p| p > 0);
    buf
}

/// `pid` and every process under it.
fn subtree(pid: i32) -> Vec<i32> {
    let mut out = vec![pid];
    let mut i = 0;
    while i < out.len() && out.len() < 4096 {
        for k in child_pids(out[i]) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
        i += 1;
    }
    out
}

/// The kernel's name for a process ("zsh", "node"), up to 32 characters.
pub fn name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// A process's arguments (KERN_PROCARGS2), for naming script runners.
fn args(pid: i32) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    let ok = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) };
    if ok != 0 || size < 4 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size];
    let ok = unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if ok != 0 {
        return Vec::new();
    }
    buf.truncate(size);
    parse_procargs(&buf)
}

/// KERN_PROCARGS2's layout: argc (i32), the executable path, NUL padding,
/// then argc NUL-terminated arguments (then the environment).
fn parse_procargs(buf: &[u8]) -> Vec<String> {
    let Some(argc) = buf.get(..4).map(|b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]])) else { return Vec::new() };
    let rest = &buf[4..];
    let Some(end) = rest.iter().position(|&b| b == 0) else { return Vec::new() };
    let start = rest[end..].iter().position(|&b| b != 0).map_or(rest.len(), |p| end + p);
    rest[start..].split(|&b| b == 0).take(argc.max(0) as usize).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

/// What a process is, for people: its program's name, or for a script
/// runner (node, bun, python) the script's ("claude-agent-acp").
pub fn program(pid: i32) -> String {
    let n = name(pid).unwrap_or_else(|| format!("pid {pid}"));
    if !matches!(n.as_str(), "node" | "bun" | "deno" | "python" | "python3" | "env") {
        return n;
    }
    let a = args(pid);
    let script = a.iter().skip(1).find(|s| !s.starts_with('-') && !s.contains('='));
    match script.and_then(|s| s.rsplit('/').next()) {
        Some(s) if !s.is_empty() => s.trim_end_matches(".js").to_string(),
        _ => n,
    }
}

/// Sample `pid` (Qur) and its children now. `labels` names the children the
/// caller knows (an agent's ACP adapter, a terminal's shell) by pid.
pub fn report(pid: i32, labels: &HashMap<i32, String>) -> Report {
    let own = footprint(pid).unwrap_or(0);
    let mut children: Vec<Child> = child_pids(pid)
        .into_iter()
        .filter_map(|c| {
            let procs = subtree(c);
            let bytes: u64 = procs.iter().filter_map(|&p| footprint(p)).sum();
            let label = labels.get(&c).cloned().unwrap_or_else(|| program(c));
            // Gone since it was listed (a short-lived git or sw_vers).
            (bytes > 0).then_some(Child { pid: c, label, bytes, procs: procs.len() })
        })
        .collect();
    children.sort_by_key(|c| std::cmp::Reverse(c.bytes));
    Report { pid, own, children }
}

/// Every running Qur except this process (for `qur doctor`).
pub fn other_qurs() -> Vec<i32> {
    let mut buf = vec![0i32; 8192];
    let n = unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast(), (buf.len() * 4) as i32) };
    buf.truncate(n.max(0) as usize);
    let me = std::process::id() as i32;
    buf.into_iter().filter(|&p| p > 0 && p != me && name(p).as_deref() == Some("qur")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_footprint() {
        let f = footprint(std::process::id() as i32).unwrap_or(0);
        assert!(f > 1_048_576, "footprint {f}");
        assert!(name(std::process::id() as i32).is_some());
    }

    #[test]
    fn counts_children() {
        let mut child = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let mut labels = HashMap::new();
        labels.insert(child.id() as i32, "a sleeper".to_string());
        let r = report(std::process::id() as i32, &labels);
        let c = r.children.iter().find(|c| c.pid == child.id() as i32).cloned();
        child.kill().ok();
        child.wait().ok();
        let c = c.expect("the child is listed");
        assert_eq!((c.label.as_str(), c.procs), ("a sleeper", 1));
        assert!(c.bytes > 0 && r.total() > r.own);
        assert!(r.summary().contains("total"));
    }

    #[test]
    fn parses_procargs() {
        let mut b = 2i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/usr/local/bin/node\0\0\0\0node\0/x/bin/claude-agent-acp\0PATH=/bin\0");
        assert_eq!(parse_procargs(&b), vec!["node".to_string(), "/x/bin/claude-agent-acp".to_string()]);
        assert!(parse_procargs(&[1, 0]).is_empty());
    }
}
