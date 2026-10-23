//! Agent task worktrees (handoff section 10). Every task gets its own git
//! worktree **outside** the repo, under `<qur home>/worktrees/<project-id>/`,
//! on branch `qur/<agent>/<slug>`, based on the main checkout's HEAD.
//! Uses the git CLI (no libgit2); everything here blocks, so call it off the UI thread.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn is_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() == Ok("true")
}

/// "Fix the navbar on mobile!" -> "fix-the-navbar-on-mobile". ASCII only,
/// at most 40 chars, cut at a word boundary; "task" when nothing is left.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    for word in title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
    {
        let extra = if out.is_empty() { word.len() } else { word.len() + 1 };
        if out.len() + extra > 40 {
            if out.is_empty() {
                out = word[..40].to_string();
            }
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&word);
    }
    if out.is_empty() { "task".into() } else { out }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    pub branch: String,
    /// The commit the task started from (for Changes in M4).
    pub base: String,
    /// The slug actually used (made unique with -2, -3, ...).
    pub slug: String,
}

/// Create `qur/<agent>/<slug>` in `<dir>/<slug>`, from the repo's HEAD.
/// A taken slug (branch or folder) gets -2, -3, ...
pub fn create(root: &Path, dir: &Path, agent: &str, slug: &str) -> Result<Created, String> {
    if !is_repo(root) {
        return Err("Agent tasks need a git repo (git init, then commit once)".into());
    }
    let base = git(root, &["rev-parse", "HEAD"]).map_err(|_| "The repo has no commits yet; commit once first".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut n = 1;
    loop {
        let s = if n == 1 { slug.to_string() } else { format!("{slug}-{n}") };
        let branch = format!("qur/{agent}/{s}");
        let path = dir.join(&s);
        let taken = path.exists() || git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok();
        if !taken {
            let p = path.to_str().ok_or("non-UTF-8 path")?;
            git(root, &["worktree", "add", "-q", "-b", &branch, p, &base])?;
            return Ok(Created { path, branch, base, slug: s });
        }
        n += 1;
        if n > 999 {
            return Err("too many tasks with the same name".into());
        }
    }
}

/// Remove a task's worktree and delete its branch (Discard). `force` also
/// drops uncommitted changes in the worktree.
pub fn remove(root: &Path, path: &Path, branch: &str, force: bool) -> Result<(), String> {
    if path.exists() {
        let p = path.to_str().ok_or("non-UTF-8 path")?;
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(p);
        git(root, &args)?;
    } else {
        // Folder already gone: let git forget it.
        let _ = git(root, &["worktree", "prune"]);
    }
    git(root, &["branch", if force { "-D" } else { "-d" }, branch]).map(|_| ())
}

/// Copy what the repo's `.worktreeinclude` lists into a new worktree
/// (decision 22, from Orca). One path per line, relative to the repo root;
/// `#` comments; `*`/`?` globs match within the last path component (e.g.
/// `apps/*/.env`). Typical: `.env`, `node_modules`. Copies use APFS
/// copy-on-write (`cp -c`), so even `node_modules` is instant and free on
/// disk; falls back to a plain copy on other file systems. Existing files in
/// the worktree are left alone. Returns the relative paths copied.
pub fn copy_includes(root: &Path, worktree: &Path) -> Result<Vec<PathBuf>, String> {
    let Ok(list) = std::fs::read_to_string(root.join(".worktreeinclude")) else { return Ok(Vec::new()) };
    let mut copied = Vec::new();
    for line in list.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let pat = line.trim_start_matches("./").trim_end_matches('/');
        if pat.split('/').any(|c| c == "..") {
            continue; // never reach outside the repo
        }
        for rel in expand(root, pat) {
            let (src, dst) = (root.join(&rel), worktree.join(&rel));
            if !src.exists() || dst.exists() {
                continue;
            }
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let ok = Command::new("cp").args(["-cRp"]).arg(&src).arg(&dst).status().map(|s| s.success()).unwrap_or(false)
                || Command::new("cp").args(["-Rp"]).arg(&src).arg(&dst).status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                return Err(format!("couldn't copy {} into the worktree", rel.display()));
            }
            copied.push(rel);
        }
    }
    Ok(copied)
}

/// Relative paths matching `pat` (globs only in the last component).
fn expand(root: &Path, pat: &str) -> Vec<PathBuf> {
    let (dir, last) = match pat.rsplit_once('/') {
        Some((d, l)) => (d, l),
        None => ("", pat),
    };
    let dirs: Vec<PathBuf> = if dir.contains(['*', '?']) {
        // One level of glob in the folder part (apps/*/.env).
        let (pre, mid) = dir.rsplit_once('/').unwrap_or(("", dir));
        std::fs::read_dir(root.join(pre))
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && glob_match(mid, &e.file_name().to_string_lossy()))
                    .map(|e| Path::new(pre).join(e.file_name()))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![PathBuf::from(dir)]
    };
    let mut out = Vec::new();
    for d in dirs {
        if last.contains(['*', '?']) {
            if let Ok(rd) = std::fs::read_dir(root.join(&d)) {
                for e in rd.filter_map(Result::ok) {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if glob_match(last, &name) {
                        out.push(d.join(name));
                    }
                }
            }
        } else {
            out.push(d.join(last));
        }
    }
    out
}

fn glob_match(pat: &str, s: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], s) || (!s.is_empty() && go(p, &s[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &s[1..]),
            _ => false,
        }
    }
    go(&pat.chars().collect::<Vec<_>>(), &s.chars().collect::<Vec<_>>())
}

/// Run the project's setup command in a new worktree through the login shell
/// (decision 22). Returns (success, last ~40 lines of combined output).
pub fn run_setup(worktree: &Path, command: &str, path_env: &str) -> (bool, String) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = Command::new(shell)
        .args(["-l", "-c", &format!("{command} 2>&1")])
        .current_dir(worktree)
        .env("PATH", path_env)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            let lines: Vec<&str> = text.lines().collect();
            let tail = lines[lines.len().saturating_sub(40)..].join("\n");
            (o.status.success(), tail)
        }
        Err(e) => (false, format!("couldn't run setup: {e}")),
    }
}

/// Run the project's turn-end check (e.g. `cargo check`) in a worktree
/// through the login shell. Returns (passed, last ~60 lines of combined
/// output). After `timeout` the whole process group is killed and the check
/// fails.
pub fn run_check(worktree: &Path, command: &str, path_env: &str, timeout: std::time::Duration) -> (bool, String) {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let child = Command::new(shell)
        .args(["-l", "-c", &format!("{command} 2>&1")])
        .current_dir(worktree)
        .env("PATH", path_env)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return (false, format!("couldn't run the check: {e}")),
    };
    // Read on a thread so a chatty check can't fill the pipe and stall.
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stdout.as_mut() {
            s.read_to_end(&mut buf).ok();
        }
        buf
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() >= timeout => {
                let group = format!("-{}", child.id());
                Command::new("/bin/kill").args(["-KILL", "--", &group]).status().ok();
                child.wait().ok();
                break None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => break None,
        }
    };
    let text = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    let mut tail = lines[lines.len().saturating_sub(60)..].join("\n");
    match status {
        Some(s) => (s.success(), tail),
        None => {
            tail.push_str(&format!("\n(check stopped after {} s)", timeout.as_secs()));
            (false, tail)
        }
    }
}

/// True when the worktree has uncommitted changes (or untracked files).
pub fn is_dirty(path: &Path) -> bool {
    git(path, &["status", "--porcelain"]).map(|s| !s.is_empty()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-wt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(d.join("a.txt"), "a\n").unwrap();
        git(&d, &["add", "-A"]).unwrap();
        git(&d, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]).unwrap();
        d
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Fix the navbar on mobile!"), "fix-the-navbar-on-mobile");
        assert_eq!(slugify("  ¿¡ "), "task");
        assert_eq!(slugify("add tests for the very long and winding description please"), "add-tests-for-the-very-long-and-winding");
        assert!(slugify(&"x".repeat(100)).len() == 40);
    }

    #[test]
    fn create_unique_and_remove() {
        let r = repo("cr");
        let dir = r.with_extension("wt");
        let a = create(&r, &dir, "claude", "fix-nav").unwrap();
        assert_eq!(a.branch, "qur/claude/fix-nav");
        assert!(a.path.join("a.txt").exists());
        assert!(!a.path.starts_with(&r), "worktree must live outside the repo");
        let b = create(&r, &dir, "claude", "fix-nav").unwrap();
        assert_eq!((b.slug.as_str(), b.branch.as_str()), ("fix-nav-2", "qur/claude/fix-nav-2"));
        std::fs::write(b.path.join("new.txt"), "x").unwrap();
        assert!(is_dirty(&b.path));
        assert!(remove(&r, &b.path, &b.branch, false).is_err(), "dirty worktree needs force");
        remove(&r, &b.path, &b.branch, true).unwrap();
        remove(&r, &a.path, &a.branch, true).unwrap();
        assert_eq!(git(&r, &["worktree", "list"]).unwrap().lines().count(), 1);
        assert!(git(&r, &["branch", "--list", "qur/*"]).unwrap().is_empty());
    }

    #[test]
    fn worktreeinclude_copies_ignored_files() {
        let r = repo("inc");
        std::fs::write(r.join(".gitignore"), ".env\nnode_modules/\napps/*/.env\n").unwrap();
        std::fs::write(r.join(".env"), "SECRET=1\n").unwrap();
        std::fs::create_dir_all(r.join("node_modules/pkg")).unwrap();
        std::fs::write(r.join("node_modules/pkg/index.js"), "x").unwrap();
        for app in ["web", "api"] {
            std::fs::create_dir_all(r.join("apps").join(app)).unwrap();
            std::fs::write(r.join("apps").join(app).join(".env"), app).unwrap();
        }
        std::fs::write(r.join(".worktreeinclude"), "# copied into worktrees\n.env\nnode_modules/\napps/*/.env\n../escape\nmissing.txt\n").unwrap();
        git(&r, &["add", "-A"]).unwrap();
        git(&r, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "inc"]).unwrap();
        let c = create(&r, &r.with_extension("wt"), "claude", "inc").unwrap();
        let mut copied = copy_includes(&r, &c.path).unwrap();
        copied.sort();
        let names: Vec<String> = copied.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(names, [".env", "apps/api/.env", "apps/web/.env", "node_modules"]);
        assert_eq!(std::fs::read_to_string(c.path.join("node_modules/pkg/index.js")).unwrap(), "x");
        assert!(copy_includes(&r, &c.path).unwrap().is_empty(), "second run copies nothing");
        let (ok, out) = run_setup(&c.path, "echo setting up; test -f .env", "/usr/bin:/bin");
        assert!(ok, "{out}");
        assert!(out.contains("setting up"));
        assert!(!run_setup(&c.path, "exit 3", "/usr/bin:/bin").0);
        let (ok, out) = run_check(&c.path, "echo checking; test -f .env", "/usr/bin:/bin", std::time::Duration::from_secs(10));
        assert!(ok, "{out}");
        assert!(out.contains("checking"));
        assert!(!run_check(&c.path, "echo nope; exit 2", "/usr/bin:/bin", std::time::Duration::from_secs(10)).0);
        let t0 = std::time::Instant::now();
        let (ok, out) = run_check(&c.path, "sleep 30 & sleep 30", "/usr/bin:/bin", std::time::Duration::from_millis(300));
        assert!(!ok && out.contains("check stopped"), "{out}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "timeout kills the group: {:?}", t0.elapsed());
        remove(&r, &c.path, &c.branch, true).unwrap();
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.env", "web.env") && glob_match(".env*", ".env.local") && !glob_match("a?c", "abbc"));
    }

    #[test]
    fn needs_repo_with_commit() {
        let d = std::env::temp_dir().join(format!("qur-wt-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(create(&d, &d.with_extension("wt"), "claude", "x").unwrap_err().contains("git repo"));
    }
}
