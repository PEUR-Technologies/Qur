//! `qur doctor` (M7) and Help > Run Doctor: checks that Qur can run agents on
//! this Mac. Never calls the network itself; agents start on their own login.

use std::path::{Path, PathBuf};
use std::time::Duration;

use qur_agents::{AgentDef, Mode};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

pub struct Check {
    pub level: Level,
    pub name: String,
    pub detail: String,
}

fn check(level: Level, name: impl Into<String>, detail: impl Into<String>) -> Check {
    Check { level, name: name.into(), detail: detail.into() }
}

/// Every check, in order. Blocking (starts agents): call off the UI thread.
pub fn run() -> Vec<Check> {
    let mut out = vec![macos(), git()];
    out.push(match writable(&qur_config::qur_home()) {
        Ok(()) => check(Level::Ok, "Settings folder", qur_config::qur_home().display().to_string()),
        Err(e) => check(Level::Fail, "Settings folder", format!("{}: {e}", qur_config::qur_home().display())),
    });
    out.push(worktrees());
    out.push(match qur_config::Config::load().1 {
        None => check(Level::Ok, "config.toml", "reads fine"),
        Some(e) => check(Level::Fail, "config.toml", e),
    });
    let (roster, err) = qur_agents::Roster::load();
    out.push(match err {
        None => check(Level::Ok, "agents.toml", format!("{} agent(s)", roster.agents.len())),
        Some(e) => check(Level::Fail, "agents.toml", e),
    });
    for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        if std::env::var_os(key).is_some() {
            out.push(check(Level::Warn, key, "is set in this environment; Qur removes it for agents, which use their own CLI login"));
        }
    }
    for def in &roster.agents {
        out.push(agent(def));
    }
    for (cli, name) in [("claude", "Claude sign-in"), ("codex", "ChatGPT (Codex) sign-in")] {
        if qur_agents::which(cli).is_none() {
            continue;
        }
        out.push(match login(cli) {
            Login::In(s) => check(Level::Ok, name, s),
            Login::Out => check(Level::Warn, name, format!("not signed in (run `{}`)", login_command(cli))),
            Login::Unknown(e) => check(Level::Warn, name, e),
        });
    }
    out
}

/// What Qur's task worktrees take on disk (`<qur home>/worktrees`).
fn worktrees() -> Check {
    let dir = qur_config::qur_home().join("worktrees");
    let folders: usize = std::fs::read_dir(&dir).map(|rd| rd.filter_map(Result::ok).filter_map(|p| std::fs::read_dir(p.path()).ok()).map(|rd| rd.count()).sum()).unwrap_or(0);
    if folders == 0 {
        return check(Level::Ok, "Worktrees", "none on disk");
    }
    let size = crate::files::human_size(qur_worktree::cleanup::disk_use(&dir));
    check(Level::Ok, "Worktrees", format!("{size} in {folders} folder{} (Agent > Clean Up Worktrees… frees leftovers)", if folders == 1 { "" } else { "s" }))
}

/// A CLI's own sign-in, as Qur's agents will see it (API keys removed).
#[derive(Clone, PartialEq, Debug)]
pub enum Login {
    /// Signed in: who and how, for the user.
    In(String),
    Out,
    /// Couldn't tell (the CLI failed or said something unexpected).
    Unknown(String),
}

/// The command that signs a CLI in (it opens the browser).
pub fn login_command(cli: &str) -> &'static str {
    if cli == "codex" { "codex login" } else { "claude auth login" }
}

/// Ask the CLI itself: `claude auth status --json` or `codex login status`.
/// Blocking: call off the UI thread.
pub fn login(cli: &str) -> Login {
    let Some(program) = qur_agents::which(cli) else { return Login::Unknown(format!("{cli} isn't installed")) };
    let args: &[&str] = if cli == "codex" { &["login", "status"] } else { &["auth", "status", "--json"] };
    let out = std::process::Command::new(program)
        .args(args)
        .env("PATH", qur_agents::login_path())
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(std::process::Stdio::null())
        .output();
    let Ok(out) = out else { return Login::Unknown(format!("couldn't run {cli}")) };
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if cli == "codex" { codex_login(out.status.success(), &text) } else { claude_login(&text) }
}

fn claude_login(text: &str) -> Login {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Login::Unknown(format!("unexpected answer from `claude auth status`: {}", first_line(text)));
    };
    if v["loggedIn"].as_bool() != Some(true) {
        return Login::Out;
    }
    let who = v["email"].as_str().map(|e| format!(" as {e}")).unwrap_or_default();
    match (v["authMethod"].as_str(), v["subscriptionType"].as_str()) {
        (Some("claude.ai"), Some(plan)) => Login::In(format!("Signed in{who} ({})", capitalized(plan))),
        (Some("claude.ai"), None) => Login::In(format!("Signed in{who}")),
        _ => Login::In(format!("Signed in{who} with an API account (billed per use, not a Claude plan)")),
    }
}

fn codex_login(ok: bool, text: &str) -> Login {
    let lower = text.to_lowercase();
    if ok && lower.contains("chatgpt") {
        Login::In("Signed in with ChatGPT".into())
    } else if ok && lower.contains("api key") {
        Login::In("Signed in with an API key (billed per use, not a ChatGPT plan)".into())
    } else if ok {
        Login::In(first_line(text))
    } else if lower.contains("not logged in") {
        Login::Out
    } else {
        Login::Unknown(format!("`codex login status`: {}", first_line(text)))
    }
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no output").to_string()
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// One agent: found on the login PATH, then a real start. ACP agents get an
/// `initialize`; terminal agents `--version`.
pub fn agent(def: &AgentDef) -> Check {
    let mode = if def.mode == Mode::Acp { "ACP" } else { "terminal" };
    let name = if def.label.to_lowercase().contains(&mode.to_lowercase()) { def.label.clone() } else { format!("{} ({mode})", def.label) };
    let Some(program) = qur_agents::which(&def.command) else {
        return check(Level::Fail, name, format!("{} not found on PATH", def.command));
    };
    let mut detail = match check_agent(def, &program) {
        Ok(s) => s,
        Err(e) => return check(Level::Fail, name, e),
    };
    // The ACP adapters are pinned (HANDOFF: never npx at runtime).
    let tested = match def.command.as_str() {
        "claude-agent-acp" => Some("0.94.0"),
        "codex-acp" => Some("0.16.0"),
        _ => None,
    };
    if let Some(tested) = tested
        && let Some(v) = npm_package_version(&program)
    {
        detail = format!("{detail} · adapter {v}");
        if v != tested {
            return check(Level::Warn, name, format!("{detail} (Qur is tested with {tested})"));
        }
    }
    check(Level::Ok, name, detail)
}

/// Shared with the first-run picker's Check button.
pub fn check_agent(def: &AgentDef, program: &Path) -> Result<String, String> {
    let path = qur_agents::login_path().to_string();
    match def.mode {
        Mode::Acp => {
            let cmd = qur_acp::AgentCommand { program: program.to_string_lossy().into_owned(), args: def.args.clone(), env: vec![("PATH".into(), path)] };
            let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| "/".into());
            let (client, _events) = qur_acp::Client::spawn(&cmd, &home).map_err(|e| format!("couldn't start it: {e}"))?;
            let pending = client.initialize();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                tx.send(futures::executor::block_on(pending)).ok();
            });
            let init = match rx.recv_timeout(Duration::from_secs(20)) {
                Ok(Ok(Ok(i))) => Ok(i),
                Ok(Ok(Err(e))) => Err(format!("it started but refused to talk to Qur ({e})")),
                Ok(Err(_)) => Err("it stopped right after starting; try Install again".to_string()),
                Err(_) => Err("it didn't answer in 20 s".to_string()),
            };
            client.shutdown(Duration::from_secs(2));
            let i = init?;
            let info = &i.raw["agentInfo"];
            Ok(match (info["name"].as_str(), info["version"].as_str()) {
                (Some(n), Some(v)) => format!("{n} {v}, ACP v{}", i.raw["protocolVersion"]),
                _ => format!("ACP v{}", i.raw["protocolVersion"]),
            })
        }
        Mode::Terminal => {
            let out = std::process::Command::new(program).arg("--version").env("PATH", path).output().map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            let line = text.lines().next().unwrap_or("").trim().to_string();
            if out.status.success() {
                Ok(if line.is_empty() { "runs".into() } else { line })
            } else {
                Err(format!("`{} --version` exited with {}", def.command, out.status))
            }
        }
    }
}

/// An npm-installed CLI's package version (bin symlink → package.json).
fn npm_package_version(bin: &Path) -> Option<String> {
    let real = std::fs::canonicalize(bin).ok()?;
    real.ancestors().skip(1).find_map(|dir| {
        let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v["version"].as_str().map(str::to_string)
    })
}

fn macos() -> Check {
    let out = std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion").output();
    let v = out.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let major: u32 = v.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
    if major >= 13 { check(Level::Ok, "macOS", v) } else { check(Level::Fail, "macOS", format!("{v}: Qur needs macOS 13 or newer")) }
}

fn git() -> Check {
    let out = std::process::Command::new("git").arg("--version").output();
    let Ok(out) = out else { return check(Level::Fail, "git", "not found (Qur needs git 2.38 or newer)") };
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let v: Vec<u32> = text.split_whitespace().nth(2).unwrap_or("").split('.').filter_map(|p| p.parse().ok()).collect();
    let ok = matches!(v.as_slice(), [a, b, ..] if (*a, *b) >= (2, 38));
    // Merge uses `git merge-tree --write-tree` (2.38+).
    if ok { check(Level::Ok, "git", text) } else { check(Level::Fail, "git", format!("{text}: Merge needs git 2.38 or newer")) }
}

fn writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".qur-doctor-probe");
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)
}

pub fn render(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|c| {
            let mark = match c.level {
                Level::Ok => "✓",
                Level::Warn => "!",
                Level::Fail => "✕",
            };
            format!("{mark} {}: {}", c.name, c.detail)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `qur doctor` in a terminal: print, exit 1 on any failure.
pub fn cli() -> i32 {
    println!("Qur {} doctor", env!("CARGO_PKG_VERSION"));
    let checks = run();
    println!("{}", render(&checks));
    let fails = checks.iter().filter(|c| c.level == Level::Fail).count();
    let warns = checks.iter().filter(|c| c.level == Level::Warn).count();
    memory();
    println!("\n{}", if fails > 0 { format!("{fails} problem(s), {warns} warning(s)") } else if warns > 0 { format!("OK, {warns} warning(s)") } else { "All good".to_string() });
    if fails > 0 { 1 } else { 0 }
}

/// Memory of every running Qur: its own footprint and its agents' and
/// terminals' (sampled once, now). Information only: never a problem.
fn memory() {
    let qurs = crate::mem::other_qurs();
    if qurs.is_empty() {
        println!("\nMemory: Qur isn't running");
        return;
    }
    for pid in qurs {
        let r = crate::mem::report(pid, &Default::default());
        println!("\nMemory of Qur (pid {pid}): {}", r.summary());
        for line in r.lines() {
            println!("  {line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_status() {
        let max = r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "a@b.c", "subscriptionType": "max"}"#;
        assert_eq!(claude_login(max), Login::In("Signed in as a@b.c (Max)".into()));
        assert_eq!(claude_login(r#"{"loggedIn": false}"#), Login::Out);
        assert!(matches!(claude_login(r#"{"loggedIn": true, "authMethod": "apiKey"}"#), Login::In(s) if s.contains("API")));
        assert!(matches!(claude_login("boom"), Login::Unknown(_)));
    }

    #[test]
    fn reads_codex_status() {
        assert_eq!(codex_login(true, "Logged in using ChatGPT\n"), Login::In("Signed in with ChatGPT".into()));
        assert!(matches!(codex_login(true, "Logged in using an API key - sk-***"), Login::In(s) if s.contains("API key")));
        assert_eq!(codex_login(false, "Not logged in\n"), Login::Out);
        assert!(matches!(codex_login(false, "error: config"), Login::Unknown(_)));
    }
}
