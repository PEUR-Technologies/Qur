use std::path::{Path, PathBuf};
use std::process::Command;

use crate::*;

struct Repo {
    dir: PathBuf,
    extra: Vec<PathBuf>,
}

fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let base = std::env::temp_dir().join("qur_git_tests");
    std::fs::create_dir_all(&base).unwrap();
    // Canonical so paths match what git reports (/var -> /private/var on macOS).
    let base = base.canonicalize().unwrap();
    let d = base.join(format!("{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_EDITOR", "true")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn configure(dir: &Path) {
    for (k, v) in [
        ("user.name", "Test User"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
        ("merge.conflictStyle", "merge"),
        ("core.hooksPath", "/dev/null"),
        ("init.defaultBranch", "main"),
    ] {
        sh(dir, &["config", k, v]);
    }
}

impl Repo {
    fn new(name: &str) -> Repo {
        let dir = scratch(name);
        std::fs::create_dir_all(&dir).unwrap();
        sh(&dir, &["init", "-q", "-b", "main"]);
        configure(&dir);
        Repo { dir, extra: Vec::new() }
    }

    fn git(&self, args: &[&str]) -> String {
        sh(&self.dir, args)
    }

    fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let p = self.dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.join(rel)).unwrap()
    }

    fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.head()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// A task worktree on `qur/claude/<slug>` outside the repo.
    fn worktree(&mut self, slug: &str) -> Repo {
        let wt = scratch(&format!("wt-{slug}"));
        let branch = format!("qur/claude/{slug}");
        self.git(&["worktree", "add", "-q", "-b", &branch, wt.to_str().unwrap(), "HEAD"]);
        self.extra.push(wt.clone());
        Repo { dir: wt, extra: Vec::new() }
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        for d in &self.extra {
            let _ = std::fs::remove_dir_all(d);
        }
        if !self.extra.is_empty() || self.dir.join(".git").is_dir() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

fn one(r: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    std::iter::once(r).collect()
}

fn find<'a>(d: &'a [FileDiff], p: &str) -> &'a FileDiff {
    d.iter().find(|f| f.path == Path::new(p)).unwrap_or_else(|| panic!("{p} not in {:?}", paths(d)))
}

fn paths(d: &[FileDiff]) -> Vec<PathBuf> {
    d.iter().map(|f| f.path.clone()).collect()
}

fn texts(f: &FileDiff, k: LineKind) -> Vec<String> {
    f.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == k).map(|l| l.text.clone()).collect()
}

// ---------------------------------------------------------------- diffs

#[test]
fn task_diff_covers_committed_uncommitted_untracked() {
    let mut r = Repo::new("diff");
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    r.write("a.txt", "one\ntwo\nthree\n");
    r.write("del.txt", "bye\n");
    r.write("old name.txt", &body);
    r.write("bin.dat", b"\x00\x01\x02binary\x00");
    r.write("noeol.txt", "first\nlast");
    r.write("crlf.txt", "alpha\r\nbeta\r\ngamma\r\n");
    r.write("committed.txt", "c1\n");
    r.write(".gitignore", "*.log\n");
    let base = r.commit("base");
    let wt = r.worktree("t");

    // Committed on the task branch.
    wt.write("committed.txt", "c1\nc2\n");
    wt.commit("task commit");
    // Uncommitted.
    wt.write("a.txt", "one\nTWO\nthree\n");
    std::fs::remove_file(wt.dir.join("del.txt")).unwrap();
    std::fs::remove_file(wt.dir.join("old name.txt")).unwrap();
    wt.write("new name.txt", body.replace("line 20", "line twenty"));
    wt.write("bin.dat", b"\x00\x09binary\x00changed");
    wt.write("noeol.txt", "first\nlast changed");
    wt.write("crlf.txt", "alpha\r\nBETA\r\ngamma\r\n");
    // Untracked, odd names; ignored file must not show.
    wt.write("dir/ünï cödé \"q\".txt", "hi\n");
    wt.write("nl\nname.txt", "x\n");
    wt.write("debug.log", "ignored\n");

    let d = task_diff(&wt.dir, &base).unwrap();
    let mut sorted = paths(&d);
    sorted.sort();
    assert_eq!(paths(&d), sorted, "sorted by path");
    assert!(!d.iter().any(|f| f.path == Path::new("debug.log")));

    let a = find(&d, "a.txt");
    assert_eq!(a.status, FileStatus::Modified);
    assert_eq!((a.added, a.removed), (1, 1));
    assert_eq!(texts(a, LineKind::Removed), ["two"]);
    assert_eq!(texts(a, LineKind::Added), ["TWO"]);
    let h = &a.hunks[0];
    assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (1, 3, 1, 3));
    assert!(h.header.starts_with("@@ -1,3 +1,3 @@"));
    assert_eq!(h.lines[0], DiffLine { kind: LineKind::Context, old_no: Some(1), new_no: Some(1), text: "one".into(), emphasis: vec![] });
    assert_eq!((h.lines[1].old_no, h.lines[1].new_no), (Some(2), None));
    assert_eq!((h.lines[2].old_no, h.lines[2].new_no), (None, Some(2)));

    let del = find(&d, "del.txt");
    assert_eq!(del.status, FileStatus::Deleted);
    assert_eq!(texts(del, LineKind::Removed), ["bye"]);

    let ren = find(&d, "new name.txt");
    assert_eq!(ren.status, FileStatus::Renamed);
    assert_eq!(ren.old_path.as_deref(), Some(Path::new("old name.txt")));
    assert_eq!((ren.added, ren.removed), (1, 1));
    assert!(!d.iter().any(|f| f.path == Path::new("old name.txt")));

    let bin = find(&d, "bin.dat");
    assert_eq!(bin.status, FileStatus::Binary);
    assert!(bin.hunks.is_empty());

    let ne = find(&d, "noeol.txt");
    assert_eq!(texts(ne, LineKind::Removed), ["last"]);
    assert_eq!(texts(ne, LineKind::Added), ["last changed"]);
    assert_eq!((ne.added, ne.removed), (1, 1));

    let cr = find(&d, "crlf.txt");
    assert_eq!(texts(cr, LineKind::Removed), ["beta"]);
    assert_eq!(texts(cr, LineKind::Added), ["BETA"]);
    assert_eq!(texts(cr, LineKind::Context), ["alpha", "gamma"]);

    let c = find(&d, "committed.txt");
    assert_eq!(texts(c, LineKind::Added), ["c2"]);

    let u = find(&d, "dir/ünï cödé \"q\".txt");
    assert_eq!(u.status, FileStatus::Added);
    assert_eq!(texts(u, LineKind::Added), ["hi"]);
    assert_eq!(u.hunks[0].lines[0].new_no, Some(1));
    let nl = find(&d, "nl\nname.txt");
    assert_eq!(nl.status, FileStatus::Added);

    // The real index is untouched: untracked files are still untracked.
    let st = status(&wt.dir).unwrap();
    assert!(st.untracked.contains(&PathBuf::from("dir/ünï cödé \"q\".txt")));
    assert!(st.staged.is_empty(), "{:?}", st.staged);

    let (added, removed) = diff_totals(&wt.dir, &base).unwrap();
    let sum_a: u32 = d.iter().map(|f| f.added).sum();
    let sum_r: u32 = d.iter().map(|f| f.removed).sum();
    assert_eq!((added, removed), (sum_a, sum_r));
    assert_eq!((added, removed), (1 + 1 + 1 + 1 + 1 + 1 + 1, 1 + 1 + 1 + 1 + 1));

    // No leftover temp index.
    let gd = sh(&wt.dir, &["rev-parse", "--absolute-git-dir"]);
    let leftovers: Vec<_> = std::fs::read_dir(gd.trim())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("qur-"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn task_diff_caps_large_files() {
    let mut r = Repo::new("big");
    r.write("keep.txt", "k\n");
    let base = r.commit("base");
    let wt = r.worktree("big");
    let big: String = (0..6000).map(|i| format!("row {i}\n")).collect();
    wt.write("huge.txt", big);
    let d = task_diff(&wt.dir, &base).unwrap();
    let f = find(&d, "huge.txt");
    assert_eq!(f.added, 6000);
    let lines: Vec<&DiffLine> = f.hunks.iter().flat_map(|h| &h.lines).collect();
    assert_eq!(lines.len(), MAX_LINES_PER_FILE + 1);
    let last = lines.last().unwrap();
    assert_eq!((last.old_no, last.new_no), (None, None));
    assert!(last.text.contains("1000 more"), "{}", last.text);
}

#[test]
fn task_diff_empty_when_nothing_changed() {
    let mut r = Repo::new("empty");
    r.write("a", "a\n");
    let base = r.commit("base");
    let wt = r.worktree("e");
    assert!(task_diff(&wt.dir, &base).unwrap().is_empty());
    assert_eq!(diff_totals(&wt.dir, &base).unwrap(), (0, 0));
    assert!(task_diff(&wt.dir, "--output=x").is_err());
}

#[test]
fn word_emphasis_on_paired_lines() {
    let (a, b) = diff::word_diff("let x = 1;", "let x = 2;");
    assert_eq!(a, one(8..9));
    assert_eq!(b, one(8..9));
    // Pure insertion: the removed side gets a zero-width point.
    let (a, b) = diff::word_diff("foo(a)", "foo(a, b)");
    assert_eq!(b, one(5..8));
    assert_eq!(a, one(5..5));
    // Nothing in common: whole line (empty).
    let (a, b) = diff::word_diff("alpha beta", "gamma delta");
    assert!(a.is_empty() && b.is_empty());
    // Unicode is sliced on char boundaries.
    let (a, b) = diff::word_diff("naïve café ok", "naïve thé ok");
    assert_eq!(&"naïve café ok"[a[0].clone()], "café");
    assert_eq!(&"naïve thé ok"[b[0].clone()], "thé");

    // Through a real diff: a 2-line -/+ run pairs line by line.
    let mut r = Repo::new("emph");
    r.write("f.rs", "fn a() {\n    let total = count + 1;\n    call(x);\n}\n");
    let base = r.commit("base");
    let wt = r.worktree("emph");
    wt.write("f.rs", "fn a() {\n    let total = count + 2;\n    call(x, y);\n    extra();\n}\n");
    let d = task_diff(&wt.dir, &base).unwrap();
    let lines: Vec<&DiffLine> = d[0].hunks[0].lines.iter().collect();
    let rem: Vec<&&DiffLine> = lines.iter().filter(|l| l.kind == LineKind::Removed).collect();
    let add: Vec<&&DiffLine> = lines.iter().filter(|l| l.kind == LineKind::Added).collect();
    assert_eq!(&rem[0].text[rem[0].emphasis[0].clone()], "1");
    assert_eq!(&add[0].text[add[0].emphasis[0].clone()], "2");
    assert_eq!(&add[1].text[add[1].emphasis[0].clone()], ", y");
    assert!(add[2].emphasis.is_empty(), "unpaired added line");
}

// ---------------------------------------------------------------- commit_all / merge

#[test]
fn commit_all_commits_everything_once() {
    let mut r = Repo::new("ca");
    r.write("a", "a\n");
    r.write(".gitignore", "*.tmp\n");
    r.commit("base");
    let wt = r.worktree("ca");
    assert_eq!(commit_all(&wt.dir, "qur: nothing").unwrap(), None);
    wt.write("a", "b\n");
    wt.write("new file.txt", "n\n");
    wt.write("x.tmp", "ignored\n");
    let c = commit_all(&wt.dir, "qur: task").unwrap().expect("a commit");
    assert_eq!(c, wt.head());
    assert_eq!(wt.git(&["log", "-1", "--format=%s|%an|%ae"]).trim(), "qur: task|Test User|test@example.com");
    let files = wt.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert!(files.contains("new file.txt") && files.contains('a') && !files.contains("x.tmp"));
    assert_eq!(commit_all(&wt.dir, "again").unwrap(), None);
    // The task branch moved; the main checkout did not.
    assert_ne!(r.head(), c);
}

#[test]
fn precheck_clean_and_conflict() {
    let mut r = Repo::new("pre");
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.commit("base");
    let wt = r.worktree("pre");
    wt.write("a.txt", "agent\n");
    commit_all(&wt.dir, "qur: a").unwrap();
    assert!(precheck(&r.dir, "qur/claude/pre").unwrap().is_empty());
    r.write("a.txt", "user\n");
    r.write("b.txt", "user b\n");
    let before = r.commit("user");
    assert_eq!(precheck(&r.dir, "qur/claude/pre").unwrap(), [PathBuf::from("a.txt")]);
    assert_eq!(r.head(), before);
    assert!(precheck(&r.dir, "no/such/branch").is_err());
}

/// Snapshot of everything the user owns in the checkout besides HEAD.
fn user_state(r: &Repo) -> (String, String, String, String) {
    (
        r.git(&["diff", "--cached", "--binary"]),
        r.git(&["diff", "--binary"]),
        r.git(&["stash", "list", "--format=%H %gs"]),
        r.git(&["ls-files", "--others", "--exclude-standard"]),
    )
}

#[test]
fn merge_clean_keeps_staged_unstaged_and_stash() {
    let mut r = Repo::new("merge");
    r.write("agent.txt", "base\n");
    r.write("staged.txt", "s0\n");
    r.write("unstaged.txt", "u0\n");
    r.write("stashed.txt", "t0\n");
    r.commit("base");
    let wt = r.worktree("m");
    wt.write("agent.txt", "base\nagent line\n");
    wt.write("agent new.txt", "brand new\n");
    commit_all(&wt.dir, "qur: agent work").unwrap();

    // User moves on and has dirty state of every kind.
    r.write("staged.txt", "s1\n");
    r.commit("user commit");
    r.write("stashed.txt", "t1\n");
    r.git(&["stash", "-q"]);
    r.write("staged.txt", "s2 staged\n");
    r.git(&["add", "staged.txt"]);
    r.write("staged.txt", "s3 staged then edited\n");
    r.write("unstaged.txt", "u1 unstaged\n");
    r.write("scratch.txt", "untracked\n");
    let old = r.head();
    let before = user_state(&r);

    let out = merge_clean(&r.dir, "qur/claude/m", "Merge qur/claude/m");
    let MergeOutcome::Merged { commit } = out else { panic!("{out:?}") };
    assert_eq!(r.head(), commit);
    assert_eq!(r.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "main");
    let parents = r.git(&["log", "-1", "--format=%P"]);
    let wt_head = wt.head();
    assert_eq!(parents.trim(), format!("{old} {wt_head}"));
    assert!(r.git(&["log", "--oneline"]).contains("Merge qur/claude/m"));
    assert!(r.git(&["log", "--format=%s"]).contains("qur: agent work"));
    assert_eq!(r.read("agent.txt"), "base\nagent line\n");
    assert_eq!(r.read("agent new.txt"), "brand new\n");
    assert_eq!(user_state(&r), before, "user's index, worktree diff, stash and untracked unchanged");
    assert_eq!(r.read("staged.txt"), "s3 staged then edited\n");
    assert!(!is_merging(&r.dir));

    // Merging again: up to date.
    assert_eq!(merge_clean(&r.dir, "qur/claude/m", "again"), MergeOutcome::UpToDate);
}

#[test]
fn merge_clean_refuses_overlapping_dirty_file() {
    let mut r = Repo::new("refuse");
    r.write("f.txt", "base\n");
    r.write("g.txt", "g\n");
    r.commit("base");
    let wt = r.worktree("r");
    wt.write("f.txt", "base\nagent\n");
    wt.write("g.txt", "g agent\n");
    wt.write("h.txt", "new from agent\n");
    commit_all(&wt.dir, "qur: f").unwrap();
    r.write("f.txt", "base\nuser dirty\n");
    r.write("g.txt", "g staged\n");
    r.git(&["add", "g.txt"]);
    r.write("h.txt", "user's untracked h\n");
    let old = r.head();
    let before = user_state(&r);
    let out = merge_clean(&r.dir, "qur/claude/r", "m");
    let MergeOutcome::Refused { reason } = out else { panic!("{out:?}") };
    assert!(reason.contains("f.txt") && reason.contains("g.txt") && reason.contains("h.txt"), "{reason}");
    assert_eq!(r.head(), old);
    assert_eq!(user_state(&r), before);
    assert_eq!(r.read("f.txt"), "base\nuser dirty\n");
    assert_eq!(r.read("h.txt"), "user's untracked h\n");
}

#[test]
fn merge_clean_reports_conflicts_without_touching() {
    let mut r = Repo::new("mconf");
    r.write("c.txt", "base\n");
    r.commit("base");
    let wt = r.worktree("c");
    wt.write("c.txt", "agent\n");
    commit_all(&wt.dir, "qur: c").unwrap();
    r.write("c.txt", "user\n");
    let old = r.commit("user");
    r.write("other.txt", "dirty untracked\n");
    let before = user_state(&r);
    assert_eq!(merge_clean(&r.dir, "qur/claude/c", "m"), MergeOutcome::Conflicts { files: vec!["c.txt".into()] });
    assert_eq!(r.head(), old);
    assert_eq!(user_state(&r), before);
    assert!(matches!(merge_clean(&r.dir, "nope", "m"), MergeOutcome::Refused { .. }));
}

fn conflict_setup(name: &str) -> (Repo, Repo) {
    let mut r = Repo::new(name);
    r.write("one.txt", "head\nmid\ntail\n");
    r.write("two.txt", "x\n");
    r.write("three.txt", "p\n");
    r.commit("base");
    let wt = r.worktree(name);
    wt.write("one.txt", "head\nagent mid\ntail\n");
    wt.write("two.txt", "agent x\n");
    wt.write("three.txt", "agent p\n");
    wt.write("clean.txt", "agent only\n");
    commit_all(&wt.dir, "qur: conflicting").unwrap();
    r.write("one.txt", "head\nuser mid\ntail\n");
    r.write("two.txt", "user x\n");
    r.write("three.txt", "user p\n");
    r.commit("user");
    (r, wt)
}

#[test]
fn conflict_merge_resolve_and_finish() {
    let (r, _wt) = conflict_setup("cm");
    let branch = "qur/claude/cm";
    r.write("dirty.txt", "x");
    r.git(&["add", "dirty.txt"]);
    let err = start_conflict_merge(&r.dir, branch).unwrap_err();
    assert!(err.contains("dirty.txt"), "{err}");
    r.git(&["rm", "-q", "--cached", "dirty.txt"]);
    std::fs::remove_file(r.dir.join("dirty.txt")).unwrap();

    let mut files = start_conflict_merge(&r.dir, branch).unwrap();
    files.sort();
    assert_eq!(files, [PathBuf::from("one.txt"), "three.txt".into(), "two.txt".into()]);
    assert!(is_merging(&r.dir));
    assert_eq!(conflicted_files(&r.dir).len(), 3);
    assert_eq!(status(&r.dir).unwrap().conflicted.len(), 3);
    assert!(status(&r.dir).unwrap().merging);

    let one = r.read("one.txt");
    let c = find_conflicts(&one);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].ours_label, "HEAD");
    assert_eq!(c[0].theirs_label, branch);
    r.write("one.txt", resolve(&one, 0, Resolution::Ours));
    r.write("two.txt", resolve(&r.read("two.txt"), 0, Resolution::Theirs));

    // three.txt still has markers: refuse, naming it.
    let err = finish_merge(&r.dir, &files, "Merge it").unwrap_err();
    assert!(err.contains("three.txt") && !err.contains("one.txt"), "{err}");
    assert!(is_merging(&r.dir));

    r.write("three.txt", resolve(&r.read("three.txt"), 0, Resolution::Both));
    let c = finish_merge(&r.dir, &files, "Merge it").unwrap();
    assert_eq!(r.head(), c);
    assert!(!is_merging(&r.dir));
    assert_eq!(r.read("one.txt"), "head\nuser mid\ntail\n");
    assert_eq!(r.read("two.txt"), "agent x\n");
    assert_eq!(r.read("three.txt"), "user p\nagent p\n");
    assert_eq!(r.read("clean.txt"), "agent only\n");
    assert_eq!(r.git(&["log", "-1", "--format=%s"]).trim(), "Merge it");
    assert_eq!(r.git(&["log", "-1", "--format=%P"]).split_whitespace().count(), 2);
    assert!(r.git(&["status", "--porcelain"]).is_empty());
}

#[test]
fn abort_merge_restores() {
    let (r, _wt) = conflict_setup("ab");
    let old = r.head();
    r.write("untracked keep.txt", "keep\n");
    start_conflict_merge(&r.dir, "qur/claude/ab").unwrap();
    assert!(r.read("one.txt").contains("<<<<<<<"));
    abort_merge(&r.dir).unwrap();
    assert!(!is_merging(&r.dir));
    assert_eq!(r.head(), old);
    assert_eq!(r.read("one.txt"), "head\nuser mid\ntail\n");
    assert!(!r.dir.join("clean.txt").exists());
    assert_eq!(r.read("untracked keep.txt"), "keep\n");
    assert!(abort_merge(&r.dir).is_err());
}

// ---------------------------------------------------------------- conflict text

#[test]
fn find_and_resolve_conflicts() {
    let t = "a\n<<<<<<< HEAD\nours1\nours2\n=======\ntheirs\n>>>>>>> qur/x/y\nb\n<<<<<<< ours\nO\n||||||| base\nB\n=======\nT\n>>>>>>> theirs\nc\n";
    let c = find_conflicts(t);
    assert_eq!(
        c,
        [
            Conflict { start: 1, base: None, mid: 4, end: 6, ours_label: "HEAD".into(), theirs_label: "qur/x/y".into() },
            Conflict { start: 8, base: Some(10), mid: 12, end: 14, ours_label: "ours".into(), theirs_label: "theirs".into() },
        ]
    );
    assert_eq!(resolve(t, 0, Resolution::Ours), t.replace("<<<<<<< HEAD\nours1\nours2\n=======\ntheirs\n>>>>>>> qur/x/y\n", "ours1\nours2\n"));
    let r1 = resolve(t, 1, Resolution::Theirs);
    assert!(r1.ends_with("b\nT\nc\n") && r1.contains("<<<<<<< HEAD"));
    assert!(resolve(t, 1, Resolution::Ours).ends_with("b\nO\nc\n"), "diff3 base is dropped");
    assert!(resolve(t, 0, Resolution::Both).starts_with("a\nours1\nours2\ntheirs\nb\n"));
    assert_eq!(resolve(t, 5, Resolution::Ours), t);
    // Resolving all, one by one (indices shift as conflicts go away).
    let all = resolve(&resolve(t, 0, Resolution::Theirs), 0, Resolution::Theirs);
    assert_eq!(all, "a\ntheirs\nb\nT\nc\n");
    assert!(find_conflicts(&all).is_empty());

    // CRLF endings are preserved.
    let crlf = "x\r\n<<<<<<< HEAD\r\nO\r\n=======\r\nT\r\n>>>>>>> b\r\ny";
    assert_eq!(find_conflicts(crlf).len(), 1);
    assert_eq!(resolve(crlf, 0, Resolution::Both), "x\r\nO\r\nT\r\ny");
    // Unbalanced markers are ignored; an empty side resolves to nothing.
    assert!(find_conflicts("<<<<<<< a\nno end\n=======\n").is_empty());
    assert!(find_conflicts("<<<<<<<< not a marker\n=======\n>>>>>>> x\n").is_empty());
    assert_eq!(resolve("<<<<<<< a\n=======\nT\n>>>>>>> b\n", 0, Resolution::Ours), "");
}

// ---------------------------------------------------------------- Git panel

#[test]
fn status_parses_everything() {
    let mut r = Repo::new("st");
    r.write("mod.txt", "m\n");
    r.write("del.txt", "d\n");
    r.write("ren from.txt", "rename me please, enough content\n");
    r.write("both.txt", "b\n");
    r.write("conf.txt", "base\n");
    r.commit("base");
    // A conflict on conf.txt.
    let wt = r.worktree("st");
    wt.write("conf.txt", "agent\n");
    commit_all(&wt.dir, "qur").unwrap();
    r.write("conf.txt", "user\n");
    r.commit("user");
    start_conflict_merge(&r.dir, "qur/claude/st").unwrap();

    r.write("mod.txt", "m2\n");
    std::fs::remove_file(r.dir.join("del.txt")).unwrap();
    r.git(&["mv", "ren from.txt", "ren tö.txt"]);
    r.write("both.txt", "b staged\n");
    r.git(&["add", "both.txt"]);
    r.write("both.txt", "b staged then edited\n");
    r.write("new\tfile ü.txt", "n\n");
    r.write("added.txt", "a\n");
    r.git(&["add", "added.txt"]);

    let s = status(&r.dir).unwrap();
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.head.as_deref(), Some(r.head().as_str()));
    assert!(s.merging);
    assert_eq!(s.conflicted, [PathBuf::from("conf.txt")]);
    assert_eq!(s.untracked, [PathBuf::from("new\tfile ü.txt")]);
    let staged: Vec<(String, char, Option<PathBuf>)> =
        s.staged.iter().map(|e| (e.path.display().to_string(), e.code, e.orig.clone())).collect();
    assert!(staged.contains(&("ren tö.txt".into(), 'R', Some("ren from.txt".into()))), "{staged:?}");
    assert!(staged.contains(&("both.txt".into(), 'M', None)));
    assert!(staged.contains(&("added.txt".into(), 'A', None)));
    let unstaged: Vec<(String, char)> = s.unstaged.iter().map(|e| (e.path.display().to_string(), e.code)).collect();
    assert!(unstaged.contains(&("mod.txt".into(), 'M')), "{unstaged:?}");
    assert!(unstaged.contains(&("del.txt".into(), 'D')));
    assert!(unstaged.contains(&("both.txt".into(), 'M')));
    assert_eq!(s.ahead, 0);
    // git refuses to abort over the unrelated staged edits; that error surfaces.
    assert!(abort_merge(&r.dir).is_err());
}

#[test]
fn status_ahead_behind_and_detached() {
    let remote = scratch("remote.git");
    sh(remote.parent().unwrap(), &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
    let r = Repo::new("ab1");
    r.write("a", "a\n");
    r.commit("base");
    r.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    r.git(&["push", "-q", "-u", "origin", "main"]);
    let s = status(&r.dir).unwrap();
    assert_eq!((s.ahead, s.behind), (0, 0));

    // Someone else pushes two commits.
    let other = scratch("ab-other");
    sh(remote.parent().unwrap(), &["clone", "-q", remote.to_str().unwrap(), other.to_str().unwrap()]);
    configure(&other);
    for i in 0..2 {
        std::fs::write(other.join("o"), format!("{i}\n")).unwrap();
        sh(&other, &["add", "-A"]);
        sh(&other, &["commit", "-q", "-m", "other"]);
    }
    sh(&other, &["push", "-q"]);
    // We commit one locally and fetch.
    r.write("b", "b\n");
    r.commit("local");
    r.git(&["fetch", "-q"]);
    let s = status(&r.dir).unwrap();
    assert_eq!((s.ahead, s.behind), (1, 2));

    r.git(&["checkout", "-q", "--detach"]);
    let s = status(&r.dir).unwrap();
    assert_eq!(s.branch, None);
    assert!(s.head.is_some());
    let _ = std::fs::remove_dir_all(&remote);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn stage_unstage_commit_discard() {
    let r = Repo::new("panel");
    // No commits yet: stage + unstage.
    r.write("first.txt", "1\n");
    stage(&r.dir, &["first.txt".into()]).unwrap();
    assert_eq!(status(&r.dir).unwrap().head, None);
    assert_eq!(status(&r.dir).unwrap().staged.len(), 1);
    unstage(&r.dir, &["first.txt".into()]).unwrap();
    let s = status(&r.dir).unwrap();
    assert!(s.staged.is_empty());
    assert_eq!(s.untracked, [PathBuf::from("first.txt")]);
    assert!(commit(&r.dir, "nothing").unwrap_err().contains("Nothing"));
    stage(&r.dir, &[r.dir.join("first.txt")]).unwrap(); // absolute path works too
    assert!(commit(&r.dir, "  \n").is_err());
    let c1 = commit(&r.dir, "first").unwrap();
    assert_eq!(r.head(), c1);

    r.write("first.txt", "1\n2\n");
    r.write("odd name 'q'.txt", "q\n");
    r.write("other.txt", "o\n");
    stage(&r.dir, &["odd name 'q'.txt".into()]).unwrap();
    let c2 = commit(&r.dir, "second").unwrap();
    let shown = r.git(&["show", "--name-only", "--format=", &c2]);
    assert!(shown.contains("odd name 'q'.txt") && !shown.contains("first.txt") && !shown.contains("other.txt"));

    // Deletions stage too.
    std::fs::remove_file(r.dir.join("odd name 'q'.txt")).unwrap();
    stage(&r.dir, &["odd name 'q'.txt".into()]).unwrap();
    assert_eq!(status(&r.dir).unwrap().staged[0].code, 'D');
    unstage(&r.dir, &["odd name 'q'.txt".into()]).unwrap();
    assert!(status(&r.dir).unwrap().staged.is_empty());

    // Unstaging a rename's new path unstages both sides.
    r.git(&["mv", "first.txt", "renamed.txt"]);
    unstage(&r.dir, &["renamed.txt".into()]).unwrap();
    assert!(status(&r.dir).unwrap().staged.is_empty());
    std::fs::rename(r.dir.join("renamed.txt"), r.dir.join("first.txt")).unwrap();

    // Discard: tracked edits restored from the index, untracked deleted, staged kept.
    r.write("first.txt", "1\n2\n3\n");
    stage(&r.dir, &["first.txt".into()]).unwrap();
    r.write("first.txt", "garbage\n");
    discard_unstaged(&r.dir, &["first.txt".into(), "other.txt".into(), "odd name 'q'.txt".into()]).unwrap();
    assert_eq!(r.read("first.txt"), "1\n2\n3\n");
    assert!(!r.dir.join("other.txt").exists());
    assert_eq!(r.read("odd name 'q'.txt"), "q\n");
    let s = status(&r.dir).unwrap();
    assert!(s.unstaged.is_empty() && s.untracked.is_empty());
    assert_eq!(s.staged.len(), 1);
    assert!(discard_unstaged(&r.dir, &["../escape".into()]).is_err());
    assert!(stage(&r.dir, &["/etc/passwd".into()]).is_err());
}

#[test]
fn file_diff_staged_unstaged_untracked() {
    let r = Repo::new("fd");
    r.write("f.txt", "a\nb\nc\n");
    r.write("mv src.txt", "same content here\nand here\n");
    r.commit("base");
    r.write("f.txt", "a\nB\nc\n");
    r.git(&["add", "f.txt"]);
    r.write("f.txt", "a\nB\nc\nd\n");
    r.write("new ü.txt", "x\ny\n");
    r.git(&["mv", "mv src.txt", "mv dst.txt"]);

    let st = file_diff(&r.dir, Path::new("f.txt"), true).unwrap().unwrap();
    assert_eq!(texts(&st, LineKind::Removed), ["b"]);
    assert_eq!(texts(&st, LineKind::Added), ["B"]);
    let un = file_diff(&r.dir, Path::new("f.txt"), false).unwrap().unwrap();
    assert!(texts(&un, LineKind::Removed).is_empty());
    assert_eq!(texts(&un, LineKind::Added), ["d"]);

    let u = file_diff(&r.dir, Path::new("new ü.txt"), false).unwrap().unwrap();
    assert_eq!(u.status, FileStatus::Added);
    assert_eq!(texts(&u, LineKind::Added), ["x", "y"]);
    assert_eq!(file_diff(&r.dir, Path::new("new ü.txt"), true).unwrap(), None);

    let mv = file_diff(&r.dir, Path::new("mv dst.txt"), true).unwrap().unwrap();
    assert_eq!(mv.status, FileStatus::Renamed);
    assert_eq!(mv.old_path.as_deref(), Some(Path::new("mv src.txt")));
    assert!(mv.hunks.is_empty());

    assert_eq!(file_diff(&r.dir, Path::new("missing.txt"), false).unwrap(), None);
    // Untracked check left no index behind and didn't stage anything.
    assert!(status(&r.dir).unwrap().untracked.contains(&PathBuf::from("new ü.txt")));
}

/// Timing on the 5,000-file fixture (`python3 spikes/fixtures/gen.py`):
/// `cargo test --release -p qur_git -- --ignored --nocapture big_repo`.
#[test]
#[ignore]
fn big_repo_task_diff_timing() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/fixtures/big-repo");
    assert!(fixture.join(".git").exists(), "run spikes/fixtures/gen.py first");
    let clone = scratch("bigclone");
    sh(Path::new("/"), &["clone", "-q", "--local", fixture.to_str().unwrap(), clone.to_str().unwrap()]);
    configure(&clone);
    let mut r = Repo { dir: clone, extra: Vec::new() };
    let base = r.head();
    let wt = r.worktree("bench");
    let files: Vec<String> =
        wt.git(&["ls-files"]).lines().filter(|l| !l.ends_with(".png")).take(50).map(String::from).collect();
    for (i, f) in files.iter().enumerate() {
        let p = wt.dir.join(f);
        let mut s = std::fs::read_to_string(&p).unwrap_or_default();
        s.insert_str(0, &format!("// edit {i}\n"));
        s.push_str("// tail edit\n");
        std::fs::write(&p, s).unwrap();
    }
    wt.write("brand/new/file.rs", "fn main() {}\n");
    // Cold: first call right after creating the worktree (every index entry is racy).
    let t = std::time::Instant::now();
    task_diff(&wt.dir, &base).unwrap();
    let cold = t.elapsed();
    // Agents write over seconds; once the racy window has passed, calls are steady.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut times = Vec::new();
    let mut n = 0;
    for _ in 0..7 {
        let t = std::time::Instant::now();
        let d = task_diff(&wt.dir, &base).unwrap();
        times.push(t.elapsed());
        n = d.len();
    }
    let t = std::time::Instant::now();
    let tot = diff_totals(&wt.dir, &base).unwrap();
    let tt = t.elapsed();
    times.sort();
    println!(
        "big-repo task_diff: {n} files, cold {cold:?}; settled min {:?} median {:?} max {:?}; diff_totals {:?} in {tt:?}",
        times[0], times[3], times[6], tot
    );
    assert_eq!(n, 51);
}
