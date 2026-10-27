//! Scripted-run state (`debug_state`) and benchmarks.

use super::*;

impl Workspace {
    /// QUR_DEBUG_ACTIONS `state`: what scripted runs check instead of a
    /// (possibly stale) screenshot.
    pub fn debug_state(&self, cx: &App) -> String {
        let overlay = match &self.overlay {
            None => "none".to_string(),
            Some(Overlay::Picker(..)) => "picker".into(),
            Some(Overlay::GoTo(i)) => format!("goto:{:?}", i.read(cx).text),
            Some(Overlay::Rename(i)) => format!("rename:{:?}", i.read(cx).text),
            Some(Overlay::Setup(v)) => format!("settings:{}", v.read(cx).section_name()),
            Some(Overlay::Accent) => "accent".into(),
            Some(Overlay::AgentMenu { id, .. }) => format!("agent_menu:{id}"),
            Some(Overlay::FileMenu { rel, .. }) => format!("file_menu:{}", rel.display()),
            Some(Overlay::Layouts) => "layouts".into(),
            Some(Overlay::About(_)) => "about".into(),
            Some(Overlay::Transparency { .. }) => "transparency".into(),
            Some(Overlay::Gaps) => "gaps".into(),
            Some(Overlay::NewTask { .. }) => "new_task".into(),
            Some(Overlay::Review { input, .. }) => format!("review:{:?}", input.read(cx).text),
            Some(Overlay::Cleanup(c)) => c.debug(),
        };
        let selected = match &self.selected {
            Some(Selected::Agent(id) | Selected::TermTask(id)) => {
                let title = self.task_parts(id, cx).map(|p| p.2).unwrap_or_default();
                format!("task:{id}:{title:?}")
            }
            Some(Selected::Shell(n)) => format!("shell:{n}"),
            Some(Selected::Git) => "git".into(),
            Some(_) => "other".into(),
            None => "none".into(),
        };
        let code = match &self.code {
            Some(c) => {
                let e = c.read(cx);
                let p = e.cursor();
                let mode = e.vim.as_ref().and_then(|v| v.mode).map(|m| m.label()).unwrap_or("-");
                let text: String = e.model.rope().to_string().chars().take(200).collect();
                format!("{}@{}:{} vim={mode} dirty={} changes={} format={:?} text={text:?}", e.title(), p.line + 1, p.col + 1, e.is_dirty(), e.diff_debug(), Self::code_status(e))
            }
            None => "none".into(),
        };
        let buffers = format!("{}/{}", self.buffers.loaded().count(), self.buffers.len());
        let vc = self.git.as_ref().map(|g| g.read(cx).debug_state(cx)).unwrap_or_else(|| "none".into());
        let docks = format!("acp={} terminal={}", Docks::get(cx, dock::Kind::Acp).serialize(), Docks::get(cx, dock::Kind::Terminal).serialize());
        let u = &self.config.ui;
        let gaps = format!("on={} gap={} rounded={} radius={} border={} sidebar_w={}", u.gaps, u.gap, u.rounded, u.radius, u.tile_border, self.sidebar_w);
        let sidebar = format!("on={} mode={} side={} shown={} visible={} sections={}", self.sidebar, u.sidebar, u.sidebar_side, self.sidebar_shown, self.sidebar_visible(), u.sidebar_sections.join(","));
        let viewer = self.viewer.as_ref().map_or_else(|| "none".into(), |v| v.debug_state(cx));
        let crash = self.crash_notice.as_ref().and_then(|p| p.file_name()).map_or_else(|| "none".into(), |n| n.to_string_lossy().into_owned());
        let project = match (self.home, self.repo) {
            (true, _) => "none".to_string(),
            (false, true) => format!("repo:{}", self.project_name()),
            (false, false) => format!("folder:{}", self.project_name()),
        };
        let agents = self.agents.iter().map(|a| a.read(cx).debug_state()).collect::<Vec<_>>().join(" ");
        // The selected task's Changes view, and the changed files its heat counts as the agent's.
        let work = match &self.selected {
            Some(Selected::Agent(id) | Selected::TermTask(id)) => {
                let join = |v: Vec<PathBuf>| v.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(",");
                let monitor = self.agent(id).map(|a| a.read(cx).monitor.clone()).or_else(|| self.term_task(id).map(|t| t.monitor.clone()));
                let heat = monitor.map(|m| join(m.read(cx).heat.changed())).unwrap_or_default();
                let changes = self.task_parts(id, cx).map(|p| join(p.3.read(cx).files.iter().map(|f| f.path.clone()).collect())).unwrap_or_default();
                format!("changes=[{changes}] heat=[{heat}]")
            }
            _ => "none".into(),
        };
        format!(
            "project={project} overlay={overlay} selected={selected} crash={crash} agents={{{agents}}} work={{{work}}} code={code} buffers={buffers} viewer={viewer} term={{{}}} git={{{vc}}} docks={{{docks}}} gaps={{{gaps}}} sidebar={{{sidebar}}}",
            self.terms.read(cx).debug_state(cx)
        )
    }

    /// QUR_BENCH=glow: a terminal task's worktree gets 20 probe writes from a
    /// background thread (like a formatter or codegen); each is timed from the
    /// write until its row is painted glowing (acceptance: < 100 ms).
    pub(super) fn bench_glow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let dir = qur_agents::worktrees_dir(&self.project_id);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let created = bg.spawn(async move { qur_worktree::create(&root, &dir, "bench", "glow") }).await;
            let Ok(c) = created else { return };
            let wt = c.path.clone();
            this.update_in(cx, |this, window, cx| {
                let record = TaskRecord {
                    id: c.slug.clone(),
                    agent: "bench".into(),
                    title: "glow bench".into(),
                    branch: c.branch.clone(),
                    worktree: c.path.clone(),
                    base: c.base.clone(),
                    session_id: None,
                    created: 0,
                    mode: Mode::Terminal,
                    browser_url: None,
                    renamed: false,
                    in_root: false,
                    queued: Vec::new(),
                    turns: Vec::new(), review: None, pr_url: None,
                };
                this.add_term_task(record, window, cx);
                // Show the tree without starting a terminal process.
                this.selected = Some(Selected::TermTask(c.slug.clone()));
                let root = this.current_root(cx);
                this.tree_for(&root, window, cx);
                cx.notify();
            })
            .ok();
            bg.timer(Duration::from_millis(1500)).await;
            for i in 0..20 {
                let p = wt.join(format!("glow-{i}.txt"));
                std::fs::write(&p, "x\n").ok();
                crate::tree_view::GLOW_WRITES.lock().unwrap().push((p, Instant::now()));
                bg.timer(Duration::from_millis(400)).await;
            }
            bg.timer(Duration::from_millis(800)).await;
            let mut v = crate::tree_view::GLOW_SEEN.lock().unwrap().clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if !v.is_empty() {
                println!("METRIC glow_n={}", v.len());
                println!("METRIC glow_p50_ms={:.1}", v[v.len() / 2]);
                println!("METRIC glow_max_ms={:.1}", v[v.len() - 1]);
            }
            println!("METRIC glow_missed={}", 20 - v.len());
            this.update(cx, |this, cx| {
                let _ = qur_worktree::remove(&this.root, &c.path, &c.branch, true);
                cx.quit();
            })
            .ok();
        })
        .detach();
    }

    /// QUR_BENCH=term: a shell runs scripts/termbench.sh (timed `cat` of big
    /// files, so the terminal's own draining is what's measured); its results
    /// print as METRIC lines.
    pub(super) fn bench_term(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = repo.join("spikes/out/termbench-qur.txt");
        std::fs::remove_file(&out).ok();
        self.new_terminal_in(self.root.clone(), window, cx);
        let Some(Selected::Shell(tid)) = self.selected else { return };
        let cmd = format!("bash {} {}; exit\r", repo.join("scripts/termbench.sh").display(), out.display());
        self.terms.update(cx, |p, _| p.send_keys(tid, &cmd));
        let bg = cx.background_executor().clone();
        cx.spawn(async move |_, cx| {
            loop {
                bg.timer(Duration::from_millis(250)).await;
                let text = std::fs::read_to_string(&out).unwrap_or_default();
                if text.lines().any(|l| l == "done") {
                    text.lines().filter(|l| *l != "done").for_each(|l| println!("METRIC term_{l}"));
                    break;
                }
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    /// QUR_BENCH=theme: switch through every theme, one per frame.
    pub(super) fn bench_theme_step(&mut self, i: usize, mut samples: Vec<f64>, window: &mut Window, cx: &mut Context<Self>) {
        let themes = Registry::global().all();
        if i >= themes.len() * 2 {
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let pct = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
            println!("METRIC theme_switch_n={}", samples.len());
            println!("METRIC theme_switch_p50_ms={:.2}", pct(0.5));
            println!("METRIC theme_switch_max_ms={:.2}", samples[samples.len() - 1]);
            return cx.quit();
        }
        let t0 = Instant::now();
        self.preview_theme = Some(themes[i % themes.len()].name.clone());
        self.apply_look(window, cx);
        let me = cx.entity();
        window.on_next_frame(move |window, cx| {
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
            me.update(cx, |ws, cx| ws.bench_theme_step(i + 1, samples, window, cx));
        });
    }
}
