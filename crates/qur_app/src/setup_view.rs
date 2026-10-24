//! The first-run picker (handoff section 13, M6): AI accounts to connect
//! (install, sign in, agents found on PATH with a Check; decision 45), the
//! stage layout, keys (VS Code or + Vim), and the theme with live previews.
//! Saves config.toml and agents.toml. Shown once on a new install; Qur >
//! Setup… reopens it.

use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, EventEmitter, FocusHandle, Task, Window, div, prelude::*, px, svg};
use qur_agents::{AgentDef, Mode};
use qur_theme::{Registry, Theme};

use crate::assets;
use crate::doctor::Login;
use crate::look::Look;

/// Agents Qur knows how to run (Codex since decision 44). Others can be
/// added to agents.toml by hand.
const KNOWN: &[(&str, &str, Mode, &str, Option<&str>)] = &[
    ("claude", "Claude Code", Mode::Acp, "claude-agent-acp", None),
    ("claude-cli", "Claude Code (terminal)", Mode::Terminal, "claude", Some("arg")),
    // Zed's ACP adapter for OpenAI's Codex CLI; signs in with ChatGPT.
    ("codex", "Codex", Mode::Acp, "codex-acp", None),
    ("codex-cli", "Codex (terminal)", Mode::Terminal, "codex", Some("arg")),
    ("aider", "Aider (terminal)", Mode::Terminal, "aider", Some("--message")),
];

/// An AI account Setup connects (decision 45): a CLI the user signs in to,
/// and Qur's ACP adapter for it, both installed with npm.
struct Service {
    name: &'static str,
    plan: &'static str,
    cli: &'static str,
    /// (command, npm package): the CLI, then the adapter, pinned as tested.
    packages: [(&'static str, &'static str); 2],
    /// Its agents in `KNOWN`: over ACP, then in a terminal.
    agents: [&'static str; 2],
}

const SERVICES: [Service; 2] = [
    Service {
        name: "Claude",
        plan: "Uses your Claude Pro or Max plan",
        cli: "claude",
        packages: [("claude", "@anthropic-ai/claude-code"), ("claude-agent-acp", "@agentclientprotocol/claude-agent-acp@0.94.0")],
        agents: ["claude", "claude-cli"],
    },
    Service {
        name: "ChatGPT (Codex)",
        plan: "Uses your ChatGPT Plus, Pro or Business plan",
        cli: "codex",
        packages: [("codex", "@openai/codex"), ("codex-acp", "@zed-industries/codex-acp@0.16.0")],
        agents: ["codex", "codex-cli"],
    },
];

/// What Setup waits for while an install or sign-in runs in a terminal.
#[derive(Clone, Copy, PartialEq)]
enum Wait {
    Install,
    SignIn,
}

#[derive(Clone, PartialEq)]
enum Check {
    NotRun,
    Running,
    Ok(String),
    Failed(String),
}

struct Found {
    def: AgentDef,
    path: Option<std::path::PathBuf>,
    enabled: bool,
    check: Check,
}

pub struct Choices {
    pub agents: Vec<AgentDef>,
    pub layout: String,
    pub vim: bool,
    pub theme: String,
    pub accent: usize,
}

pub enum SetupEvent {
    /// Install or sign in: Setup steps aside while this runs in a terminal.
    RunInTerminal { label: String, command: String },
    /// That step finished: show Setup again.
    Ready(String),
    /// Live preview while choosing.
    PreviewTheme(String),
    PreviewAccent(usize),
    Done(Choices),
    Cancel,
}

pub struct SetupView {
    pub focus: FocusHandle,
    agents: Vec<Found>,
    layout: String,
    vim: bool,
    theme: String,
    accent: usize,
    /// Each service's sign-in; `None` while checking.
    logins: [Option<Login>; 2],
    npm: bool,
    waiting: Option<(usize, Wait)>,
    _watch: Option<Task<()>>,
}

impl EventEmitter<SetupEvent> for SetupView {}

impl SetupView {
    pub fn new(config: &qur_config::Config, roster: &qur_agents::Roster, cx: &mut Context<Self>) -> Self {
        let agents = KNOWN
            .iter()
            .map(|(id, label, mode, command, prompt)| {
                let existing = roster.get(id).cloned();
                let path = qur_agents::which(command);
                Found {
                    enabled: existing.is_some() || (path.is_some() && *id == "claude"),
                    def: existing.unwrap_or(AgentDef {
                        id: id.to_string(),
                        label: label.to_string(),
                        mode: *mode,
                        command: command.to_string(),
                        args: Vec::new(),
                        accent: None,
                        prompt: prompt.map(str::to_string),
                    }),
                    path,
                    check: Check::NotRun,
                }
            })
            .collect();
        // A stage arranged by hand shows as "Yours".
        let preset = config.peek_layout.as_str();
        let custom = crate::dock::Docks::get(cx, crate::dock::Kind::Acp) != crate::dock::Dock::preset(crate::dock::Kind::Acp, preset)
            || crate::dock::Docks::get(cx, crate::dock::Kind::Terminal) != crate::dock::Dock::preset(crate::dock::Kind::Terminal, preset);
        let layout = if custom { "yours".to_string() } else { config.peek_layout.clone() };
        let mut this = SetupView {
            focus: cx.focus_handle(),
            agents,
            layout,
            vim: config.keys.vim,
            theme: config.theme.clone(),
            accent: config.ui.accent % 6,
            logins: [None, None],
            npm: false,
            waiting: None,
            _watch: None,
        };
        this.refresh(cx);
        this
    }

    pub fn theme(&self) -> &str {
        &self.theme
    }

    /// Look again: what's installed, and each sign-in.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_paths();
        for ix in 0..SERVICES.len() {
            self.check_login(ix, cx);
        }
    }

    fn refresh_paths(&mut self) {
        for f in &mut self.agents {
            f.path = qur_agents::which(&f.def.command);
        }
        self.npm = qur_agents::which("npm").is_some();
    }

    /// Installed: from the agents' paths (refreshed by `refresh_paths`).
    fn found(&self, bin: &str) -> bool {
        self.agents.iter().find(|f| f.def.command == bin).map(|f| f.path.is_some()).unwrap_or_else(|| qur_agents::which(bin).is_some())
    }

    fn missing(&self, ix: usize) -> Vec<(&'static str, &'static str)> {
        SERVICES[ix].packages.iter().copied().filter(|(bin, _)| !self.found(bin)).collect()
    }

    fn check_login(&mut self, ix: usize, cx: &mut Context<Self>) {
        let cli = SERVICES[ix].cli;
        if qur_agents::which(cli).is_none() {
            self.logins[ix] = None;
            return;
        }
        self.logins[ix] = None;
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let login = bg.spawn(async move { crate::doctor::login(cli) }).await;
            this.update(cx, |this, cx| this.set_login(ix, login, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    /// A sign-in found: with no agent chosen yet, use this account's ACP one.
    fn set_login(&mut self, ix: usize, login: Login, cx: &mut Context<Self>) {
        if matches!(login, Login::In(_))
            && !self.agents.iter().any(|f| f.enabled)
            && let Some(f) = self.agents.iter_mut().find(|f| f.def.id == SERVICES[ix].agents[0] && f.path.is_some())
        {
            f.enabled = true;
        }
        self.logins[ix] = Some(login);
        cx.notify();
    }

    fn connected(&self, ix: usize) -> bool {
        self.missing(ix).is_empty() && matches!(self.logins[ix], Some(Login::In(_)))
    }

    fn install(&mut self, ix: usize, cx: &mut Context<Self>) {
        let packages = self.missing(ix).iter().map(|(_, p)| *p).collect::<Vec<_>>().join(" ");
        self.run(ix, Wait::Install, format!("Install {}", SERVICES[ix].name), format!("npm install -g {packages}"), cx);
    }

    fn sign_in(&mut self, ix: usize, cx: &mut Context<Self>) {
        let cli = SERVICES[ix].cli;
        self.run(ix, Wait::SignIn, format!("Sign in to {}", SERVICES[ix].name), crate::doctor::login_command(cli).to_string(), cx);
    }

    /// Run a step in a Qur terminal, where the user can see it (and answer
    /// it), then watch for it to take effect and come back.
    fn run(&mut self, ix: usize, wait: Wait, label: String, command: String, cx: &mut Context<Self>) {
        self.waiting = Some((ix, wait));
        cx.emit(SetupEvent::RunInTerminal { label, command });
        let bg = cx.background_executor().clone();
        self._watch = Some(cx.spawn(async move |this, cx| {
            // Ten minutes, then the user reopens Setup by hand.
            for _ in 0..200 {
                bg.timer(Duration::from_secs(3)).await;
                let Ok(still) = this.update(cx, |t, cx| {
                    t.refresh_paths();
                    cx.notify();
                    t.waiting == Some((ix, wait))
                }) else {
                    return;
                };
                if !still {
                    return;
                }
                let done = match wait {
                    Wait::Install => this.update(cx, |t, _| t.missing(ix).is_empty()).unwrap_or(false),
                    Wait::SignIn => {
                        let cli = SERVICES[ix].cli;
                        let login = bg.spawn(async move { crate::doctor::login(cli) }).await;
                        let signed = matches!(login, Login::In(_));
                        if signed {
                            this.update(cx, |t, cx| t.set_login(ix, login, cx)).ok();
                        }
                        signed
                    }
                };
                if done {
                    this.update(cx, |t, cx| {
                        t.waiting = None;
                        let name = SERVICES[ix].name;
                        let msg = match wait {
                            Wait::Install => {
                                t.check_login(ix, cx);
                                format!("{name} is installed. Next: sign in.")
                            }
                            Wait::SignIn => format!("{name} is connected"),
                        };
                        cx.emit(SetupEvent::Ready(msg));
                    })
                    .ok();
                    return;
                }
            }
            this.update(cx, |t, cx| {
                t.waiting = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// ACP agents: start them and run `initialize` on the user's own login.
    /// Terminal agents: `<command> --version`.
    fn check(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(f) = self.agents.get_mut(ix) else { return };
        let Some(program) = f.path.clone() else { return };
        f.check = Check::Running;
        let def = f.def.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = bg.spawn(async move { crate::doctor::check_agent(&def, &program) }).await;
            this.update(cx, |this, cx| {
                if let Some(f) = this.agents.get_mut(ix) {
                    f.check = match result {
                        Ok(s) => Check::Ok(s),
                        Err(e) => Check::Failed(e),
                    };
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn done(&mut self, cx: &mut Context<Self>) {
        let agents = self.agents.iter().filter(|f| f.enabled).map(|f| f.def.clone()).collect();
        cx.emit(SetupEvent::Done(Choices { agents, layout: self.layout.clone(), vim: self.vim, theme: self.theme.clone(), accent: self.accent }));
    }
}

/// A small live sample of a theme: a sidebar with two agents, highlighted
/// code, and one glowing file row (handoff section 12a).
fn preview(t: &Theme, look: &Look) -> gpui::Div {
    let c = |rgba| look.c(rgba);
    let syn = |name: &str| c(t.syntax_style(name).color.unwrap_or(t.editor.text));
    let mut glow = c(t.accents[0].base);
    glow.a = 0.22;
    let code = |parts: Vec<(&'static str, gpui::Hsla)>| div().flex().children(parts.into_iter().map(|(s, col)| div().text_color(col).child(s)));
    div()
        .w(look.z(168.))
        .h(look.z(86.))
        .flex()
        .rounded(look.z(5.))
        .overflow_hidden()
        .border_1()
        .border_color(c(t.ui.border))
        .bg(c(t.ui.editor_bg))
        .child(
            div()
                .w(look.z(52.))
                .h_full()
                .bg(c(t.ui.sidebar_bg))
                .p(look.z(4.))
                .flex()
                .flex_col()
                .gap(look.z(3.))
                .child(div().h(look.z(9.)).rounded(look.z(2.)).bg(c(t.accents[0].base)))
                .child(div().h(look.z(9.)).rounded(look.z(2.)).border_1().border_color(c(t.accents[1].base)))
                .child(div().mt(look.z(4.)).h(look.z(7.)).rounded(look.z(1.)).bg(glow))
                .child(div().h(look.z(7.)).w(look.z(30.)).rounded(look.z(1.)).bg(c(t.ui.text_muted)).opacity(0.35)),
        )
        .child(
            div()
                .flex_1()
                .p(look.z(6.))
                .flex()
                .flex_col()
                .gap(look.z(1.))
                .font_family(look.code_font.clone())
                .text_size(look.z(8.5))
                .child(code(vec![("fn ", syn("keyword")), ("greet", syn("function")), ("() {", c(t.editor.text))]))
                .child(code(vec![("  let ", syn("keyword")), ("msg", c(t.editor.text)), (" = ", syn("operator")), ("\"hi\"", syn("string"))]))
                .child(code(vec![("  // ", syn("comment")), ("agent edit", syn("comment"))]))
                .child(code(vec![("}", c(t.editor.text))])),
        )
}

impl Render for SetupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let heading = |s: &'static str| div().mt(look.z(14.)).mb(look.z(6.)).text_size(look.text(-1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child(s);
        let pill = |id: (&'static str, usize), label: String, on: bool| {
            div()
                .id(id)
                .px(look.z(10.))
                .py(look.z(4.))
                .rounded(look.z(5.))
                .border_1()
                .cursor_pointer()
                .text_size(look.text(-1.))
                .border_color(look.c(if on { ui.focus_ring } else { ui.border }))
                .when(on, |d| d.bg(look.c(ui.active_bg)))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .text_color(look.c(if on { ui.text } else { ui.text_secondary }))
                .child(label)
        };
        // 1. AI accounts (decision 45): install, sign in, then which agents.
        let primary = |id: (&'static str, usize), label: String| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(12.))
                .py(look.z(4.))
                .rounded(look.z(5.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .bg(look.c(ui.button_primary_bg))
                .text_color(look.c(ui.button_primary_text))
                .child(label)
        };
        let note = |text: String, color| div().flex_1().min_w(px(0.)).truncate().text_size(look.text(-2.)).text_color(look.c(color)).child(text);
        let step = |n: &'static str, title: &'static str, done: bool| {
            div()
                .flex()
                .items_center()
                .gap(look.z(8.))
                .py(look.z(3.))
                .child(
                    div()
                        .flex_none()
                        .size(look.z(16.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(look.text(-3.))
                        .border_1()
                        .border_color(look.c(if done { ui.success } else { ui.border_strong }))
                        .text_color(look.c(if done { ui.success } else { ui.text_secondary }))
                        .child(if done { "✓" } else { n }),
                )
                .child(div().w(look.z(140.)).flex_none().text_size(look.text(-1.)).text_color(look.c(ui.text)).child(title))
        };
        let agent_row = |i: usize, f: &Found| {
            let status = match (&f.path, &f.check) {
                (None, _) => format!("needs {}", f.def.command),
                (Some(_), Check::NotRun) => f.def.command.clone(),
                (Some(_), Check::Running) => "checking…".into(),
                (Some(_), Check::Ok(s)) => format!("✓ {s}"),
                (Some(_), Check::Failed(e)) => format!("✕ {e}"),
            };
            div()
                .flex()
                .items_center()
                .gap(look.z(8.))
                .py(look.z(3.))
                .child(
                    div()
                        .id(("agent-on", i))
                        .flex_none()
                        .size(look.z(14.))
                        .rounded(look.z(3.))
                        .border_1()
                        .border_color(look.c(ui.border_strong))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(f.enabled, |d| d.bg(look.c(ui.button_primary_bg)).child(svg().path(assets::icon::CHECK).size(look.z(10.)).text_color(look.c(ui.button_primary_text))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(f) = this.agents.get_mut(i) {
                                f.enabled = !f.enabled;
                            }
                            cx.notify();
                        })),
                )
                .child(div().w(look.z(190.)).flex_none().text_size(look.text(-1.)).text_color(look.c(if f.path.is_some() { ui.text } else { ui.text_muted })).child(f.def.label.clone()))
                .child(div().flex_1().min_w(px(0.)).truncate().text_size(look.text(-2.)).text_color(look.c(match f.check {
                    Check::Failed(_) => ui.error,
                    Check::Ok(_) => ui.success,
                    _ => ui.text_muted,
                })).child(status))
                .when(f.path.is_some(), |d| {
                    d.child(pill(("agent-check", i), "Check".into(), false).on_click(cx.listener(move |this, _, _, cx| this.check(i, cx))))
                })
        };
        let cards = SERVICES.iter().enumerate().map(|(ix, svc)| {
            let missing = self.missing(ix);
            let installed = missing.is_empty();
            let waiting = self.waiting.filter(|(w, _)| *w == ix).map(|(_, wait)| wait);
            // ① Install
            let mut install = step("1", "Install", installed);
            install = if installed {
                install.child(note("Installed".into(), ui.success))
            } else if waiting == Some(Wait::Install) {
                install.child(note("Installing in a terminal… Setup comes back when it's done".into(), ui.text_muted))
            } else if !self.npm {
                install
                    .child(note("Needs Node.js first (it comes with npm)".into(), ui.text_muted))
                    .child(pill(("svc-recheck", ix), "Check again".into(), false).on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
                    .child(primary(("svc-node", ix), "Get Node.js".into()).on_click(cx.listener(|_, _, _, cx| cx.open_url("https://nodejs.org/en/download"))))
            } else {
                let packages = missing.iter().map(|(_, p)| *p).collect::<Vec<_>>().join(" ");
                install
                    .child(note(format!("Runs npm install -g {packages}"), ui.text_muted))
                    .child(primary(("svc-install", ix), "Install".into()).on_click(cx.listener(move |this, _, _, cx| this.install(ix, cx))))
            };
            // ② Sign in
            let mut sign = step("2", "Sign in", self.connected(ix));
            let cli_found = self.found(svc.cli);
            sign = match (&self.logins[ix], waiting) {
                _ if !cli_found => sign.child(note("Install first".into(), ui.text_muted)),
                (_, Some(Wait::SignIn)) => sign.child(note("Finish signing in in your browser… Setup comes back when it's done".into(), ui.text_muted)),
                (None, _) => sign.child(note("Checking…".into(), ui.text_muted)),
                (Some(Login::In(who)), _) => sign.child(note(who.clone(), ui.success)),
                (Some(Login::Out), _) => sign
                    .child(note("Not signed in. Opens your browser, where you may already be signed in.".into(), ui.text_muted))
                    .child(primary(("svc-sign-in", ix), "Sign in".into()).on_click(cx.listener(move |this, _, _, cx| this.sign_in(ix, cx)))),
                (Some(Login::Unknown(e)), _) => sign
                    .child(note(e.clone(), ui.error))
                    .child(pill(("svc-login-again", ix), "Check again".into(), false).on_click(cx.listener(move |this, _, _, cx| this.check_login(ix, cx))))
                    .child(pill(("svc-sign-in-anyway", ix), "Sign in".into(), false).on_click(cx.listener(move |this, _, _, cx| this.sign_in(ix, cx)))),
            };
            let rows = self.agents.iter().enumerate().filter(|(_, f)| svc.agents.contains(&f.def.id.as_str())).map(|(i, f)| agent_row(i, f)).collect::<Vec<_>>();
            div()
                .mb(look.z(8.))
                .p(look.z(10.))
                .rounded(look.z(6.))
                .border_1()
                .border_color(look.c(if self.connected(ix) { ui.success } else { ui.border }))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(look.z(8.))
                        .mb(look.z(4.))
                        .child(div().text_size(look.text(0.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child(svc.name))
                        .child(div().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(svc.plan)),
                )
                .child(install)
                .child(sign)
                .child(div().mt(look.z(4.)).text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("Use in Qur"))
                .children(rows)
        }).collect::<Vec<_>>();
        // Agents outside the two accounts (Aider): only when installed or chosen.
        let others = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, f)| !SERVICES.iter().any(|s| s.agents.contains(&f.def.id.as_str())) && (f.path.is_some() || f.enabled))
            .map(|(i, f)| agent_row(i, f))
            .collect::<Vec<_>>();
        let any_connected = (0..SERVICES.len()).any(|ix| self.connected(ix));
        // 3. Layout: drawn as the panes they give you.
        let layout_row = crate::dock::layout_cards(&self.layout, &look, cx, |this: &mut SetupView, key, cx| {
            this.layout = key.to_string();
            cx.notify();
        });
        // 4. Keys
        let keys_row = div()
            .flex()
            .gap(look.z(8.))
            .child(pill(("keys", 0), "VS Code".into(), !self.vim).on_click(cx.listener(|this, _, _, cx| {
                this.vim = false;
                cx.notify();
            })))
            .child(pill(("keys", 1), "VS Code + Vim mode".into(), self.vim).on_click(cx.listener(|this, _, _, cx| {
                this.vim = true;
                cx.notify();
            })));
        // 4b. Accent: the theme's palette in its own order (the look shown is
        // rotated by the accent being previewed).
        let mut palette = look.theme.accents;
        palette.rotate_right(self.accent);
        let accent_row = div().flex().gap(look.z(10.)).children(palette.iter().enumerate().map(|(i, a)| {
            let on = i == self.accent;
            div()
                .id(("setup-accent", i))
                .size(look.z(24.))
                .rounded_full()
                .cursor_pointer()
                .bg(look.c(a.base))
                .border_2()
                .border_color(look.c(if on { ui.text } else { ui.overlay_bg }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.accent = i;
                    cx.emit(SetupEvent::PreviewAccent(i));
                    cx.notify();
                }))
        }));
        // 5. Theme
        let themes: Vec<Arc<Theme>> = Registry::global().all().to_vec();
        let auto_on = self.theme == qur_config::THEME_AUTO;
        let mut grid: Vec<gpui::AnyElement> = vec![div()
            .id("theme-auto")
            .flex()
            .flex_col()
            .gap(look.z(4.))
            .cursor_pointer()
            .child(
                div()
                    .w(look.z(168.))
                    .h(look.z(86.))
                    .rounded(look.z(5.))
                    .border_1()
                    .border_color(look.c(if auto_on { ui.focus_ring } else { ui.border }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(look.text(-1.))
                    .text_color(look.c(ui.text_secondary))
                    .child("Follows macOS"),
            )
            .child(div().text_size(look.text(-2.)).text_color(look.c(if auto_on { ui.text } else { ui.text_muted })).child("Auto"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.theme = qur_config::THEME_AUTO.into();
                cx.emit(SetupEvent::PreviewTheme(this.theme.clone()));
                cx.notify();
            }))
            .into_any_element()];
        for (i, t) in themes.iter().enumerate() {
            let on = self.theme == t.name;
            let name = t.name.clone();
            grid.push(
                div()
                    .id(("theme", i))
                    .flex()
                    .flex_col()
                    .gap(look.z(4.))
                    .cursor_pointer()
                    .child(preview(t, &look).when(on, |d| d.border_2().border_color(look.c(ui.focus_ring))))
                    .child(div().text_size(look.text(-2.)).text_color(look.c(if on { ui.text } else { ui.text_muted })).child(t.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.theme = name.clone();
                        cx.emit(SetupEvent::PreviewTheme(name.clone()));
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        div()
            .id("setup")
            .key_context("Setup")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &crate::commands::setup::Done, _, cx| this.done(cx)))
            .on_action(cx.listener(|_, _: &crate::commands::setup::Cancel, _, cx| cx.emit(SetupEvent::Cancel)))
            .occlude()
            .w(look.z(760.))
            .max_h(look.z(640.))
            .overflow_y_scroll()
            .p(look.z(20.))
            .flex()
            .flex_col()
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.z(2.))
                    .child(
                        svg()
                            .path(if look.logo_white() { "brand/svg/qur-horizontal-white.svg" } else { "brand/svg/qur-horizontal-black.svg" })
                            .w(look.z(96.))
                            .h(look.z(32.))
                            // Pure black or white only: the logo is never tinted.
                            .text_color(if look.logo_white() { gpui::white() } else { gpui::black() }),
                    )
                    .child(div().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("by PEUR Technologies")),
            )
            .child(heading("Connect an AI account"))
            .child(div().mb(look.z(6.)).text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("Qur's agents run on a subscription you already have. No API keys, nothing extra to pay. Connect at least one."))
            .children(cards)
            .when(!others.is_empty(), |d| d.child(heading("Other agents")).children(others))
            .child(heading("Agent stage layout"))
            .child(layout_row)
            .child(heading("Keys"))
            .child(keys_row)
            .child(heading("Accent color"))
            .child(accent_row)
            .child(heading("Theme"))
            .child(div().flex().flex_wrap().gap(look.z(12.)).children(grid))
            .child(
                div()
                    .mt(look.z(18.))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .child(div().flex_1().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(if any_connected {
                        "Saved to config.toml and agents.toml; edit them any time. Reopen with Qur > Setup…"
                    } else {
                        "No AI account connected yet: agents won't run until you do. Connect one any time from Qur > Setup…"
                    }))
                    .child(pill(("setup-skip", 0), "Cancel".into(), false).on_click(cx.listener(|_, _, _, cx| cx.emit(SetupEvent::Cancel))))
                    .child(
                        div()
                            .id("setup-done")
                            .px(look.z(14.))
                            .py(look.z(5.))
                            .rounded(look.z(5.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .bg(look.c(ui.button_primary_bg))
                            .text_color(look.c(ui.button_primary_text))
                            .child(if any_connected { "Done" } else { "Skip for now" })
                            .on_click(cx.listener(|this, _, _, cx| this.done(cx))),
                    ),
            )
    }
}
