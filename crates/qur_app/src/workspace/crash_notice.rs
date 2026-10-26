//! "Qur quit unexpectedly last time": a small card in the window's bottom
//! corner for a crash report the user hasn't seen (crash.rs), with View
//! Report, Report a Problem and Dismiss. It never blocks: it waits there
//! (not in the top bar, which zen hides) until one is clicked, and any of
//! them marks the report seen.

use super::*;

impl Workspace {
    pub(super) fn crash_notice_el(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.crash_notice.as_ref()?;
        let ui = &look.theme.ui;
        let button = |id: &'static str, label: &'static str| {
            crate::setup_view::link(look, id, label).relative().children(crate::script::mark(move || format!("crash:{}", id.trim_start_matches("crash-"))))
        };
        Some(
            div()
                .id("crash-notice")
                .absolute()
                .bottom(look.z(16.))
                .when(self.sidebar_right(), |d| d.left(look.z(16.)))
                .when(!self.sidebar_right(), |d| d.right(look.z(16.)))
                .flex()
                .flex_col()
                .gap(look.z(6.))
                .px(look.z(12.))
                .py(look.z(10.))
                .bg(look.c(ui.overlay_bg))
                .border_1()
                .border_color(look.c(ui.border_strong))
                .rounded(look.z(6.))
                // Clicks stay in the card.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().text_size(look.text(-1.)).text_color(look.c(ui.text)).child("Qur quit unexpectedly last time"))
                .child(
                    div()
                        .flex()
                        .gap(look.z(12.))
                        .child(button("crash-view", "View Report").on_click(cx.listener(|this, _, _, cx| {
                            if let Some(p) = this.close_crash_notice(cx) {
                                this.open_args(&[p.as_os_str()], cx);
                            }
                        })))
                        .child(button("crash-report", "Report a Problem").on_click(cx.listener(|this, _, window, cx| {
                            this.close_crash_notice(cx);
                            this.report_problem(&qur::ReportProblem, window, cx);
                        })))
                        .child(button("crash-dismiss", "Dismiss").on_click(cx.listener(|this, _, _, cx| {
                            this.close_crash_notice(cx);
                        }))),
                )
                .into_any_element(),
        )
    }

    /// Hide the notice and remember its report as seen (`state.toml`).
    fn close_crash_notice(&mut self, cx: &mut Context<Self>) -> Option<PathBuf> {
        let p = self.crash_notice.take()?;
        crate::crash::acknowledge(&p);
        cx.notify();
        Some(p)
    }

    /// `open` a file, folder or URL (`-R` first: select it in Finder).
    /// Scripted runs print `OPEN …` instead of leaving Qur.
    pub(super) fn open_args(&mut self, args: &[&std::ffi::OsStr], cx: &mut Context<Self>) {
        if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
            println!("OPEN {}", args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "));
            return;
        }
        if let Err(e) = std::process::Command::new("/usr/bin/open").args(args).spawn() {
            self.flash(format!("Couldn't open {}: {e}", args.last().map(|a| a.to_string_lossy()).unwrap_or_default()), cx);
        }
    }
}
