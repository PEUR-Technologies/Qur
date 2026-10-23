//! Qur: a lightweight agentic code editor for macOS.
//!
//! `qur [path]` opens a folder as a project, or a file in its project.
//! Without a path it reopens the most recent project, else asks for a folder.

mod assets;
mod agent_view;
mod browser_view;
mod changes_view;
mod commands;
mod editor_view;
mod git_panel;
mod input;
mod live;
mod look;
mod monitor;
mod picker;
mod prompt;
mod terminal;
mod tree_view;
mod workspace;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, PathPromptOptions, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, point, px, size,
};

use commands::MenuState;
use look::Look;
use workspace::Workspace;

static START: OnceLock<Instant> = OnceLock::new();

/// QUR_BENCH: print a startup timestamp (ms since main).
pub fn mark(label: &str) {
    if std::env::var_os("QUR_BENCH").is_some() {
        let ms = START.get().map(|s| s.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0);
        println!("MARK {label} {ms:.1}");
    }
}

/// Menus are app-wide on macOS; the active window's state wins.
pub fn set_menus(state: MenuState, cx: &mut App) {
    #[derive(Default)]
    struct Current(Option<MenuState>);
    impl gpui::Global for Current {}
    let cur = cx.default_global::<Current>();
    if cur.0.as_ref() != Some(&state) {
        cur.0 = Some(state.clone());
        cx.set_menus(commands::menus(&state));
    }
}

/// Called once per window after its first frame (scripts/budget.sh).
pub fn first_frame(handle: AnyWindowHandle, cx: &mut App) {
    // Dev scripting for screenshots/tests: QUR_DEBUG_ACTIONS="qur::ThemePicker,wait:300,qur::ToggleTerminal"
    // dispatches commands by name after the first frame.
    if let Ok(list) = std::env::var("QUR_DEBUG_ACTIONS") {
        let bg = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                if let Some(ms) = name.strip_prefix("wait:").and_then(|v| v.parse().ok()) {
                    bg.timer(std::time::Duration::from_millis(ms)).await;
                    continue;
                }
                bg.timer(std::time::Duration::from_millis(200)).await;
                let _ = handle.update(cx, |_, window, cx| match cx.build_action(name, None) {
                    Ok(a) => window.dispatch_action(a, cx),
                    Err(e) => eprintln!("QUR_DEBUG_ACTIONS: {name}: {e}"),
                });
            }
        })
        .detach();
    }
    if std::env::var_os("QUR_BENCH").is_some() {
        static ONCE: OnceLock<()> = OnceLock::new();
        if ONCE.set(()).is_ok() {
            let ms = START.get().map(|s| s.elapsed().as_millis()).unwrap_or(0);
            println!("METRIC first_frame_ms={ms}");
            if std::env::var("QUR_BENCH").as_deref() == Ok("first_frame") {
                cx.quit();
            }
        }
    }
}

/// The project folder for a path: a folder is its own project; a file opens
/// in its git work tree, else its parent folder.
fn project_root_for(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    let dir = path.parent().unwrap_or(Path::new("/"));
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(["rev-parse", "--show-toplevel"]).output();
    match out {
        Ok(o) if o.status.success() => PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()),
        _ => dir.to_path_buf(),
    }
}

/// Focus the window already showing `root`, else open a new one. Then open `file`.
pub fn open_project(root: PathBuf, file: Option<PathBuf>, cx: &mut App) {
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    for handle in cx.windows() {
        if let Some(ws) = handle.downcast::<Workspace>() {
            let mine = ws.read(cx).map(|w| w.root == root).unwrap_or(false);
            if mine {
                let _ = ws.update(cx, |ws, window, cx| {
                    window.activate_window();
                    if let Some(f) = &file {
                        ws.open_path(f, window, cx);
                    }
                });
                return;
            }
        }
    }
    let window = open_window(root, cx);
    mark("window_open");
    if let Some(f) = file {
        let _ = window.update(cx, |ws, window, cx| ws.open_path(&f, window, cx));
        mark("file_open");
    }
}

pub fn prompt_open_folder(cx: &mut App) {
    let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = rx.await
            && let Some(p) = paths.into_iter().next()
        {
            cx.update(|cx| open_project(p, None, cx));
        }
    })
    .detach();
}

fn open_window(root: PathBuf, cx: &mut App) -> WindowHandle<Workspace> {
    let bench = std::env::var_os("QUR_BENCH").is_some();
    // Cascade new windows a little so they don't stack exactly.
    let n = cx.windows().len() as f32;
    let mut bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
    bounds.origin = point(bounds.origin.x + px(24. * n), bounds.origin.y + px(24. * n));
    let title = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Zed-style: transparent system titlebar, content drawn under
                // it, Qur owns dragging (decision 1).
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(9.), px(9.))),
                }),
                app_owns_titlebar_drag: true,
                // Keep agent activity drawing while Qur is unfocused (M0
                // finding: GPUI's default throttle stalls inactive windows).
                inactive_frame_interval: if bench { None } else { Some(std::time::Duration::from_millis(16)) },
                kind: if bench { gpui::WindowKind::PopUp } else { gpui::WindowKind::Normal },
                ..Default::default()
            },
            |window, cx| {
                let ws = cx.new(|cx| Workspace::new(root, window, cx));
                let w = ws.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    w.update(cx, |ws, cx| ws.confirm_close(window, cx)).unwrap_or(true)
                });
                ws
            },
        )
        .expect("open window");
    let _ = window.update(cx, |_, window, _| window.activate_window());
    window
}

fn main() {
    START.get_or_init(Instant::now);
    let arg = std::env::args_os().nth(1).map(PathBuf::from);
    gpui_platform::application().with_assets(assets::Assets).run(move |cx: &mut App| {
        mark("app_run");
        cx.text_system()
            .add_fonts(assets::FONTS.iter().map(|b| Cow::Borrowed(*b)).collect())
            .expect("bundled fonts");
        mark("fonts_added");
        qur_agents::warm_login_path();
        commands::bind_keys(cx);
        mark("keys_bound");
        cx.activate(true);

        match arg {
            Some(p) => {
                let p = std::fs::canonicalize(&p).unwrap_or(p);
                let file = p.is_file().then(|| p.clone());
                open_project(project_root_for(&p), file, cx);
            }
            None => match qur_config::State::load().recent_projects.into_iter().find(|p| p.is_dir()) {
                Some(p) => open_project(p, None, cx),
                None => prompt_open_folder(cx),
            },
        }

        cx.on_window_closed(|cx, id| {
            Look::remove(id, cx);
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.on_app_quit(|cx| {
            for h in cx.windows() {
                if let Some(ws) = h.downcast::<Workspace>() {
                    let _ = ws.update(cx, |ws, _, cx| ws.shutdown(cx));
                }
            }
            async {}
        })
        .detach();
    });
}
