//! Push and Open Pull Request: ship a task's branch to the user's remote and
//! open a pull request with their own `gh`, or GitHub's compare page when
//! `gh` isn't installed or signed in.
//!
//! Qur makes no network call itself: the user's `git` and `gh` do, with their
//! own logins, found through the login-shell PATH (passed in as `path`, so
//! tests can put a fake `gh` first). Every command has a timeout and nothing
//! here prompts. It all blocks: call it off the UI thread. Remote branches
//! are never deleted.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `git push` (hooks and slow links included) and each `gh` call.
pub const PUSH_TIMEOUT: Duration = Duration::from_secs(120);
pub const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// Where a task goes: the remote of the user's current branch (or `origin`),
/// its URL, and the branch the pull request merges into (by its name there).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub remote: String,
    pub url: String,
    pub base: String,
}

/// A pushed task branch, with the pull request's text.
#[derive(Clone, Debug, PartialEq)]
pub struct Pushed {
    pub target: Target,
    pub branch: String,
    pub title: String,
    pub body: String,
}

/// How the pull request was opened.
#[derive(Clone, Debug, PartialEq)]
pub enum Opened {
    /// `gh pr create` made it (its URL).
    Created(String),
    /// `gh` found one already open for the branch.
    Existing(String),
    /// No usable `gh`: GitHub's compare page, to open in the browser.
    Compare(String),
    /// Not a github.com remote and no usable `gh`: pushed, nothing more.
    PushedOnly,
}

/// The pull request's title and body. The title is the task's; the body
/// lists what was asked (an ACP chat's prompts), the agent's own commits
/// (not Qur's "qur: …" commit of the leftover work) and the diff's size.
/// One place to build it, so a generated message can replace it later.
pub fn pr_text(title: &str, branch: &str, prompts: &[String], commits: &[String], stat: &str) -> (String, String) {
    let first = title.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let t = if first.is_empty() { branch.to_string() } else { clip(first, 100) };
    let mut body = String::new();
    let asked: Vec<String> = prompts.iter().filter_map(|p| p.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| clip(l, 200))).take(10).collect();
    if !asked.is_empty() {
        body.push_str("Asked:\n");
        for a in asked {
            body.push_str(&format!("- {a}\n"));
        }
        body.push('\n');
    }
    let own = format!("qur: {}", title.trim());
    let commits: Vec<&String> = commits.iter().filter(|c| !c.trim().is_empty() && c.trim() != own).take(20).collect();
    if !commits.is_empty() {
        body.push_str("Commits:\n");
        for c in commits {
            body.push_str(&format!("- {}\n", c.trim()));
        }
        body.push('\n');
    }
    if !stat.trim().is_empty() {
        body.push_str(&format!("{}\n\n", stat.trim()));
    }
    body.push_str(&format!("From the task branch `{branch}`."));
    (t, body)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// `git push -u <remote> <branch>`.
pub fn push_args(remote: &str, branch: &str) -> Vec<String> {
    ["push", "-u", remote, branch].map(String::from).to_vec()
}

/// `gh pr create …`; `repo` ("host/owner/name") when the remote URL tells it,
/// so `gh` needn't guess between several remotes.
pub fn gh_create_args(repo: Option<&str>, branch: &str, base: &str, title: &str, body: &str) -> Vec<String> {
    let mut v: Vec<String> = ["pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body].map(String::from).to_vec();
    if let Some(r) = repo {
        v.extend(["--repo".to_string(), r.to_string()]);
    }
    v
}

/// Host, owner and repository name of a remote URL: `https://host/o/r(.git)`,
/// `git@host:o/r(.git)`, `ssh://git@host[:port]/o/r(.git)`. None for paths
/// and anything else.
pub fn parse_remote(url: &str) -> Option<(String, String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = ["https://", "http://", "ssh://", "git://"].iter().find_map(|p| url.strip_prefix(p)) {
        let (auth, path) = rest.split_once('/')?;
        let host = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
        (host.split(':').next()?.to_string(), path)
    } else if !url.contains("://") && !url.starts_with('/') && !url.starts_with('.') {
        // scp-like: [user@]host:owner/repo
        let (auth, path) = url.split_once(':')?;
        (auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth).to_string(), path)
    } else {
        return None;
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    if host.is_empty() || owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some((host.to_lowercase(), owner.to_string(), repo.to_string()))
}

/// GitHub's "open a pull request" page for `branch` into `base`. Only for
/// github.com remotes.
pub fn compare_url(remote_url: &str, base: &str, branch: &str) -> Option<String> {
    let (host, owner, repo) = parse_remote(remote_url)?;
    (host == "github.com").then(|| format!("https://github.com/{owner}/{repo}/compare/{}...{}?expand=1", enc(base), enc(branch)))
}

/// Percent-encode what would break a URL path (branch names may hold any of these).
fn enc(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '/' | '~' => out.push(c),
            _ => {
                let mut b = [0; 4];
                for byte in c.encode_utf8(&mut b).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}

/// The first pull request URL in some output (`gh` prints it; "already
/// exists" errors name it).
pub fn find_pr_url(text: &str) -> Option<String> {
    text.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/' && c != ':' && c != '.')).find(|w| w.starts_with("https://") && w.contains("/pull/")).map(str::to_string)
}

/// The last few lines of a command's error output, for the flash and notice.
pub fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join("\n")
}

// ---------------------------------------------------------------------------
// Running git and gh
// ---------------------------------------------------------------------------

pub struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    /// stderr's tail, else stdout's.
    pub fn error(&self) -> String {
        let e = tail(&self.stderr);
        if e.is_empty() { tail(&self.stdout) } else { e }
    }
}

/// Run with a timeout, in its own process group so a stuck `ssh` under
/// `git push` goes too.
pub fn run(mut cmd: Command, timeout: Duration) -> Result<Ran, String> {
    let name = cmd.get_program().to_string_lossy().rsplit('/').next().unwrap_or("command").to_string();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("{name}: {e}"))?;
    let read = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut s);
            }
            String::from_utf8_lossy(&s).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                // SAFETY: signals the child's own process group (process_group(0) above).
                unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
                let _ = child.wait();
                return Err(format!("{name} didn't finish within {} s", timeout.as_secs()));
            }
            Err(e) => return Err(format!("{name}: {e}")),
        }
    };
    Ok(Ran { ok: status.success(), stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

fn git(dir: &Path, path: &str, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .env("PATH", path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(args);
    c
}

/// Quick read-only git: trimmed stdout, or the error's tail.
fn git_text(dir: &Path, path: &str, args: &[&str]) -> Result<String, String> {
    let r = run(git(dir, path, args), GH_TIMEOUT)?;
    if r.ok { Ok(r.stdout.trim().to_string()) } else { Err(r.error()) }
}

/// `program` in a PATH string.
pub fn find_in(path: &str, program: &str) -> Option<PathBuf> {
    path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(program)).find(|p| p.is_file())
}

fn gh(program: &Path, dir: &Path, path: &str, args: &[String]) -> Command {
    let mut c = Command::new(program);
    c.current_dir(dir)
        .env("PATH", path)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_SPINNER_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args);
    c
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

/// The user's current branch in `root` and where it pushes.
pub fn target(root: &Path, path: &str) -> Result<Target, String> {
    let local = git_text(root, path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .filter(|b| !b.is_empty())
        .ok_or("Your project folder isn't on a branch (detached HEAD): check out the branch the pull request should go into")?;
    let configured = git_text(root, path, &["config", "--get", &format!("branch.{local}.remote")]).ok().filter(|r| !r.is_empty() && r != ".");
    let remote = configured.clone().unwrap_or_else(|| "origin".into());
    let url = git_text(root, path, &["remote", "get-url", &remote]).map_err(|e| {
        if configured.is_none() { format!("No remote to push to: add one (git remote add origin <url>). {e}") } else { format!("Remote “{remote}”: {e}") }
    })?;
    let base = configured
        .and_then(|_| git_text(root, path, &["config", "--get", &format!("branch.{local}.merge")]).ok())
        .and_then(|m| m.strip_prefix("refs/heads/").map(str::to_string))
        .filter(|b| !b.is_empty())
        .unwrap_or(local);
    Ok(Target { remote, url, base })
}

/// Commit the task's leftover work (like Merge: "qur: <title>"), check its
/// branch has something the user's branch lacks, and push it.
pub fn commit_and_push(root: &Path, wt: &Path, branch: &str, title: &str, prompts: &[String], path: &str) -> Result<Pushed, String> {
    if branch.is_empty() || branch.starts_with('-') {
        return Err(format!("Not a branch: {branch:?}"));
    }
    let target = target(root, path)?;
    if target.remote.starts_with('-') {
        return Err(format!("Not a remote: {:?}", target.remote));
    }
    qur_git::commit_all(wt, &format!("qur: {title}")).map_err(|e| format!("Couldn't commit the task's work: {e}"))?;
    let local = git_text(root, path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let range = format!("{local}..{branch}");
    let ahead = git_text(root, path, &["rev-list", "--count", &range, "--"])?;
    if ahead == "0" {
        return Err(format!("Nothing to push: {local} already has every commit of {branch}"));
    }
    let commits = git_text(root, path, &["log", "--reverse", "--format=%s", &range, "--"]).unwrap_or_default();
    let commits: Vec<String> = commits.lines().map(str::to_string).collect();
    let stat = git_text(root, path, &["diff", "--shortstat", &format!("{local}...{branch}"), "--"]).unwrap_or_default();
    let args = push_args(&target.remote, branch);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = run(git(wt, path, &args), PUSH_TIMEOUT)?;
    if !r.ok {
        return Err(format!("git push failed: {}", r.error()));
    }
    let (title, body) = pr_text(title, branch, prompts, &commits, &stat);
    Ok(Pushed { target, branch: branch.to_string(), title, body })
}

/// Open the pull request for a pushed branch: `gh pr create` when `gh` is
/// installed and signed in, else GitHub's compare page (github.com only).
pub fn open(wt: &Path, p: &Pushed, path: &str) -> Result<Opened, String> {
    let remote = parse_remote(&p.target.url);
    let fallback = || compare_url(&p.target.url, &p.target.base, &p.branch).map(Opened::Compare).unwrap_or(Opened::PushedOnly);
    let Some(gh_bin) = find_in(path, "gh") else { return Ok(fallback()) };
    let mut auth = vec!["auth".to_string(), "status".to_string()];
    if let Some((host, ..)) = &remote {
        auth.extend(["--hostname".to_string(), host.clone()]);
    }
    let signed_in = run(gh(&gh_bin, wt, path, &auth), GH_TIMEOUT).is_ok_and(|r| r.ok);
    if !signed_in {
        return Ok(fallback());
    }
    let repo = remote.map(|(h, o, r)| format!("{h}/{o}/{r}"));
    let r = run(gh(&gh_bin, wt, path, &gh_create_args(repo.as_deref(), &p.branch, &p.target.base, &p.title, &p.body)), GH_TIMEOUT)?;
    match (r.ok, find_pr_url(&r.stdout), find_pr_url(&r.stderr)) {
        (true, Some(url), _) => Ok(Opened::Created(url)),
        (false, _, Some(url)) if r.stderr.contains("already exists") => Ok(Opened::Existing(url)),
        (true, None, _) => Err(format!("gh pr create printed no pull request address: {}", tail(&r.stdout))),
        _ => Err(format!("gh pr create failed: {}", r.error())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls() {
        let gh = Some(("github.com".to_string(), "acme".to_string(), "proj".to_string()));
        for u in [
            "https://github.com/acme/proj.git",
            "https://github.com/acme/proj",
            "https://user:tok@github.com/acme/proj.git/",
            "git@github.com:acme/proj.git",
            "ssh://git@github.com/acme/proj.git",
            "ssh://git@GitHub.com:22/acme/proj",
        ] {
            assert_eq!(parse_remote(u), gh, "{u}");
        }
        assert_eq!(parse_remote("git@gitlab.example.com:team/app.git"), Some(("gitlab.example.com".into(), "team".into(), "app".into())));
        for u in ["/tmp/remote.git", "./remote.git", "file:///tmp/r.git", "https://github.com/acme", "github.com", ""] {
            assert_eq!(parse_remote(u), None, "{u}");
        }
    }

    #[test]
    fn compare_only_for_github() {
        assert_eq!(compare_url("git@github.com:acme/proj.git", "main", "qur/claude/fix-nav").as_deref(), Some("https://github.com/acme/proj/compare/main...qur/claude/fix-nav?expand=1"));
        assert_eq!(compare_url("https://github.com/acme/proj", "rel#2", "a b").as_deref(), Some("https://github.com/acme/proj/compare/rel%232...a%20b?expand=1"));
        assert_eq!(compare_url("git@gitlab.com:acme/proj.git", "main", "x"), None);
        assert_eq!(compare_url("/srv/git/proj.git", "main", "x"), None);
    }

    #[test]
    fn title_and_body() {
        let prompts = vec!["Fix the navbar on mobile\nit overlaps the logo".to_string(), "  ".to_string(), "also add a test".to_string()];
        let commits = vec!["Make the navbar wrap".to_string(), "qur: Fix the navbar".to_string()];
        let (t, b) = pr_text("Fix the navbar", "qur/claude/fix-the-navbar", &prompts, &commits, " 2 files changed, 12 insertions(+), 4 deletions(-)");
        assert_eq!(t, "Fix the navbar");
        assert_eq!(
            b,
            "Asked:\n- Fix the navbar on mobile\n- also add a test\n\nCommits:\n- Make the navbar wrap\n\n2 files changed, 12 insertions(+), 4 deletions(-)\n\nFrom the task branch `qur/claude/fix-the-navbar`."
        );
        let (t, b) = pr_text("  ", "qur/claude/task", &[], &[], "");
        assert_eq!((t.as_str(), b.as_str()), ("qur/claude/task", "From the task branch `qur/claude/task`."));
        let long = "x".repeat(150);
        assert_eq!(pr_text(&long, "b", &[], &[], "").0.chars().count(), 100);
    }

    #[test]
    fn commands() {
        assert_eq!(push_args("origin", "qur/claude/fix"), ["push", "-u", "origin", "qur/claude/fix"]);
        assert_eq!(
            gh_create_args(Some("github.com/acme/proj"), "qur/claude/fix", "main", "Fix", "Body"),
            ["pr", "create", "--head", "qur/claude/fix", "--base", "main", "--title", "Fix", "--body", "Body", "--repo", "github.com/acme/proj"]
        );
        assert_eq!(gh_create_args(None, "b", "main", "T", "B").len(), 10);
        assert_eq!(find_pr_url("https://github.com/acme/proj/pull/7\n").as_deref(), Some("https://github.com/acme/proj/pull/7"));
        assert_eq!(
            find_pr_url("a pull request for branch \"x\" into branch \"main\" already exists:\nhttps://github.com/a/b/pull/3").as_deref(),
            Some("https://github.com/a/b/pull/3")
        );
        assert_eq!(find_pr_url("https://github.com/a/b/compare/x"), None);
        assert_eq!(tail("a\n\nb\nc\nd\ne\n"), "b\nc\nd\ne");
    }

    #[test]
    fn timeout_kills() {
        let t = Instant::now();
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "sleep 5; echo late"]);
        let e = run(c, Duration::from_millis(200)).err().unwrap();
        assert!(e.contains("didn't finish"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(3));
    }

    // -- a scripted run: a local bare remote and a fake `gh` -------------------

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-pr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").current_dir(dir).env_remove("GIT_DIR").env_remove("GIT_INDEX_FILE").args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A bare remote, a clone tracking it on `main`, and a task worktree
    /// with an uncommitted file on `qur/test/fix`.
    fn setup(d: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let (remote, root, wt) = (d.join("remote.git"), d.join("proj"), d.join("wt"));
        sh(d, &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        sh(d, &["init", "-q", "-b", "main", root.to_str().unwrap()]);
        for (k, v) in [("user.name", "Test"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null")] {
            sh(&root, &["config", k, v]);
        }
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        sh(&root, &["add", "-A"]);
        sh(&root, &["commit", "-q", "-m", "init"]);
        sh(&root, &["remote", "add", "origin", remote.to_str().unwrap()]);
        sh(&root, &["push", "-q", "-u", "origin", "main"]);
        sh(&root, &["worktree", "add", "-q", "-b", "qur/test/fix", wt.to_str().unwrap()]);
        std::fs::write(wt.join("b.txt"), "b\n").unwrap();
        (remote, root, wt)
    }

    /// A fake `gh` that records its arguments; `auth` is its `auth status` exit code.
    fn fake_gh(d: &Path, auth: i32) -> (String, PathBuf) {
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let log = d.join("gh-args.txt");
        let script = format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{log}'; done\necho --- >> '{log}'\nif [ \"$1\" = auth ]; then exit {auth}; fi\necho https://github.com/acme/proj/pull/7\n",
            log = log.display()
        );
        let gh = bin.join("gh");
        std::fs::write(&gh, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        (format!("{}:/usr/bin:/bin", bin.display()), log)
    }

    #[test]
    fn push_and_create_with_fake_gh() {
        let d = scratch("gh");
        let (remote, root, wt) = setup(&d);
        let (path, log) = fake_gh(&d, 0);
        let p = commit_and_push(&root, &wt, "qur/test/fix", "Fix it", &["Add b".into()], &path).unwrap();
        assert_eq!(p.target, Target { remote: "origin".into(), url: remote.to_str().unwrap().into(), base: "main".into() });
        // The leftover work was committed and pushed, with an upstream.
        assert_eq!(sh(&remote, &["log", "-1", "--format=%s", "qur/test/fix"]), "qur: Fix it");
        assert_eq!(sh(&wt, &["rev-parse", "--abbrev-ref", "qur/test/fix@{upstream}"]), "origin/qur/test/fix");
        assert!(p.body.contains("- Add b") && p.body.contains("1 file changed"), "{}", p.body);
        assert_eq!(open(&wt, &p, &path).unwrap(), Opened::Created("https://github.com/acme/proj/pull/7".into()));
        let args = std::fs::read_to_string(&log).unwrap();
        let calls: Vec<&str> = args.split("---\n").filter(|s| !s.is_empty()).collect();
        assert_eq!(calls[0], "auth\nstatus\n", "a local remote has no host to name");
        assert!(calls[1].starts_with("pr\ncreate\n--head\nqur/test/fix\n--base\nmain\n--title\nFix it\n--body\n"), "{}", calls[1]);
        // Again (new commits for the open pull request): pushes what's new.
        std::fs::write(wt.join("c.txt"), "c\n").unwrap();
        commit_and_push(&root, &wt, "qur/test/fix", "Fix it", &[], &path).unwrap();
        assert_eq!(sh(&remote, &["rev-parse", "qur/test/fix"]), sh(&wt, &["rev-parse", "HEAD"]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn without_gh_and_failures() {
        let d = scratch("nogh");
        let (_, root, wt) = setup(&d);
        // A task with no commits of its own: refused before pushing.
        let empty = d.join("empty");
        sh(&root, &["worktree", "add", "-q", "-b", "qur/test/empty", empty.to_str().unwrap()]);
        let e = commit_and_push(&root, &empty, "qur/test/empty", "Empty", &[], "/usr/bin:/bin").err().unwrap();
        assert!(e.starts_with("Nothing to push"), "{e}");
        // No gh, a non-GitHub remote: pushed only.
        let p = commit_and_push(&root, &wt, "qur/test/fix", "Fix", &[], "/usr/bin:/bin").unwrap();
        assert_eq!(open(&wt, &p, "/usr/bin:/bin").unwrap(), Opened::PushedOnly);
        // gh signed out, a github.com remote: the compare page (no push here).
        let (path, log) = fake_gh(&d, 1);
        let gh_remote = Pushed { target: Target { url: "git@github.com:acme/proj.git".into(), ..p.target.clone() }, ..p.clone() };
        assert_eq!(open(&wt, &gh_remote, &path).unwrap(), Opened::Compare("https://github.com/acme/proj/compare/main...qur/test/fix?expand=1".into()));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "auth\nstatus\n--hostname\ngithub.com\n---\n");
        // A rejected push shows git's error.
        std::fs::write(wt.join("c.txt"), "c\n").unwrap();
        sh(&root, &["remote", "set-url", "origin", d.join("missing.git").to_str().unwrap()]);
        let e = commit_and_push(&root, &wt, "qur/test/fix", "Fix", &[], "/usr/bin:/bin").err().unwrap();
        assert!(e.starts_with("git push failed:") && e.contains("missing.git"), "{e}");
        // No remote at all.
        sh(&root, &["remote", "remove", "origin"]);
        let e = target(&root, "/usr/bin:/bin").err().unwrap();
        assert!(e.starts_with("No remote to push to"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
