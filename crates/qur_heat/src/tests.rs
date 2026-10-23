use super::*;
use std::process::Command;

const WT: &str = "/wt/task";

fn hm() -> Heatmap {
    Heatmap::new(PathBuf::from(WT), "base".into())
}
fn abs(rel: &str) -> PathBuf {
    Path::new(WT).join(rel)
}
fn p(rel: &str) -> PathBuf {
    PathBuf::from(rel)
}
struct Clock(Instant);
impl Clock {
    fn at(&self, ms: u64) -> Instant {
        self.0 + Duration::from_millis(ms)
    }
}
fn clock() -> Clock {
    Clock(Instant::now())
}
fn state(h: &Heatmap, rel: &str, now: Instant) -> HeatState {
    h.cell(Path::new(rel), now).state
}
const CHANGED: DiffStat = DiffStat { added: 3, removed: 1, created: false, deleted: false, binary: false };

#[test]
fn priority_order() {
    use HeatState::*;
    let order = [Idle, Changed, Deleted, Created, Reading, Editing, NeedsApproval];
    assert!(order.windows(2).all(|w| w[0] < w[1]));

    // Stack every source on one file and peel them off one by one.
    let c = clock();
    let mut h = hm();
    let f = abs("a.rs");
    h.set_stat(Path::new("a.rs"), Some(CHANGED));
    assert_eq!(state(&h, "a.rs", c.at(0)), Changed);
    h.set_stat(Path::new("a.rs"), Some(DiffStat { deleted: true, ..CHANGED }));
    assert_eq!(state(&h, "a.rs", c.at(0)), Deleted);
    h.set_stat(Path::new("a.rs"), Some(DiffStat { created: true, ..CHANGED }));
    assert_eq!(state(&h, "a.rs", c.at(0)), Created);
    h.on_tool("r", Some(ToolKind::Read), Some(ToolStatus::Pending), std::slice::from_ref(&f), c.at(0));
    assert_eq!(state(&h, "a.rs", c.at(0)), Reading);
    h.on_tool("e", Some(ToolKind::Edit), Some(ToolStatus::Pending), std::slice::from_ref(&f), c.at(0));
    assert_eq!(state(&h, "a.rs", c.at(0)), Editing);
    h.on_permission(std::slice::from_ref(&f), true);
    assert_eq!(state(&h, "a.rs", c.at(0)), NeedsApproval);

    h.on_permission(std::slice::from_ref(&f), false);
    assert_eq!(state(&h, "a.rs", c.at(10)), Editing);
    h.on_tool("e", None, Some(ToolStatus::Completed), &[], c.at(10));
    assert_eq!(state(&h, "a.rs", c.at(10)), Reading);
    h.on_tool("r", None, Some(ToolStatus::Completed), &[], c.at(10));
    assert_eq!(state(&h, "a.rs", c.at(10 + 1500)), Created);
    h.set_stat(Path::new("a.rs"), None);
    let cell = h.cell(Path::new("a.rs"), c.at(2000));
    assert_eq!((cell.state, cell.fade, cell.stat), (Idle, 0.0, None));
}

#[test]
fn tool_call_without_paths_then_update_with_paths() {
    // The Claude adapter: kind on the first call, locations 0.5-0.9 s later, then
    // completed with no in_progress.
    let c = clock();
    let mut h = hm();
    h.on_tool("t1", Some(ToolKind::Edit), Some(ToolStatus::Pending), &[], c.at(0));
    assert_eq!(h.folder_state(Path::new(""), c.at(0)), None);
    assert!(h.focus().is_none());

    h.on_tool("t1", None, None, &[abs("src/greet.ts")], c.at(535));
    assert_eq!(state(&h, "src/greet.ts", c.at(535)), HeatState::Editing);
    assert_eq!(h.cell(Path::new("src/greet.ts"), c.at(535)).level, 1);
    assert_eq!(h.focus(), Some((abs("src/greet.ts"), None)));
    h.set_tool_line("t1", &abs("src/greet.ts"), 12);
    assert_eq!(h.focus(), Some((abs("src/greet.ts"), Some(12))));

    // Repeated updates with the same location don't add touches.
    h.on_tool("t1", None, None, &[abs("src/greet.ts")], c.at(887));
    h.on_permission(&[abs("src/greet.ts")], true);
    assert_eq!(state(&h, "src/greet.ts", c.at(897)), HeatState::NeedsApproval);
    h.on_permission(&[abs("src/greet.ts")], false);
    assert_eq!(state(&h, "src/greet.ts", c.at(900)), HeatState::Editing);

    h.on_tool("t1", None, Some(ToolStatus::Completed), &[], c.at(910));
    // No stat yet and no turn write: back to idle, with a recompute due 150 ms later.
    assert_eq!(state(&h, "src/greet.ts", c.at(910)), HeatState::Idle);
    assert!(h.take_due(c.at(1059)).is_empty());
    assert_eq!(h.take_due(c.at(1060)), vec![p("src/greet.ts")]);
    h.set_stat(Path::new("src/greet.ts"), Some(DiffStat { added: 4, created: true, ..Default::default() }));
    let cell = h.cell(Path::new("src/greet.ts"), c.at(1100));
    assert_eq!((cell.state, cell.level), (HeatState::Created, 1));

    // A late update after completion never revives it.
    h.on_tool("t1", None, Some(ToolStatus::Pending), &[], c.at(1200));
    assert_eq!(state(&h, "src/greet.ts", c.at(1200)), HeatState::Created);
    assert!(h.take_due(c.at(5000)).is_empty());
}

#[test]
fn reading_fades_over_1_5_s_after_completion() {
    let c = clock();
    let mut h = hm();
    h.on_tool("r", Some(ToolKind::Read), Some(ToolStatus::Pending), &[], c.at(0));
    h.on_tool("r", None, None, &[abs("lib.rs")], c.at(500));
    assert_eq!(h.cell(Path::new("lib.rs"), c.at(5000)).fade, 1.0, "still active: no fade");
    h.on_tool("r", None, Some(ToolStatus::Completed), &[], c.at(6000));
    let at = |ms| h.cell(Path::new("lib.rs"), c.at(ms));
    assert_eq!((at(6000).state, at(6000).fade), (HeatState::Reading, 1.0));
    assert_eq!(at(6750).state, HeatState::Reading);
    assert!((at(6750).fade - 0.5).abs() < 1e-3, "{}", at(6750).fade);
    assert_eq!(at(7499).state, HeatState::Reading);
    assert_eq!((at(7500).state, at(7500).fade), (HeatState::Idle, 0.0));
    assert_eq!(h.next_deadline(c.at(6100)), Some(c.at(7500)));
    assert_eq!(h.next_deadline(c.at(7500)), None);

    // Search counts too; paths arriving together with completion start the fade at once.
    h.on_tool("s", Some(ToolKind::Search), Some(ToolStatus::Completed), &[abs("src")], c.at(8000));
    assert_eq!(state(&h, "src", c.at(8000)), HeatState::Reading);
    assert_eq!(state(&h, "src", c.at(9500)), HeatState::Idle);
    // Failed ends a read like completed.
    h.on_tool("f", Some(ToolKind::Read), Some(ToolStatus::InProgress), &[abs("x")], c.at(10_000));
    h.on_tool("f", None, Some(ToolStatus::Failed), &[], c.at(10_100));
    assert_eq!(state(&h, "x", c.at(11_599)), HeatState::Reading);
    assert_eq!(state(&h, "x", c.at(11_600)), HeatState::Idle);
}

#[test]
fn editing_times_out_after_last_turn_write() {
    let c = clock();
    let mut h = hm();
    let f = abs("gen.rs");
    h.on_fs_change(std::slice::from_ref(&f), true, c.at(0));
    h.on_fs_change(std::slice::from_ref(&f), true, c.at(1000));
    let at = |ms| h.cell(Path::new("gen.rs"), c.at(ms));
    assert_eq!(at(2499).state, HeatState::Editing);
    assert!((at(1750).fade - 0.5).abs() < 1e-3);
    assert_eq!(at(2500).state, HeatState::Idle);
    assert_eq!(at(2500).level, 1, "a burst within the window is one touch");
    assert_eq!(h.next_deadline(c.at(1200)), Some(c.at(1200)), "overdue recompute is due now");
    h.take_due(c.at(1200));
    assert_eq!(h.next_deadline(c.at(1200)), Some(c.at(2500)));

    // Outside a turn: no editing, no touch, just a recompute.
    h.on_fs_change(std::slice::from_ref(&f), false, c.at(3000));
    assert_eq!(state(&h, "gen.rs", c.at(3000)), HeatState::Idle);
    assert_eq!(h.cell(Path::new("gen.rs"), c.at(3000)).level, 1);
    assert_eq!(h.take_due(c.at(3150)), vec![p("gen.rs")]);

    // Edit tool active: editing regardless of time, and its write isn't a second touch.
    h.on_tool("e", Some(ToolKind::Edit), Some(ToolStatus::Pending), std::slice::from_ref(&f), c.at(4000));
    h.on_fs_change(std::slice::from_ref(&f), true, c.at(4100));
    assert_eq!(state(&h, "gen.rs", c.at(60_000)), HeatState::Editing);
    assert_eq!(h.cell(Path::new("gen.rs"), c.at(60_000)).level, 2);
}

#[test]
fn levels_cap_at_4() {
    let c = clock();
    let mut h = hm();
    for i in 0..9u64 {
        h.on_tool(&format!("t{i}"), Some(ToolKind::Read), Some(ToolStatus::Completed), &[abs("hot.rs")], c.at(i));
        let want = (i + 1).min(4) as u8;
        assert_eq!(h.cell(Path::new("hot.rs"), c.at(i)).level, want);
    }
    // Separate write bursts (> 1.5 s apart) count too, still capped.
    for i in 0..6u64 {
        h.on_fs_change(&[abs("w.rs")], true, c.at(i * 2000));
    }
    assert_eq!(h.cell(Path::new("w.rs"), c.at(20_000)).level, 4);
    assert_eq!(h.cell(Path::new("untouched.rs"), c.at(0)).level, 0);
}

#[test]
fn folder_aggregation_nested() {
    let c = clock();
    let mut h = hm();
    h.set_stat(Path::new("src/a/b/deep.rs"), Some(CHANGED));
    h.set_stat(Path::new("src/top.rs"), Some(DiffStat { created: true, ..Default::default() }));
    h.on_tool("r", Some(ToolKind::Read), Some(ToolStatus::Pending), &[abs("docs/x.md")], c.at(0));
    let fs = |h: &Heatmap, d: &str| h.folder_state(Path::new(d), c.at(0));
    assert_eq!(fs(&h, "src/a/b"), Some(HeatState::Changed));
    assert_eq!(fs(&h, "src/a"), Some(HeatState::Changed));
    assert_eq!(fs(&h, "src"), Some(HeatState::Created));
    assert_eq!(fs(&h, "docs"), Some(HeatState::Reading));
    assert_eq!(fs(&h, ""), Some(HeatState::Reading));
    assert_eq!(fs(&h, "other"), None);
    // "src/a" must not match "src/ab".
    h.set_stat(Path::new("src/ab/z.rs"), Some(DiffStat { deleted: true, ..Default::default() }));
    assert_eq!(fs(&h, "src/a"), Some(HeatState::Changed));
    h.on_permission(&[abs("src/a/b/deep.rs")], true);
    assert_eq!(h.folder_state(Path::new("src"), c.at(0)), Some(HeatState::NeedsApproval));
    // Once a fade ends the folder goes quiet.
    h.on_tool("r", None, Some(ToolStatus::Completed), &[], c.at(0));
    assert_eq!(h.folder_state(Path::new("docs"), c.at(1500)), None);
}

#[test]
fn take_due_debounces_bursts() {
    let c = clock();
    let mut h = hm();
    // 20 writes 10 ms apart: one recompute, 150 ms after the last.
    for i in 0..20u64 {
        h.on_fs_change(&[abs("a.rs"), abs("b.rs")], true, c.at(i * 10));
        assert!(h.take_due(c.at(i * 10)).is_empty());
    }
    assert!(h.take_due(c.at(190 + 149)).is_empty());
    assert_eq!(h.next_deadline(c.at(200)), Some(c.at(340)));
    assert_eq!(h.take_due(c.at(340)), vec![p("a.rs"), p("b.rs")]);
    assert!(h.take_due(c.at(10_000)).is_empty(), "taken once");
    // Paths outside the worktree and .git internals are ignored.
    h.on_fs_change(&[PathBuf::from("/elsewhere/x"), abs(".git/index"), PathBuf::from(WT)], true, c.at(0));
    assert!(h.take_due(c.at(10_000)).is_empty());
    h.on_tool("o", Some(ToolKind::Edit), None, &[PathBuf::from("/wt/task2/x")], c.at(0));
    assert!(h.focus().is_none());
    assert_eq!(h.folder_state(Path::new(""), c.at(10_000)), None);
}

#[test]
fn focus_follows_the_latest_edit() {
    let dir = std::env::temp_dir().join(format!("qur-heat-focus-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["fmt.rs", "late.rs"] {
        std::fs::write(dir.join(f), "x").unwrap();
    }
    let c = clock();
    let mut h = Heatmap::new(dir.clone(), "base".into());
    // A turn with no ACP edits: the most recent write is the focus.
    h.on_fs_change(&[dir.join("fmt.rs")], true, c.at(0));
    assert_eq!(h.focus(), Some((dir.join("fmt.rs"), None)));
    // Reads never take focus.
    h.on_tool("r", Some(ToolKind::Read), Some(ToolStatus::Completed), &[dir.join("late.rs")], c.at(10));
    assert_eq!(h.focus(), Some((dir.join("fmt.rs"), None)));
    // ACP edit with a location wins.
    h.on_tool("e1", Some(ToolKind::Edit), Some(ToolStatus::Pending), &[], c.at(100));
    h.on_tool("e1", None, None, &[dir.join("a.rs")], c.at(700));
    h.set_tool_line("e1", &dir.join("a.rs"), 40);
    assert_eq!(h.focus(), Some((dir.join("a.rs"), Some(40))));
    // A formatter write while that edit is active doesn't steal focus...
    h.on_fs_change(&[dir.join("fmt.rs")], true, c.at(800));
    assert_eq!(h.focus(), Some((dir.join("a.rs"), Some(40))));
    h.on_tool("e1", None, Some(ToolStatus::Completed), &[], c.at(900));
    // ...nor shortly after it...
    h.on_fs_change(&[dir.join("fmt.rs")], true, c.at(1500));
    assert_eq!(h.focus(), Some((dir.join("a.rs"), Some(40))));
    // ...but the next edit moves it, line known later (set before the path is fine too).
    h.on_tool("e2", Some(ToolKind::Edit), Some(ToolStatus::Pending), &[], c.at(2000));
    h.set_tool_line("e2", &dir.join("b.rs"), 7);
    h.on_tool("e2", None, None, &[dir.join("b.rs")], c.at(2500));
    assert_eq!(h.focus(), Some((dir.join("b.rs"), Some(7))));
    h.on_tool("e2", None, Some(ToolStatus::Completed), &[], c.at(2600));
    // Long after the last ACP edit, a shell write (codegen) takes focus; deleted files
    // and directories don't.
    h.on_fs_change(&[dir.join("gone.rs"), dir.clone()], true, c.at(9000));
    assert_eq!(h.focus(), Some((dir.join("b.rs"), Some(7))));
    h.on_fs_change(&[dir.join("late.rs")], true, c.at(9000));
    assert_eq!(h.focus(), Some((dir.join("late.rs"), None)));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- git helpers against a real repo ----

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

fn repo() -> (PathBuf, String) {
    let d = std::env::temp_dir().join(format!("qur-heat-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("dir")).unwrap();
    sh(&d, &["init", "-q", "-b", "main"]);
    let w = |f: &str, s: &[u8]| std::fs::write(d.join(f), s).unwrap();
    w("keep.txt", b"same\n");
    w("mod.txt", b"1\n2\n3\n");
    w("del.txt", b"a\nb\n");
    w("old name.txt", b"x\ny\nz\n");
    w("bin.dat", b"\x00\x01\x02");
    w("dir/in.txt", b"q\n");
    w(".gitignore", b"*.log\n");
    sh(&d, &["add", "-A"]);
    sh(&d, &["commit", "-q", "-m", "base"]);
    let base = sh(&d, &["rev-parse", "HEAD"]).trim().to_string();
    // Agent's changes.
    w("mod.txt", b"1\nTWO\n3\n4\n");
    std::fs::remove_file(d.join("del.txt")).unwrap();
    std::fs::rename(d.join("old name.txt"), d.join("new name.txt")).unwrap();
    w("bin.dat", b"\x00\x09");
    w("new.txt", b"l1\nl2\nno newline");
    w("new.bin", b"ab\x00cd");
    w("noise.log", b"ignored\n");
    w("dir/added.txt", b"k\n");
    // Staged changes are still "differs from base".
    w("staged.txt", b"s\n");
    sh(&d, &["add", "staged.txt"]);
    (d, base)
}

fn st(added: u32, removed: u32) -> DiffStat {
    DiffStat { added, removed, ..Default::default() }
}

#[test]
fn diff_stats_and_full_scan_against_real_repo() {
    let (d, base) = repo();
    let created = |n| DiffStat { added: n, created: true, ..Default::default() };
    let want: Vec<(PathBuf, DiffStat)> = vec![
        (p("bin.dat"), DiffStat { binary: true, ..Default::default() }),
        (p("del.txt"), DiffStat { removed: 2, deleted: true, ..Default::default() }),
        (p("dir/added.txt"), created(1)),
        (p("mod.txt"), st(2, 1)),
        (p("new name.txt"), created(3)),
        (p("new.bin"), DiffStat { binary: true, created: true, ..Default::default() }),
        (p("new.txt"), created(3)),
        (p("old name.txt"), DiffStat { removed: 3, deleted: true, ..Default::default() }),
        (p("staged.txt"), created(1)),
    ];
    assert_eq!(full_scan(&d, &base), want);

    let rels = [p("mod.txt"), p("keep.txt"), p("new.txt"), p("del.txt"), p("noise.log"), p("missing.txt")];
    assert_eq!(
        diff_stats(&d, &base, &rels),
        vec![
            (p("del.txt"), Some(DiffStat { removed: 2, deleted: true, ..Default::default() })),
            (p("keep.txt"), None),
            (p("missing.txt"), None),
            (p("mod.txt"), Some(st(2, 1))),
            (p("new.txt"), Some(created(3))),
            (p("noise.log"), None),
        ]
    );
    // A directory (FSEvents reports `mv dir/` as just the dir) expands to its files.
    assert_eq!(diff_stats(&d, &base, &[p("dir")]), vec![(p("dir"), None), (p("dir/added.txt"), Some(created(1)))]);
    std::fs::rename(d.join("dir"), d.join("moved")).unwrap();
    assert_eq!(
        diff_stats(&d, &base, &[p("dir"), p("moved")]),
        vec![
            (p("dir"), None),
            (p("dir/in.txt"), Some(DiffStat { removed: 1, deleted: true, ..Default::default() })),
            (p("moved"), None),
            (p("moved/added.txt"), Some(created(1))),
            (p("moved/in.txt"), Some(created(1))),
        ]
    );
    // Pathspec magic characters are literal.
    assert_eq!(diff_stats(&d, &base, &[p("*.txt")]), vec![(p("*.txt"), None)]);
    assert!(diff_stats(&d, &base, &[]).is_empty());

    // Feeding results back into the heatmap.
    let mut h = Heatmap::new(d.clone(), base.clone());
    let now = Instant::now();
    for (rel, s) in full_scan(&d, &base) {
        h.set_stat(&rel, Some(s));
    }
    assert_eq!(state(&h, "del.txt", now), HeatState::Deleted);
    assert_eq!(state(&h, "new name.txt", now), HeatState::Created);
    assert_eq!(state(&h, "mod.txt", now), HeatState::Changed);
    assert!(h.changed().contains(&p("old name.txt")));
    let _ = std::fs::remove_dir_all(&d);
}
