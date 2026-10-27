//! The look and settings: applying them, writing settings files, the Settings
//! screen and the settings files' templates.

use super::*;

impl Workspace {
    // -- look (theme, transparency, zoom) -------------------------------------

    pub(super) fn apply_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let eff = self.effective();
        let name = self.preview_theme.clone().unwrap_or(eff.theme);
        let choice = if name == qur_config::THEME_AUTO { ThemeChoice::Auto } else { ThemeChoice::Named(name) };
        let theme = Registry::global().resolve(&choice, self.system_dark);
        // Your accent first ([ui] accent): agents take the palette in order,
        // tuned by the Accent Color sliders; focus outlines follow it.
        let k = self.preview_accent.unwrap_or(self.config.ui.accent) % 6;
        let (hue, brightness) = self.preview_tune.unwrap_or((self.config.ui.accent_hue, self.config.ui.accent_brightness));
        self.accent_base = theme.accents[k].base;
        let mut theme = theme.with_accent(k, hue, brightness);
        crate::terminal::override_colors(&mut theme.terminal);
        let theme = std::sync::Arc::new(theme);
        let min = qur_theme::MIN_OPACITY;
        let code_font = if self.config.editor.font_family.is_empty() { default_code_font(cx) } else { self.config.editor.font_family.clone() };
        let look = Look {
            opacity: eff.opacity.clamp(min, 1.0),
            blur: eff.blur,
            zoom: self.config.ui.zoom,
            ui_font: assets::UI_FONT.into(),
            code_font: code_font.into(),
            ui_size: self.config.ui.font_size,
            code_size: self.config.editor.font_size,
            line_height: self.config.editor.line_height,
            theme: theme.clone(),
            tiles: crate::look::Tiles::from_config(&self.config.ui),
            show_whitespace: self.config.editor.show_whitespace,
        };
        window.set_background_appearance(look.background_appearance());
        Look::set(window, look, cx);
        // This window's terminals answer color queries with its theme.
        self.terms.read(cx).set_colors(theme.terminal.clone());
        // Agent accents follow the theme's palette (unless agents.toml overrides).
        let n = theme.accents.len();
        let mut slot = 0;
        for a in &self.agents {
            let def = a.read(cx).def.clone();
            let accent = accent_for(&def, &theme.accents[slot % n]);
            a.update(cx, |a, cx| a.set_accent(accent, cx));
            slot += 1;
        }
        for t in &mut self.term_tasks {
            let def = self.roster.get(&t.record.agent).cloned();
            t.accent = def.map(|d| accent_for(&d, &theme.accents[slot % n])).unwrap_or(theme.accents[slot % n].base);
            let accent = t.accent;
            t.monitor.update(cx, |m, _| m.accent = accent);
            t.live.update(cx, |l, _| l.accent = accent);
            t.browser.update(cx, |b, _| b.accent = accent);
            slot += 1;
        }
        self.sync_settings(cx);
        window.refresh();
        cx.notify();
    }

    /// Re-read config.toml and this project's settings.
    pub fn reload_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (config, err) = Config::load();
        self.config = config;
        let term_err = crate::terminal::set_config(&self.config.terminal);
        self.terms.update(cx, |p, cx| p.apply_config(cx));
        Docks::load(&self.config, cx);
        self.project = ProjectSettings::load(&self.project_id);
        if let Some(e) = err.or(term_err).or_else(|| crate::hotkey::set(&self.config.keys.global).err()) {
            self.flash(e, cx);
        }
        let wrap = self.config.editor.soft_wrap;
        for b in self.buffers.loaded_vec() {
            b.update(cx, |b, cx| b.set_soft_wrap(wrap, cx));
            let vim = self.config.keys.vim;
            b.update(cx, |b, cx| b.set_vim(vim, cx));
        }
        self.apply_look(window, cx);
        self.sync_menus(cx);
    }

    pub(super) fn write_setting(&mut self, project_scope: bool, key: &str, value: Option<Value>, cx: &mut Context<Self>) {
        // The empty window has no project: its changes go to all projects.
        let path = if project_scope && !self.home { qur_config::project_settings_path(&self.project_id) } else { qur_config::config_path() };
        if let Err(e) = qur_config::write_key(&path, key, value) {
            self.flash(format!("Couldn't save {}: {e}", path.display()), cx);
        }
    }

    /// Tell every other window to re-read settings (a global value changed).
    pub(super) fn broadcast_reload(&self, cx: &mut Context<Self>) {
        let me = cx.entity();
        for handle in cx.windows() {
            if let Some(ws) = handle.downcast::<Workspace>() {
                let _ = ws.update(cx, |ws, window, cx| {
                    if cx.entity() != me {
                        ws.reload_settings(window, cx);
                    }
                });
            }
        }
    }

    fn set_zoom(&mut self, z: f32, window: &mut Window, cx: &mut Context<Self>) {
        let z = (z * 10.).round() / 10.;
        self.config.ui.zoom = z.clamp(qur_config::MIN_ZOOM, qur_config::MAX_ZOOM);
        self.write_setting(false, "ui.zoom", Some(self.config.ui.zoom.into()), cx);
        self.apply_look(window, cx);
        self.broadcast_reload(cx);
    }
    pub(super) fn zoom_in(&mut self, _: &qur::ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        let z = self.config.ui.zoom + 0.1;
        self.set_zoom(z, window, cx)
    }
    pub(super) fn zoom_out(&mut self, _: &qur::ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        let z = self.config.ui.zoom - 0.1;
        self.set_zoom(z, window, cx)
    }
    pub(super) fn zoom_reset(&mut self, _: &qur::ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1.0, window, cx)
    }

    pub(super) fn open_config(&mut self, _: &qur::OpenConfig, window: &mut Window, cx: &mut Context<Self>) {
        let path = qur_config::config_path();
        if !path.exists() {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(&path, CONFIG_TEMPLATE);
        }
        self.open_path(&path, window, cx);
    }
    pub(super) fn open_keymap(&mut self, _: &qur::OpenKeymap, window: &mut Window, cx: &mut Context<Self>) {
        let path = qur_config::keymap_path();
        if !path.exists() {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(&path, KEYMAP_TEMPLATE);
        }
        self.open_path(&path, window, cx);
    }

    /// Re-read keymap.toml (saved in Qur): rebinds keys app-wide, updates the
    /// menu shortcuts, and reports skipped entries (also in the log).
    /// Another terminal's settings into `[terminal]` (Settings > Editor).
    fn import_terminal(&mut self, source: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = qur_config::import::SOURCES.iter().find(|s| s.0 == source).map_or(source, |s| s.1);
        let Some(path) = qur_config::import::find(source) else { return self.flash(format!("No {name} config found"), cx) };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => return self.flash(format!("{}: {e}", path.display()), cx),
        };
        let keys = qur_config::import::terminal_keys(source, &text, path.parent().unwrap_or(&path));
        if keys.is_empty() {
            return self.flash(format!("Nothing in {} that Qur's terminal uses", path.display()), cx);
        }
        let n = keys.len();
        for (k, v) in keys {
            self.write_setting(false, &format!("terminal.{k}"), Some(v), cx);
        }
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
        self.flash(format!("Imported {n} settings from {name} into config.toml [terminal]"), cx);
    }

    pub fn reload_keymap(&mut self, cx: &mut Context<Self>) {
        let errors = commands::reload_keymap(cx);
        for e in &errors {
            crate::log(e);
        }
        self.sync_menus(cx);
        match errors.as_slice() {
            [] => self.flash("keymap.toml applied", cx),
            [one] => self.flash(one.clone(), cx),
            [first, rest @ ..] => self.flash(format!("{first} (and {} more, see Help > Show Logs)", rest.len()), cx),
        }
    }

    pub(super) fn open_agents(&mut self, _: &qur::OpenAgents, window: &mut Window, cx: &mut Context<Self>) {
        let _ = Roster::load(); // writes the defaults when missing
        self.open_path(&qur_agents::roster_path(), window, cx);
    }

    /// Keep a theme for this project (its projects/<id>.toml) or for all
    /// (config.toml, clearing this project's own); the picker and Settings.
    pub(super) fn save_theme(&mut self, name: &str, project_scope: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_theme = None;
        if project_scope {
            self.write_setting(true, "theme", Some(name.into()), cx);
        } else {
            // Clear first: the empty window's project scope is config.toml too.
            self.write_setting(true, "theme", None, cx);
            self.write_setting(false, "theme", Some(name.into()), cx);
        }
        self.reload_settings(window, cx);
        if !project_scope {
            self.broadcast_reload(cx);
        }
    }

    pub(super) fn use_default_theme(&mut self, _: &qur::UseDefaultTheme, window: &mut Window, cx: &mut Context<Self>) {
        self.write_setting(true, "theme", None, cx);
        self.reload_settings(window, cx);
        self.flash(format!("{} now uses the default theme", self.project_name()), cx);
    }

    /// Edit > Vim Mode (and Settings): on/off for every editor, saved as `[keys] vim`.
    pub(super) fn toggle_vim(&mut self, _: &qur::VimMode, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.config.keys.vim;
        self.write_setting(false, "keys.vim", Some(Value::Bool(on)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
        self.flash(if on { "Vim mode on (Esc for normal mode)" } else { "Vim mode off" }, cx);
    }

    /// View > Show Keep Awake in Sidebar, saved as `[ui] keep_awake`.
    pub(super) fn toggle_keep_awake(&mut self, _: &qur::ShowKeepAwake, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.config.ui.keep_awake;
        self.write_setting(false, "ui.keep_awake", Some(Value::Bool(on)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
    }

    /// Qur > Settings… (Cmd+,): the settings screen.
    pub(super) fn open_setup(&mut self, _: &qur::CheckSetup, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(false, window, cx);
    }

    /// The settings in effect, for the settings screen (previews included).
    fn settings_now(&self, cx: &App) -> setup_view::Current {
        let eff = self.effective();
        let preset = self.layout.as_str();
        let custom = Docks::get(cx, dock::Kind::Acp) != dock::Dock::preset(dock::Kind::Acp, preset) || Docks::get(cx, dock::Kind::Terminal) != dock::Dock::preset(dock::Kind::Terminal, preset);
        let (hue, brightness) = self.preview_tune.unwrap_or((self.config.ui.accent_hue, self.config.ui.accent_brightness));
        setup_view::Current {
            project: if self.home { String::new() } else { self.project_name() },
            theme: self.preview_theme.clone().unwrap_or(eff.theme),
            theme_is_project: eff.theme_is_project,
            opacity: eff.opacity.clamp(qur_theme::MIN_OPACITY, 1.0),
            blur: eff.blur,
            window_is_project: eff.window_is_project,
            accent: self.preview_accent.unwrap_or(self.config.ui.accent) % 6,
            accent_base: self.accent_base,
            hue,
            brightness,
            vim: self.config.keys.vim,
            global_key: self.config.keys.global.clone(),
            zen: self.config.ui.zen,
            traffic_lights: self.config.ui.traffic_lights.clone(),
            gaps: self.config.ui.gaps,
            gap: self.config.ui.gap,
            sidebar: self.config.ui.sidebar.clone(),
            sidebar_side: self.config.ui.sidebar_side.clone(),
            layout: if custom { "yours".into() } else { preset.into() },
        }
    }

    /// Show the settings screen what's in effect now (after any change).
    pub(super) fn sync_settings(&self, cx: &mut Context<Self>) {
        let open = match &self.overlay {
            Some(Overlay::Setup(v)) => Some(v.clone()),
            _ => None,
        };
        for v in open.iter().chain(self.parked_setup.iter()) {
            let now = self.settings_now(cx);
            v.update(cx, |v, cx| v.set_current(now, cx));
        }
    }

    /// The settings screen; `first_run` makes it the welcome (logo, Skip for
    /// now / Done). Every change in it goes through the same methods as the
    /// menu commands and popovers.
    pub(super) fn show_settings(&mut self, first_run: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Setup(v)) = &self.overlay {
            let f = v.read(cx).focus.clone();
            window.focus(&f, cx);
            return;
        }
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        // Back from an install or sign-in: the same screen, looked at again.
        if let Some(view) = self.parked_setup.take() {
            let now = self.settings_now(cx);
            view.update(cx, |v, cx| {
                v.set_current(now, cx);
                v.refresh(cx);
            });
            let f = view.read(cx).focus.clone();
            window.focus(&f, cx);
            self.overlay = Some(Overlay::Setup(view));
            cx.notify();
            return;
        }
        let now = self.settings_now(cx);
        let view = cx.new(|cx| SetupView::new(now, &self.roster, first_run, cx));
        let sub = cx.subscribe_in(&view, window, |this, view, ev: &SetupEvent, window, cx| match ev {
            // Install or sign in, in a terminal of its own; Settings step
            // aside and come back by themselves when the step has worked.
            SetupEvent::RunInTerminal { label, command } => {
                this.close_overlays(window, cx);
                this.parked_setup = Some(view.clone());
                let root = this.root.clone();
                let kind = Kind::Agent { task: label.clone(), command: command.clone() };
                let Some(tid) = this.terms.update(cx, |p, cx| p.open(&root, kind, label.clone(), cx)) else { return };
                this.shells.push(tid);
                this.select(Some(Selected::Shell(tid)), window, cx);
                this.flash(format!("{label}: Settings come back when it's done (or Qur > Settings…)"), cx);
            }
            SetupEvent::Ready(msg) => {
                if this.parked_setup.is_some() && this.overlay.is_none() {
                    this.show_settings(false, window, cx);
                }
                this.flash(msg.clone(), cx);
            }
            SetupEvent::Agents { chosen, listed } => this.save_agents(chosen.clone(), listed, cx),
            SetupEvent::Theme { name, project_scope } => this.save_theme(name, *project_scope, window, cx),
            SetupEvent::Opacity { project_scope, opacity, blur, save } => {
                this.set_opacity(*project_scope, *opacity, *blur, window, cx);
                if *save {
                    this.save_transparency(*project_scope, cx);
                }
            }
            SetupEvent::Accent(k) => this.set_accent(*k, window, cx),
            SetupEvent::Tune { hue, brightness, save } => {
                if *save {
                    this.save_tune((*hue, *brightness), window, cx);
                } else {
                    this.preview_tune = Some((*hue, *brightness));
                    this.apply_look(window, cx);
                }
            }
            SetupEvent::Gaps { on, gap } => {
                this.config.ui.gaps = *on;
                this.config.ui.gap = gap.round().clamp(qur_config::MIN_GAP, qur_config::MAX_GAP);
                this.write_setting(false, "ui.gaps", Some(Value::Bool(*on)), cx);
                this.write_setting(false, "ui.gap", Some(this.config.ui.gap.into()), cx);
                this.apply_look(window, cx);
                this.broadcast_reload(cx);
                this.sync_menus(cx);
                this.sync_settings(cx);
            }
            SetupEvent::Sidebar(mode) => this.set_sidebar_mode(mode, cx),
            SetupEvent::SidebarSide(right) => this.set_sidebar_side(*right, cx),
            SetupEvent::GlobalKey(key) => match crate::hotkey::set(key) {
                Ok(()) => {
                    this.write_setting(false, "keys.global", Some(key.as_str().into()), cx);
                    this.reload_settings(window, cx);
                    this.broadcast_reload(cx);
                    this.flash(if key.is_empty() { "Show or hide Qur: off".to_string() } else { format!("{key} now shows or hides Qur") }, cx);
                }
                Err(e) => {
                    let _ = crate::hotkey::set(&this.config.keys.global);
                    this.flash(e, cx);
                }
            },
            SetupEvent::ImportTerminal(source) => this.import_terminal(source, window, cx),
            SetupEvent::TrafficLights(key) => {
                this.write_setting(false, "ui.traffic_lights", Some(key.as_str().into()), cx);
                this.reload_settings(window, cx);
                this.broadcast_reload(cx);
            }
            SetupEvent::Layout(key) => this.set_layout(Layout::parse(key), cx),
            SetupEvent::Close => {
                this.parked_setup = None;
                this.close_overlays(window, cx);
            }
        });
        self._subs.push(sub);
        let f = view.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Setup(view));
        cx.notify();
    }

    /// The agents ticked in Settings, then any agents.toml entry Settings
    /// never listed. Listed but unticked ones are removed.
    fn save_agents(&mut self, chosen: Vec<AgentDef>, listed: &[String], cx: &mut Context<Self>) {
        // Re-read the file: entries added since Settings opened (by hand, or
        // from another window) weren't listed there, so they stay.
        let mut agents = chosen;
        let kept: Vec<_> = qur_agents::Roster::load().0.agents.into_iter().filter(|a| !listed.contains(&a.id) && !agents.iter().any(|c| c.id == a.id)).collect();
        agents.extend(kept);
        let roster = qur_agents::Roster { agents };
        if let Err(e) = roster.save() {
            self.flash(format!("Couldn't save agents.toml: {e}"), cx);
        }
        self.roster = roster;
    }

    /// A Gaps popover change, shown at once (saved on close).
    pub(super) fn set_gaps(&mut self, f: impl FnOnce(&mut qur_config::UiConfig), window: &mut Window, cx: &mut Context<Self>) {
        f(&mut self.config.ui);
        let u = &mut self.config.ui;
        u.gap = u.gap.round().clamp(qur_config::MIN_GAP, qur_config::MAX_GAP);
        u.radius = u.radius.round().clamp(0., qur_config::MAX_RADIUS);
        self.apply_look(window, cx);
    }

    pub(super) fn save_gaps(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Some(Overlay::Gaps)) {
            return;
        }
        self.gap_sliding = None;
        let u = self.config.ui.clone();
        self.write_setting(false, "ui.gaps", Some(u.gaps.into()), cx);
        self.write_setting(false, "ui.gap", Some(u.gap.into()), cx);
        self.write_setting(false, "ui.rounded", Some(u.rounded.into()), cx);
        self.write_setting(false, "ui.radius", Some(u.radius.into()), cx);
        self.write_setting(false, "ui.tile_border", Some(u.tile_border.into()), cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
    }

    /// Opacity and blur for this project or all (shown now, saved by
    /// `save_transparency`); the popover and Settings.
    pub(super) fn set_opacity(&mut self, project_scope: bool, opacity: f32, blur: bool, window: &mut Window, cx: &mut Context<Self>) {
        let min = qur_theme::MIN_OPACITY;
        let opacity = ((opacity.clamp(min, 1.0)) * 100.).round() / 100.;
        if project_scope {
            self.project.window.opacity = Some(opacity);
            self.project.window.blur = Some(blur);
        } else {
            self.config.window.opacity = opacity;
            self.config.window.blur = blur;
            // Saving for all projects clears this project's own; show that now.
            self.project.window.opacity = None;
            self.project.window.blur = None;
        }
        self.apply_look(window, cx);
    }

    /// Esc or a click outside the Transparency popover keeps what it shows.
    pub(super) fn save_popover_transparency(&mut self, cx: &mut Context<Self>) {
        if let Some(Overlay::Transparency { project_scope }) = self.overlay {
            self.save_transparency(project_scope, cx);
        }
    }

    fn save_transparency(&mut self, project_scope: bool, cx: &mut Context<Self>) {
        let eff = self.effective();
        if project_scope {
            self.write_setting(true, "window.opacity", Some(eff.opacity.into()), cx);
            self.write_setting(true, "window.blur", Some(eff.blur.into()), cx);
        } else {
            self.write_setting(false, "window.opacity", Some(eff.opacity.into()), cx);
            self.write_setting(false, "window.blur", Some(eff.blur.into()), cx);
            self.write_setting(true, "window.opacity", None, cx);
            self.write_setting(true, "window.blur", None, cx);
            self.broadcast_reload(cx);
        }
    }
}

fn accent_for(def: &AgentDef, slot: &qur_theme::Accent) -> Rgba {
    def.accent.as_deref().and_then(Rgba::parse).unwrap_or(slot.base)
}

/// SF Mono when installed (Xcode, Terminal), else Menlo; nothing extra ships.
/// A direct lookup (listing every font costs ~40 ms at startup).
fn default_code_font(cx: &App) -> String {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let sf = *FOUND.get_or_init(|| {
        let ts = cx.text_system();
        let id = ts.resolve_font(&gpui::font("SF Mono"));
        ts.get_font_for_id(id).is_some_and(|f| f.family.as_ref() == "SF Mono")
    });
    if sf { "SF Mono".into() } else { "Menlo".into() }
}

const CONFIG_TEMPLATE: &str = r#"# Qur settings. Saved changes apply right away.
# Per-project overrides live in projects/<id>.toml (set them in Qur > Settings…, or View > Appearance).
# Agents are configured in agents.toml (Qur > Settings…, or Qur > Settings Files > Agents).

# theme = "Auto"            # Auto follows macOS: Qur Light / Qur Dark

[window]
# opacity = 1.0             # 0.6 to 1.0
# blur = true

[editor]
# font_family = ""          # empty = SF Mono if installed, else Menlo
# font_size = 13
# line_height = 1.5
# soft_wrap = true
# tab_width = 4
# show_whitespace = false   # spaces as ·, tabs as →, line ends as ¬ (View > Show Whitespace)
# large_file_mb = 50        # bigger files open read-only, paged from disk (hex for binaries)

[terminal]
# scrollback = 5000         # lines kept per terminal (new terminals); about 2.4 KB a line
# font_family = ""          # "" = the editor's font
# font_size = 0             # 0 = one point under the editor's
# line_height = 1.3
# padding_x = 0
# padding_y = 0
# cursor_style = ""         # block, bar, underline; "" = block, programs may change it
# cursor_blink = false
# theme = ""                # a Ghostty theme file: a path, or a name in themes/ beside this file
# foreground = ""           # colors as #rrggbb; "" = the theme's
# background = ""
# cursor_color = ""
# selection_foreground = ""
# selection_background = ""
# palette = []              # Ghostty style: ['1=#ff5555', '9=#ff6e6e'] (0 to 15)
# bold_is_bright = false
# option_as_alt = false     # Option+key sends Esc+key (Meta) instead of typing ∂, ƒ…
# copy_on_select = true      # selecting copies
# scroll_multiplier = 1.0
# focus_follows_mouse = true  # moving the pointer onto a split pane focuses it
# shell = ""                # "" = your login shell; e.g. "nu" for Nushell's tables
# text_align = "left"       # or "right": shell output grows from the right edge

[ui]
# font_size = 13
# zoom = 1.0
# traffic_lights = "auto"   # full screen: auto (grey in dark themes), color, mono
# gaps = false              # window gaps (Hyprland look): every region its own tile
# gap = 8                   # 2 to 24
# rounded = true            # rounded tiles; radius = 8 (0 to 16)
# tile_border = false
# sidebar = "show"          # show, auto (slides in at the window's edge), hidden
# sidebar_side = "left"     # left or right
# sidebar_sections = ["agents", "terminals", "git", "files"]   # top to bottom

[panels]
# terminal = true
# git = true
# search = true

[keys]
# vim = false
# global = "cmd-\\"          # from any app: show or hide Qur ("" = off; keys macOS uses are refused)
"#;

const KEYMAP_TEMPLATE: &str = r#"# Qur key bindings. Saved changes apply right away; entries Qur can't use
# are skipped and reported (Help > Show Logs). See docs/guide.md, "Keymap".
#
# Name a command by its action ("qur::GoToSymbol", "editor::DeleteLine") or
# its command palette entry ("Go: Go to Symbol in File…", or just the title
# when only one command has it). Keys use the form "cmd-shift-o"; a chord is
# "cmd-k cmd-t"; a list binds several; "" unbinds. Each binding keeps the
# command's usual context (editor, terminal…).

[bindings]
# "Go to Symbol in File" = "cmd-r"
# "editor::DeleteLine" = ["cmd-shift-k", "ctrl-shift-k"]
# "View: Sidebar" = ""
"#;
