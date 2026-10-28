//! Settings (Qur > Settings…, Cmd+,): one screen with a section list on the
//! left. **Agents**: AI accounts to connect (install, sign in, agents found on
//! PATH with a Check; decision 45). **Appearance**: theme (this project or all),
//! accent, transparency, top bar, traffic lights. **Editor**: Vim mode, keys.
//! **Layout**: the agent stage presets as diagrams. **Files**: the settings
//! files themselves.
//!
//! Every change applies at once and is saved by the workspace through the same
//! code paths as the menu commands and popovers (events here, or the menu's own
//! actions dispatched), so nothing writes a setting twice. The workspace pushes
//! the values in effect back (`set_current`) after each change.
//!
//! On a brand-new home it opens by itself as the first-run welcome, with the
//! logo, Skip for now / Done, and the agents ticked for you saved on close.
//!
//! Row helpers (`section`, `row`, `pill`, `link`, `check_box`, `slider`) are
//! shared so new rows stay one-liners.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Bounds, Context, DispatchPhase, ElementId, Entity, EventEmitter, FocusHandle, Hsla, MouseButton, Pixels, ScrollHandle, SharedString, Stateful, Task, Window, canvas, div, prelude::*, px, svg,
};
use qur_agents::{AgentDef, Mode};
use qur_theme::{Registry, Rgba, Theme};

use crate::assets;
use crate::commands::qur;
use crate::doctor::Login;
use crate::input::{InputEvent, TextInput};
use crate::look::Look;

/// Agents Qur knows how to run (Codex since decision 44): (id, label, mode,
/// command, args, prompt). Others are added in Settings (Add agent) or in
/// agents.toml by hand.
type Known = (&'static str, &'static str, Mode, &'static str, &'static [&'static str], Option<&'static str>);
pub const KNOWN: &[Known] = &[
    ("claude", "Claude Code", Mode::Acp, "claude-agent-acp", &[], None),
    ("claude-cli", "Claude Code (terminal)", Mode::Terminal, "claude", &[], Some("arg")),
    // Zed's ACP adapter for OpenAI's Codex CLI; signs in with ChatGPT.
    ("codex", "Codex", Mode::Acp, "codex-acp", &[], None),
    ("codex-cli", "Codex (terminal)", Mode::Terminal, "codex", &[], Some("arg")),
    ("aider", "Aider (terminal)", Mode::Terminal, "aider", &[], Some("--message")),
    // Native ACP; `--acp` is stable from @google/gemini-cli 0.50.0.
    ("gemini", "Gemini CLI", Mode::Acp, "gemini", &["--acp"], None),
    ("opencode", "opencode", Mode::Acp, "opencode", &["acp"], None),
    // ACP in public preview; signs in with a GitHub Copilot plan.
    ("copilot", "GitHub Copilot", Mode::Acp, "copilot", &["--acp"], None),
    // Cursor's CLI installs as `agent` (~/.local/bin); sign in with `agent login`.
    ("cursor", "Cursor", Mode::Acp, "agent", &["acp"], None),
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

/// The section list, top to bottom (each is one child of the scrolled column).
pub const SECTIONS: [&str; 5] = ["Agents", "Appearance", "Editor", "Layout", "Files"];

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

/// The settings in effect, as the workspace sees them (previews included).
#[derive(Clone, Debug, PartialEq)]
pub struct Current {
    /// The project's name; empty in the empty window.
    pub project: String,
    pub theme: String,
    pub theme_is_project: bool,
    pub opacity: f32,
    pub blur: bool,
    pub window_is_project: bool,
    /// Which of the theme's accents leads, and that accent before tuning.
    pub accent: usize,
    pub accent_base: Rgba,
    pub hue: f32,
    pub brightness: f32,
    pub vim: bool,
    pub zen: bool,
    pub traffic_lights: String,
    /// Window gaps (`[ui] gaps`) and their size (`[ui] gap`).
    pub gaps: bool,
    pub gap: f32,
    /// `[ui] sidebar` ("show", "auto", "hidden") and `sidebar_side`.
    pub sidebar: String,
    pub sidebar_side: String,
    /// "A", "B", "C", or "yours" for a stage arranged by hand.
    pub layout: String,
}

/// A slider on this screen. Add a variant for a new one and handle it in
/// `SetupView::slide`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slider {
    Opacity,
    Hue,
    Brightness,
}

pub enum SetupEvent {
    /// Install or sign in: Setup steps aside while this runs in a terminal.
    RunInTerminal { label: String, command: String },
    /// That step finished: show Setup again.
    Ready(String),
    /// The agents ticked (never empty), and the ids of every agent listed:
    /// agents.toml becomes `chosen` plus the entries Settings never listed.
    Agents { chosen: Vec<AgentDef>, listed: Vec<String> },
    /// Theme picked, for this project or all (decision 13).
    Theme { name: String, project_scope: bool },
    /// Opacity and blur; `save` once the slider is let go (or blur clicked).
    Opacity { project_scope: bool, opacity: f32, blur: bool, save: bool },
    Accent(usize),
    /// Accent color and brightness sliders; `save` on release.
    Tune { hue: f32, brightness: f32, save: bool },
    TrafficLights(String),
    /// Import another terminal's settings into `[terminal]` (`qur_config::import`).
    ImportTerminal(&'static str),
    /// Window gaps on or off, and the gap (saved at once).
    Gaps { on: bool, gap: f32 },
    /// The sidebar's mode ("show", "auto", "hidden") and side (right or not).
    Sidebar(String),
    SidebarSide(bool),
    /// A stage layout preset ("A", "B", "C").
    Layout(String),
    Close,
}

pub struct SetupView {
    pub focus: FocusHandle,
    /// Opened by itself on a new home: the welcome, with Skip for now / Done.
    first_run: bool,
    agents: Vec<Found>,
    current: Current,
    /// Where Theme and Transparency changes go: this project or all projects.
    theme_project: bool,
    window_project: bool,
    /// Each service's sign-in; `None` while checking.
    logins: [Option<Login>; 2],
    npm: bool,
    waiting: Option<(usize, Wait)>,
    scroll: ScrollHandle,
    /// The section shown in the list (clicked, or scrolled to).
    section: usize,
    sliders: Rc<RefCell<HashMap<Slider, Bounds<Pixels>>>>,
    drag: Option<Slider>,
    /// Scrolled by hand: the list shows the section at the top.
    follow_scroll: bool,
    /// Add agent: name, command line, and how it runs.
    new_name: Entity<TextInput>,
    new_command: Entity<TextInput>,
    new_mode: Mode,
    _watch: Option<Task<()>>,
}

impl EventEmitter<SetupEvent> for SetupView {}

impl SetupView {
    pub fn new(current: Current, roster: &qur_agents::Roster, first_run: bool, cx: &mut Context<Self>) -> Self {
        let mut agents: Vec<Found> = KNOWN
            .iter()
            .map(|(id, label, mode, command, args, prompt)| {
                let existing = roster.get(id).cloned();
                let path = qur_agents::which(existing.as_ref().map_or(*command, |d| d.command.as_str()));
                Found {
                    // First run: only what's installed (agents.toml starts out
                    // listing Claude Code whether or not it is); signing in or
                    // installing later ticks the account's agent (`set_login`).
                    // Afterwards: what agents.toml lists.
                    enabled: if first_run { path.is_some() && (existing.is_some() || *id == "claude") } else { existing.is_some() },
                    def: existing.unwrap_or(AgentDef {
                        id: id.to_string(),
                        label: label.to_string(),
                        mode: *mode,
                        command: command.to_string(),
                        args: args.iter().map(|a| a.to_string()).collect(),
                        accent: None,
                        prompt: prompt.map(str::to_string),
                    }),
                    path,
                    check: Check::NotRun,
                }
            })
            .collect();
        // Agents added in Settings or by hand: listed under Other agents.
        agents.extend(roster.agents.iter().filter(|a| !KNOWN.iter().any(|k| k.0 == a.id)).map(|a| Found {
            path: qur_agents::which(&a.command),
            def: a.clone(),
            enabled: true,
            check: Check::NotRun,
        }));
        let mut this = SetupView {
            focus: cx.focus_handle(),
            first_run,
            agents,
            theme_project: current.theme_is_project && !first_run,
            window_project: current.window_is_project && !first_run,
            current,
            logins: [None, None],
            npm: false,
            waiting: None,
            scroll: ScrollHandle::new(),
            section: 0,
            sliders: Rc::default(),
            drag: None,
            follow_scroll: false,
            new_name: cx.new(|cx| TextInput::new("Name", cx)),
            new_command: cx.new(|cx| TextInput::new("Command, e.g. gemini --acp", cx)),
            new_mode: Mode::Acp,
            _watch: None,
        };
        for input in [this.new_name.clone(), this.new_command.clone()] {
            cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
                if let InputEvent::Confirm { .. } = ev {
                    this.add_agent(cx);
                }
            })
            .detach();
        }
        this.refresh(cx);
        this
    }

    /// The workspace's settings after a change (or a preview).
    pub fn set_current(&mut self, current: Current, cx: &mut Context<Self>) {
        if self.current != current {
            self.current = current;
            cx.notify();
        }
    }

    pub fn section_name(&self) -> &'static str {
        SECTIONS[self.section.min(SECTIONS.len() - 1)]
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
            self.save_agents(cx);
        }
        self.logins[ix] = Some(login);
        cx.notify();
    }

    fn connected(&self, ix: usize) -> bool {
        self.missing(ix).is_empty() && matches!(self.logins[ix], Some(Login::In(_)))
    }

    /// The ticked agents go to agents.toml (never an empty list).
    fn save_agents(&mut self, cx: &mut Context<Self>) {
        let agents: Vec<AgentDef> = self.agents.iter().filter(|f| f.enabled).map(|f| f.def.clone()).collect();
        if !agents.is_empty() {
            let listed = self.agents.iter().map(|f| f.def.id.clone()).collect();
            cx.emit(SetupEvent::Agents { chosen: agents, listed });
        }
    }

    /// Add agent: a new ticked row from the name and command fields.
    fn add_agent(&mut self, cx: &mut Context<Self>) {
        let taken = |id: &str| self.agents.iter().any(|f| f.def.id == id);
        let Some(def) = new_agent(&self.new_name.read(cx).text, &self.new_command.read(cx).text, self.new_mode, taken) else { return };
        self.agents.push(Found { path: qur_agents::which(&def.command), def, enabled: true, check: Check::NotRun });
        self.new_name.update(cx, |t, cx| t.set_text("", cx));
        self.new_command.update(cx, |t, cx| t.set_text("", cx));
        self.save_agents(cx);
        cx.notify();
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

    /// Done, Skip for now, Enter, Esc, Close: everything is saved already;
    /// the first run also keeps the agents ticked for you.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.first_run {
            self.save_agents(cx);
        }
        cx.emit(SetupEvent::Close);
    }

    /// Show a section (the list on the left).
    fn go_to(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.section = ix;
        self.follow_scroll = false;
        self.scroll.scroll_to_top_of_item(ix);
        cx.notify();
    }

    // -- sliders --------------------------------------------------------------

    /// A slider moved to `t` (0..=1): preview it, or save on release.
    fn slide(&mut self, which: Slider, t: f32, save: bool, cx: &mut Context<Self>) {
        let c = &self.current;
        match which {
            Slider::Opacity => {
                let min = qur_theme::MIN_OPACITY;
                let opacity = ((min + (1. - min) * t) * 100.).round() / 100.;
                cx.emit(SetupEvent::Opacity { project_scope: self.window_project, opacity, blur: c.blur, save });
            }
            Slider::Hue => cx.emit(SetupEvent::Tune { hue: (-180. + 360. * t).round(), brightness: c.brightness, save }),
            Slider::Brightness => cx.emit(SetupEvent::Tune { hue: c.hue, brightness: ((-1. + 2. * t) * 100.).round() / 100., save }),
        }
    }

    fn drag_to(&mut self, which: Slider, x: Pixels, save: bool, cx: &mut Context<Self>) {
        let Some(b) = self.sliders.borrow().get(&which).copied() else { return };
        if b.size.width <= px(0.) {
            return;
        }
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        self.slide(which, t, save, cx);
    }

    fn end_drag(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Some(which) = self.drag.take() {
            self.drag_to(which, x, true, cx);
        }
    }

    /// A track (plain, or painted with the colors it gives) and a thumb at
    /// `t`. Press, drag anywhere in the window, release to save.
    pub fn slider(&self, look: &Look, which: Slider, t: f32, colors: Option<Vec<Hsla>>, cx: &Context<Self>) -> Stateful<gpui::Div> {
        let ui = &look.theme.ui;
        let bounds = self.sliders.clone();
        let w = look.z(220.);
        let t = t.clamp(0., 1.);
        let track = match colors {
            Some(colors) => div()
                .absolute()
                .top(look.z(4.))
                .left_0()
                .w(w)
                .h(look.z(8.))
                .rounded(look.z(4.))
                .overflow_hidden()
                .flex()
                .children(colors.into_iter().map(|c| div().flex_1().h_full().bg(c))),
            None => div()
                .absolute()
                .top(look.z(7.))
                .left_0()
                .w(w)
                .h(look.z(2.))
                .bg(look.c(ui.border_strong))
                .child(div().w(w * t).h_full().bg(look.c(ui.text))),
        };
        div()
            .id(SharedString::from(format!("settings-slider-{which:?}")))
            .relative()
            .flex_none()
            .w(w)
            .h(look.z(16.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    this.drag = Some(which);
                    this.drag_to(which, ev.position.x, false, cx);
                }),
            )
            .child(canvas(move |b, _, _| _ = bounds.borrow_mut().insert(which, b), |_, _, _, _| {}).absolute().size_full())
            .child(track)
            .child(div().absolute().top(look.z(1.)).left(w * t - look.z(7.)).size(look.z(14.)).rounded_full().border_2().border_color(look.c(ui.text)).bg(look.c(ui.overlay_bg)))
            .children(crate::script::mark(move || format!("settings:slider:{which:?}")))
    }

    // -- sections -------------------------------------------------------------

    fn agents_section(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = look.theme.ui.clone();
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
                        .border_color(look.c(if done { ui.text_secondary } else { ui.border_strong }))
                        .text_color(look.c(if done { ui.text } else { ui.text_secondary }))
                        .child(if done { "✓" } else { n }),
                )
                .child(div().w(look.z(120.)).flex_none().text_size(look.text(-1.)).text_color(look.c(ui.text)).child(title))
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
                .child(check_box(look, ("agent-on", i), f.enabled).on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(f) = this.agents.get_mut(i) {
                        f.enabled = !f.enabled;
                    }
                    this.save_agents(cx);
                    cx.notify();
                })))
                .child(div().w(look.z(180.)).flex_none().text_size(look.text(-1.)).text_color(look.c(if f.path.is_some() { ui.text } else { ui.text_muted })).child(f.def.label.clone()))
                .child(div().flex_1().min_w(px(0.)).truncate().text_size(look.text(-2.)).text_color(look.c(match f.check {
                    Check::Failed(_) => ui.error,
                    Check::Ok(_) => ui.text_secondary,
                    _ => ui.text_muted,
                })).child(status))
                .when(f.path.is_some(), |d| d.child(pill(look, ("agent-check", i), "Check", false).on_click(cx.listener(move |this, _, _, cx| this.check(i, cx)))))
        };
        let cards = SERVICES
            .iter()
            .enumerate()
            .map(|(ix, svc)| {
                let missing = self.missing(ix);
                let installed = missing.is_empty();
                let waiting = self.waiting.filter(|(w, _)| *w == ix).map(|(_, wait)| wait);
                // ① Install
                let mut install = step("1", "Install", installed);
                install = if installed {
                    install.child(note("Installed".into(), ui.text_secondary))
                } else if waiting == Some(Wait::Install) {
                    install.child(note("Installing in a terminal… Settings come back when it's done".into(), ui.text_muted))
                } else if !self.npm {
                    install
                        .child(note("Needs Node.js first (it comes with npm)".into(), ui.text_muted))
                        .child(pill(look, ("svc-recheck", ix), "Check again", false).on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
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
                    (_, Some(Wait::SignIn)) => sign.child(note("Finish signing in in your browser… Settings come back when it's done".into(), ui.text_muted)),
                    (None, _) => sign.child(note("Checking…".into(), ui.text_muted)),
                    (Some(Login::In(who)), _) => sign.child(note(who.clone(), ui.text_secondary)),
                    (Some(Login::Out), _) => sign
                        .child(note("Not signed in. Opens your browser, where you may already be signed in.".into(), ui.text_muted))
                        .child(primary(("svc-sign-in", ix), "Sign in".into()).on_click(cx.listener(move |this, _, _, cx| this.sign_in(ix, cx)))),
                    (Some(Login::Unknown(e)), _) => sign
                        .child(note(e.clone(), ui.error))
                        .child(pill(look, ("svc-login-again", ix), "Check again", false).on_click(cx.listener(move |this, _, _, cx| this.check_login(ix, cx))))
                        .child(pill(look, ("svc-sign-in-anyway", ix), "Sign in", false).on_click(cx.listener(move |this, _, _, cx| this.sign_in(ix, cx)))),
                };
                let rows = self.agents.iter().enumerate().filter(|(_, f)| svc.agents.contains(&f.def.id.as_str())).map(|(i, f)| agent_row(i, f)).collect::<Vec<_>>();
                div()
                    .mb(look.z(8.))
                    .p(look.z(10.))
                    .rounded(look.z(6.))
                    .border_1()
                    .border_color(look.c(ui.border))
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
            })
            .collect::<Vec<_>>();
        // Agents outside the two accounts (Aider): only when installed or chosen.
        let others = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, f)| !SERVICES.iter().any(|s| s.agents.contains(&f.def.id.as_str())) && (f.path.is_some() || f.enabled || !KNOWN.iter().any(|k| k.0 == f.def.id)))
            .map(|(i, f)| agent_row(i, f))
            .collect::<Vec<_>>();
        section(look, "Agents")
            .child(div().mb(look.z(8.)).text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("Connect an AI account. Qur's agents run on a subscription you already have: no API keys, nothing extra to pay. Connect at least one."))
            .children(cards)
            .child(sub_heading(look, "Other agents"))
            .children(others)
            .child(self.add_agent_row(look, cx))
    }

    /// Add agent: name, command line, Structured or Terminal, Add.
    fn add_agent_row(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let field = |input: &Entity<TextInput>, w: f32| {
            let f = input.read(cx).focus.clone();
            div()
                .w(look.z(w))
                .flex_none()
                .px(look.z(6.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .border_1()
                .border_color(look.c(ui.border))
                .bg(look.c(ui.editor_bg))
                .text_size(look.text(-1.))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| window.focus(&f, cx))
                .child(input.clone())
        };
        let mode = |id: &'static str, label: &'static str, m: Mode| {
            pill(look, id, label, self.new_mode == m).on_click(cx.listener(move |this, _, _, cx| {
                this.new_mode = m;
                cx.notify();
            }))
        };
        div()
            .flex()
            .items_center()
            .gap(look.z(6.))
            .py(look.z(3.))
            .child(field(&self.new_name, 140.))
            .child(div().flex_1().min_w(px(0.)).child(field(&self.new_command, 220.).w_full()))
            .child(mode("add-agent-acp", "Structured", Mode::Acp))
            .child(mode("add-agent-terminal", "Terminal", Mode::Terminal))
            .child(pill(look, "add-agent", "Add", false).on_click(cx.listener(|this, _, _, cx| this.add_agent(cx))))
    }

    fn appearance_section(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = look.theme.ui.clone();
        let c = &self.current;
        // Theme: scope, Use Default, then the themes as live previews.
        let theme_scope = self.scope_pills(look, "theme-scope", self.theme_project, |t, p| t.theme_project = p, cx).when(c.theme_is_project, |d| {
            d.child(link(look, "theme-default", "Use Default Theme for This Project").relative().children(crate::script::mark(|| "settings:theme-default".to_string())).on_click(cx.listener(|_, _, window, cx| window.dispatch_action(Box::new(qur::UseDefaultTheme), cx))))
        });
        let small = Look { zoom: look.zoom * 0.75, ..look.clone() };
        let themes: Vec<Arc<Theme>> = Registry::global().all().to_vec();
        let card = |id: ElementId, mark: String, name: String, on: bool, body: gpui::Div| {
            div()
                .id(id)
                .relative()
                .flex()
                .flex_col()
                .gap(look.z(3.))
                .cursor_pointer()
                .child(body.when(on, |d| d.border_2().border_color(look.c(ui.focus_ring))))
                .child(div().text_size(look.text(-3.)).text_color(look.c(if on { ui.text } else { ui.text_muted })).child(name.clone()))
                .children(crate::script::mark(move || format!("settings:theme:{mark}")))
                .on_click(cx.listener(move |this, _, _, cx| cx.emit(SetupEvent::Theme { name: name.clone(), project_scope: this.theme_project })))
        };
        let auto = div()
            .w(small.z(168.))
            .h(small.z(86.))
            .rounded(small.z(5.))
            .border_1()
            .border_color(look.c(ui.border))
            .flex()
            .items_center()
            .justify_center()
            .text_size(look.text(-2.))
            .text_color(look.c(ui.text_secondary))
            .child("Follows macOS");
        let mut grid = vec![card("theme-auto".into(), "Auto".into(), qur_config::THEME_AUTO.into(), c.theme == qur_config::THEME_AUTO, auto)];
        for (i, t) in themes.iter().enumerate() {
            grid.push(card(("theme", i).into(), i.to_string(), t.name.clone(), c.theme == t.name, preview(t, &small)));
        }
        // Accent: the theme's palette in its own order (the look is rotated
        // by the accent in use), then its color and brightness.
        let mut palette = look.theme.accents;
        palette.rotate_right(c.accent);
        let swatches = div().flex().gap(look.z(10.)).children(palette.iter().enumerate().map(|(i, a)| {
            let on = i == c.accent;
            div()
                .id(("setup-accent", i))
                .size(look.z(22.))
                .rounded_full()
                .cursor_pointer()
                .bg(look.c(a.base))
                .border_2()
                .border_color(look.c(if on { ui.text } else { ui.overlay_bg }))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(SetupEvent::Accent(i))))
        }));
        let (hue, brightness, base) = (c.hue, c.brightness, c.accent_base);
        let steps = |f: &dyn Fn(f32) -> Rgba| (0..24).map(|i| look.c(f(i as f32 / 23.))).collect::<Vec<_>>();
        let value = |s: String| div().w(look.z(48.)).text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(s);
        let labeled = |name: &'static str, el: Stateful<gpui::Div>, v: String| {
            div().flex().items_center().gap(look.z(10.)).child(div().w(look.z(70.)).text_size(look.text(-2.)).text_color(look.c(ui.text_secondary)).child(name)).child(el).child(value(v))
        };
        let accent = div()
            .flex()
            .flex_col()
            .gap(look.z(8.))
            .child(swatches)
            .child(labeled("Color", self.slider(look, Slider::Hue, (hue + 180.) / 360., Some(steps(&|t| base.tuned(-180. + 360. * t, brightness))), cx), format!("{hue:+.0}°")))
            .child(labeled("Brightness", self.slider(look, Slider::Brightness, (brightness + 1.) / 2., Some(steps(&|t| base.tuned(hue, -1. + 2. * t))), cx), format!("{:+.0}%", brightness * 100.)))
            .when(hue != 0. || brightness != 0., |d| {
                d.child(link(look, "accent-reset", "Reset color and brightness").on_click(cx.listener(|_, _, _, cx| cx.emit(SetupEvent::Tune { hue: 0., brightness: 0., save: true }))))
            });
        // Transparency: the same 60–100% range as the popover (decision 38).
        let min = qur_theme::MIN_OPACITY;
        let t = if min < 1. { (c.opacity - min) / (1. - min) } else { 1. };
        let transparency = div()
            .flex()
            .flex_col()
            .gap(look.z(8.))
            .child(self.scope_pills(look, "window-scope", self.window_project, |t, p| t.window_project = p, cx))
            .child(labeled("Opacity", self.slider(look, Slider::Opacity, t, None, cx), format!("{}%", (c.opacity * 100.).round() as i32)))
            .child(
                div()
                    .id("settings-blur")
                    .relative()
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .cursor_pointer()
                    .text_size(look.text(-1.))
                    .text_color(look.c(ui.text))
                    .child(check_box(look, "settings-blur-box", c.blur))
                    .child("Blur the desktop behind")
                    .children(crate::script::mark(|| "settings:blur".to_string()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let c = &this.current;
                        cx.emit(SetupEvent::Opacity { project_scope: this.window_project, opacity: c.opacity, blur: !c.blur, save: true });
                    })),
            );
        let top_bar = choices(look, "top", &[("zen", "Zen: show at the top edge"), ("show", "Always show")], if c.zen { "zen" } else { "show" }, cx, |this, key, window, cx| {
            if (key == "zen") != this.current.zen {
                window.dispatch_action(Box::new(qur::ZenMode), cx);
            }
        });
        let sidebar = choices(look, "sidebar", &[("show", "Always show"), ("auto", "Auto-hide"), ("hidden", "Hidden")], &c.sidebar, cx, |_, key, _, cx| {
            cx.emit(SetupEvent::Sidebar(key.to_string()));
        });
        let sidebar_side = choices(look, "sidebar-side", &[("left", "Left"), ("right", "Right")], &c.sidebar_side, cx, |_, key, _, cx| {
            cx.emit(SetupEvent::SidebarSide(key == "right"));
        });
        let lights = choices(look, "lights", &[("auto", "Match theme"), ("color", "Color"), ("mono", "Monochrome")], &c.traffic_lights, cx, |_, key, _, cx| {
            cx.emit(SetupEvent::TrafficLights(key.to_string()));
        });
        // Off / Small / Medium / Large; a custom gap (from the popover) selects none.
        const SIZES: [(&str, &str, f32); 3] = [("4", "Small", 4.0), ("8", "Medium", 8.0), ("16", "Large", 16.0)];
        let gaps_key = if !c.gaps { "off".to_string() } else { SIZES.iter().find(|s| s.2 == c.gap).map_or(String::new(), |s| s.0.to_string()) };
        let gaps = choices(look, "gaps", &[("off", "Off"), ("4", "Small"), ("8", "Medium"), ("16", "Large")], &gaps_key, cx, |this, key, _, cx| {
            let gap = SIZES.iter().find(|s| s.0 == key).map_or(this.current.gap, |s| s.2);
            cx.emit(SetupEvent::Gaps { on: key != "off", gap });
        });
        section(look, "Appearance")
            .child(row(look, "Theme", Some("For this project or all of them"), theme_scope))
            .child(div().mt(look.z(4.)).mb(look.z(6.)).flex().flex_wrap().gap(look.z(10.)).children(grid))
            .child(row(look, "Accent color", Some("Agents take the colors in order from here"), accent))
            .child(row(look, "Transparency", Some("60–100%, live"), transparency))
            .child(row(look, "Gaps", Some("Space between panes; more in View > Appearance > Gaps…"), gaps))
            .child(row(look, "Top bar", None, top_bar))
            .child(row(look, "Sidebar", Some("Auto-hide slides it in at the window's edge; drag its section headers to reorder them"), sidebar))
            .child(row(look, "Sidebar side", None, sidebar_side))
            .child(row(look, "Traffic lights in full screen", None, lights))
    }

    /// This project / All projects, for Theme and Transparency (decision 13).
    /// None in the empty window (no project): changes there go to all projects.
    fn scope_pills(&self, look: &Look, id: &'static str, project: bool, set: fn(&mut SetupView, bool), cx: &Context<Self>) -> gpui::Div {
        if self.current.project.is_empty() {
            return div();
        }
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.z(6.))
            .child(pill(look, (id, 0usize), format!("This project ({})", self.current.project), project).children(crate::script::mark(move || format!("settings:{id}:project"))).on_click(cx.listener(move |this, _, _, cx| {
                set(this, true);
                cx.notify();
            })))
            .child(pill(look, (id, 1usize), "All projects", !project).children(crate::script::mark(move || format!("settings:{id}:all"))).on_click(cx.listener(move |this, _, _, cx| {
                set(this, false);
                cx.notify();
            })))
    }

    fn editor_section(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = look.theme.ui.clone();
        let vim = choices(look, "vim", &[("off", "Off"), ("on", "On")], if self.current.vim { "on" } else { "off" }, cx, |this, key, window, cx| {
            if (key == "on") != this.current.vim {
                window.dispatch_action(Box::new(qur::VimMode), cx);
            }
        });
        let keys = div()
            .flex()
            .items_center()
            .gap(look.z(10.))
            .child(div().text_size(look.text(-1.)).text_color(look.c(ui.text)).child("VS Code"))
            .child(link(look, "keys-keymap", "Rebind in keymap.toml…").on_click(cx.listener(|this, _, window, cx| this.open_file(Box::new(qur::OpenKeymap), window, cx))));
        section(look, "Editor")
            .child(row(look, "Vim mode", Some("In the code modal: Esc for normal mode"), vim))
            .child(row(look, "Keys", None, keys))
            .child(row(look, "Terminal settings", Some("Font, colors, cursor and habits from the terminal you use now"), self.import_links(look, cx)))
    }

    /// One "Import from …" per terminal whose config is on this Mac.
    fn import_links(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let found: Vec<_> = qur_config::import::SOURCES.iter().filter(|(id, _)| qur_config::import::find(id).is_some()).collect();
        let d = div().flex().items_center().gap(look.z(10.));
        if found.is_empty() {
            return d.child(div().text_size(look.text(-1.)).text_color(look.c(ui.text_muted)).child("No Ghostty, kitty or Alacritty config found"));
        }
        d.children(found.into_iter().map(|&(id, name)| {
            link(look, SharedString::from(format!("import-{id}")), format!("Import from {name}")).on_click(cx.listener(move |_, _, _, cx| cx.emit(SetupEvent::ImportTerminal(id))))
        }))
    }

    fn layout_section(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let cards = crate::dock::layout_cards(&self.current.layout, look, cx, |_: &mut SetupView, key, cx| {
            if matches!(key, "A" | "B" | "C") {
                cx.emit(SetupEvent::Layout(key.to_string()));
            }
        });
        section(look, "Layout")
            .child(div().mb(look.z(8.)).text_size(look.text(-2.)).text_color(look.c(look.theme.ui.text_muted)).child("The agent stage. Then drag tabs and dividers to make it yours; it's saved as you go."))
            .child(cards)
    }

    fn files_section(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = look.theme.ui.clone();
        let files: [(&'static str, Box<dyn gpui::Action>); 3] =
            [("config.toml", Box::new(qur::OpenConfig)), ("keymap.toml", Box::new(qur::OpenKeymap)), ("agents.toml", Box::new(qur::OpenAgents))];
        section(look, "Files")
            .child(div().mb(look.z(6.)).text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("Everything here is saved in these files, in ~/Library/Application Support/Qur."))
            .child(div().flex().gap(look.z(16.)).children(files.into_iter().map(|(name, action)| {
                link(look, SharedString::from(format!("file-{name}")), format!("Open {name}"))
                    .relative()
                    .children(crate::script::mark(move || format!("settings:file:{name}")))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_file(action.boxed_clone(), window, cx)))
            })))
    }

    /// Close, then open a settings file in the code modal (the menu's command).
    fn open_file(&mut self, action: Box<dyn gpui::Action>, window: &mut Window, cx: &mut Context<Self>) {
        self.close(cx);
        window.dispatch_action(action, cx);
    }
}

// -- row helpers (shared by every section) -----------------------------------

/// A section: its title, then its rows.
pub fn section(look: &Look, title: &'static str) -> gpui::Div {
    let ui = &look.theme.ui;
    div()
        .flex()
        .flex_col()
        .pb(look.z(18.))
        .child(div().pb(look.z(8.)).text_size(look.text(1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child(title))
}

fn sub_heading(look: &Look, title: &'static str) -> gpui::Div {
    div().mt(look.z(8.)).mb(look.z(4.)).text_size(look.text(-1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(look.theme.ui.text)).child(title)
}

/// Add agent's row as an agents.toml entry: `None` without a command. The
/// name defaults to the command; the id is the name's slug, made unique.
fn new_agent(name: &str, command_line: &str, mode: Mode, taken: impl Fn(&str) -> bool) -> Option<AgentDef> {
    // shortcut: split on spaces, no quoting; agents.toml takes args with spaces.
    let mut words = command_line.split_whitespace().map(str::to_string);
    let command = words.next()?;
    let label = if name.trim().is_empty() { command.clone() } else { name.trim().to_string() };
    let base = qur_worktree::slugify(&label);
    let id = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|id| !taken(id))?;
    Some(AgentDef { id, label, mode, command, args: words.collect(), accent: None, prompt: None })
}

/// One setting: its name (and a hint) on the left, the control on the right.
pub fn row(look: &Look, label: &'static str, hint: Option<&'static str>, control: impl IntoElement) -> gpui::Div {
    let ui = &look.theme.ui;
    div()
        .flex()
        .items_start()
        .gap(look.z(12.))
        .py(look.z(7.))
        .border_t_1()
        .border_color(look.c(ui.border))
        .child(
            div()
                .w(look.z(150.))
                .flex_none()
                .flex()
                .flex_col()
                .gap(look.z(2.))
                .child(div().text_size(look.text(-1.)).text_color(look.c(ui.text)).child(label))
                .children(hint.map(|h| div().text_size(look.text(-3.)).text_color(look.c(ui.text_muted)).child(h))),
        )
        .child(div().flex_1().min_w(px(0.)).child(control))
}

/// A choice button (several make a segmented row).
pub fn pill(look: &Look, id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<gpui::Div> {
    let ui = &look.theme.ui;
    div()
        .id(id)
        .relative()
        .flex_none()
        .px(look.z(10.))
        .py(look.z(3.))
        .rounded(look.z(5.))
        .border_1()
        .cursor_pointer()
        .text_size(look.text(-1.))
        .border_color(look.c(if on { ui.focus_ring } else { ui.border }))
        .when(on, |d| d.bg(look.c(ui.active_bg)))
        .hover(|d| d.bg(look.c(ui.hover_bg)))
        .text_color(look.c(if on { ui.text } else { ui.text_secondary }))
        .child(label.into())
}

/// One of `options` (key, label), as pills; `on_pick` gets the key.
pub fn choices<V: 'static>(
    look: &Look,
    id: &'static str,
    options: &[(&'static str, &'static str)],
    selected: &str,
    cx: &Context<V>,
    on_pick: impl Fn(&mut V, &'static str, &mut Window, &mut Context<V>) + Clone + 'static,
) -> gpui::Div {
    div().flex().flex_wrap().gap(look.z(6.)).children(options.iter().enumerate().map(|(i, &(key, label))| {
        let pick = on_pick.clone();
        pill(look, (id, i), label, selected == key)
            .children(crate::script::mark(move || format!("settings:{id}:{key}")))
            .on_click(cx.listener(move |this, _, window, cx| {
                pick(this, key, window, cx);
                cx.notify();
            }))
    }))
}

/// A text link (opens something, or a one-off action).
pub fn link(look: &Look, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<gpui::Div> {
    let ui = &look.theme.ui;
    div().id(id).flex_none().cursor_pointer().text_size(look.text(-2.)).text_color(look.c(ui.text_secondary)).hover(|d| d.text_color(look.c(ui.text))).child(label.into())
}

/// A tick box.
pub fn check_box(look: &Look, id: impl Into<ElementId>, on: bool) -> Stateful<gpui::Div> {
    let ui = &look.theme.ui;
    div()
        .id(id)
        .flex_none()
        .size(look.z(14.))
        .rounded(look.z(3.))
        .border_1()
        .border_color(look.c(ui.border_strong))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .when(on, |d| d.bg(look.c(ui.button_primary_bg)).child(svg().path(assets::icon::CHECK).size(look.z(10.)).text_color(look.c(ui.button_primary_text))))
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

/// The panel's size: up to 820 × 640 (zoomed), inside the window.
pub fn panel_size(look: &Look, viewport: gpui::Size<Pixels>) -> (Pixels, Pixels) {
    let fit = |want: Pixels, room: Pixels, floor: Pixels| if want < room { want } else if room > floor { room } else { floor };
    (fit(look.z(820.), viewport.width - look.z(32.), look.z(360.)), fit(look.z(640.), viewport.height - look.z(110.), look.z(240.)))
}

impl Render for SetupView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let (w, h) = panel_size(&look, window.viewport_size());
        if self.follow_scroll {
            // At the bottom, the last section (it may never reach the top).
            let at_end = self.scroll.max_offset().y > px(0.) && -self.scroll.offset().y >= self.scroll.max_offset().y - px(2.);
            self.section = if at_end { SECTIONS.len() - 1 } else { self.scroll.top_item().min(SECTIONS.len() - 1) };
        }
        // While a slider is held, follow the mouse anywhere in the window.
        let drag_listener = self.drag.is_some().then(|| {
            let view = cx.entity().downgrade();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let v = view.clone();
                    window.on_mouse_event(move |ev: &gpui::MouseMoveEvent, phase, _, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }
                        v.update(cx, |t, cx| match (t.drag, ev.pressed_button) {
                            (Some(which), Some(MouseButton::Left)) => t.drag_to(which, ev.position.x, false, cx),
                            (Some(_), _) => t.end_drag(ev.position.x, cx),
                            _ => {}
                        })
                        .ok();
                    });
                    let v = view.clone();
                    window.on_mouse_event(move |ev: &gpui::MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
                            v.update(cx, |t, cx| t.end_drag(ev.position.x, cx)).ok();
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });
        let sections = [
            self.agents_section(&look, cx),
            self.appearance_section(&look, cx),
            self.editor_section(&look, cx),
            self.layout_section(&look, cx),
            self.files_section(&look, cx),
        ];
        let nav = div().w(look.z(132.)).flex_none().flex().flex_col().gap(look.z(2.)).p(look.z(10.)).children(SECTIONS.iter().enumerate().map(|(i, name)| {
            let on = i == self.section;
            div()
                .id(("settings-nav", i))
                .relative()
                .px(look.z(8.))
                .py(look.z(4.))
                .rounded(look.z(5.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .text_color(look.c(if on { ui.text } else { ui.text_secondary }))
                .when(on, |d| d.bg(look.c(ui.active_bg)))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(*name)
                .children(crate::script::mark(move || format!("settings:nav:{name}")))
                .on_click(cx.listener(move |this, _, _, cx| this.go_to(i, cx)))
        }));
        let any_connected = (0..SERVICES.len()).any(|ix| self.connected(ix));
        let header = if self.first_run {
            div()
                .flex()
                .items_end()
                .gap(look.z(12.))
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
                .child(div().flex_1().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("Welcome. Connect an AI account, then make it yours. Reopen any time with Qur > Settings… (⌘,)."))
        } else {
            div()
                .flex()
                .items_center()
                .gap(look.z(8.))
                .child(div().flex_1().text_size(look.text(1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child("Settings"))
                .child(div().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("changes save as you make them · esc to close"))
                .child(
                    div()
                        .id("settings-close")
                        .relative()
                        .p(look.z(3.))
                        .rounded(look.z(4.))
                        .cursor_pointer()
                        .hover(|d| d.bg(look.c(ui.hover_bg)))
                        .child(svg().path(assets::icon::CLOSE).size(look.z(12.)).text_color(look.c(ui.text_secondary)))
                        .children(crate::script::mark(|| "settings:close".to_string()))
                        .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                )
        };
        let footer = self.first_run.then(|| {
            div()
                .flex()
                .items_center()
                .gap(look.z(8.))
                .px(look.z(16.))
                .py(look.z(10.))
                .border_t_1()
                .border_color(look.c(ui.border))
                .child(div().flex_1().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(if any_connected {
                    "Saved as you go to config.toml and agents.toml. Reopen with Qur > Settings… (⌘,)"
                } else {
                    "No AI account connected yet: agents won't run until you do. Connect one any time from Qur > Settings… (⌘,)"
                }))
                .child(
                    div()
                        .id("setup-done")
                        .relative()
                        .px(look.z(14.))
                        .py(look.z(5.))
                        .rounded(look.z(5.))
                        .cursor_pointer()
                        .text_size(look.text(-1.))
                        .bg(look.c(ui.button_primary_bg))
                        .text_color(look.c(ui.button_primary_text))
                        .child(if any_connected { "Done" } else { "Skip for now" })
                        .children(crate::script::mark(|| "settings:done".to_string()))
                        .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                )
        });
        div()
            .id("setup")
            .key_context("Setup")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &crate::commands::setup::Done, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &crate::commands::setup::Cancel, _, cx| this.close(cx)))
            .occlude()
            .w(w)
            .h(h)
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .children(drag_listener)
            .child(div().flex_none().px(look.z(16.)).py(look.z(12.)).border_b_1().border_color(look.c(ui.border)).child(header))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(nav)
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .px(look.z(16.))
                            .pt(look.z(12.))
                            // The list follows the scrolling.
                            .on_scroll(cx.listener(|this, _, _, cx| {
                                this.follow_scroll = true;
                                cx.notify();
                            }))
                            .children(sections),
                    ),
            )
            .children(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(zoom: f32) -> Look {
        Look {
            theme: Registry::global().resolve(&qur_theme::ThemeChoice::Auto, true),
            zoom,
            opacity: 1.0,
            blur: false,
            ui_font: "Inter".into(),
            code_font: "Menlo".into(),
            ui_size: 13.,
            code_size: 13.,
            line_height: 1.5,
            tiles: None,
            show_whitespace: false,
        }
    }

    /// The panel keeps its size in a big window, shrinks to fit a small one
    /// (it scrolls inside), scales with zoom, and never collapses.
    #[test]
    fn panel_fits_the_window() {
        let size = |w: f32, h: f32| gpui::size(px(w), px(h));
        assert_eq!(panel_size(&look(1.), size(1600., 1000.)), (px(820.), px(640.)));
        assert_eq!(panel_size(&look(1.), size(700., 600.)), (px(668.), px(490.)));
        assert_eq!(panel_size(&look(1.5), size(3000., 2000.)), (px(1230.), px(960.)));
        assert_eq!(panel_size(&look(1.), size(200., 100.)), (px(360.), px(240.)));
    }

    #[test]
    fn new_agent_parses_the_command_line() {
        let a = new_agent("  ", "gemini --acp --model x", Mode::Acp, |_| false).unwrap();
        assert_eq!((a.id.as_str(), a.label.as_str(), a.command.as_str()), ("gemini", "gemini", "gemini"));
        assert_eq!(a.args, ["--acp", "--model", "x"]);
        let b = new_agent("My Agent", "x", Mode::Terminal, |id| id == "my-agent").unwrap();
        assert_eq!(b.id, "my-agent-2");
        assert!(new_agent("Name", "   ", Mode::Acp, |_| false).is_none());
    }

    #[test]
    fn every_account_agent_is_known() {
        for svc in &SERVICES {
            for id in svc.agents {
                assert!(KNOWN.iter().any(|k| k.0 == id), "{id}");
            }
        }
        assert_eq!(SECTIONS.last(), Some(&"Files"));
    }
}
