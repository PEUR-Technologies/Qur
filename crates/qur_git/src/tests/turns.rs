//! Turn checkpoints: snapshots, per-turn diffs, Rewind, ref cleanup.

use super::*;

/// Every file in a checkout (not .git), sorted, with contents.
fn files_of(r: &Repo) -> Vec<(String, String)> {
    fn walk(base: &Path, d: &Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.push((p.strip_prefix(base).unwrap().display().to_string(), std::fs::read_to_string(&p).unwrap_or_default()));
            }
        }
    }
    let mut v = Vec::new();
    walk(&r.dir, &r.dir, &mut v);
    v.sort();
    v
}

fn index_path_of(r: &Repo) -> PathBuf {
    let p = r.git(&["rev-parse", "--git-path", "index"]);
    let p = PathBuf::from(p.trim());
    if p.is_absolute() { p } else { r.dir.join(p) }
}

/// The user's dirty main checkout with a stash entry, and a task worktree.
fn turn_setup(name: &str) -> (Repo, Repo) {
    let mut r = Repo::new(name);
    r.write(".gitignore", "*.log\n");
    r.write("a.txt", "a0\n");
    r.write("b.txt", "b0\n");
    r.write("dir/c.txt", "c0\n");
    r.commit("base");
    r.write("b.txt", "b stashed\n");
    r.git(&["stash", "-q"]);
    r.write("a.txt", "a staged\n");
    r.git(&["add", "a.txt"]);
    r.write("a.txt", "a staged then edited\n");
    r.write("mine.txt", "user untracked\n");
    let wt = r.worktree(name);
    (r, wt)
}

#[test]
fn turn_snapshot_and_rewind_keep_user_index_and_stash() {
    let (r, wt) = turn_setup("turns");
    let before = user_state(&r);

    // Turn 1 starts from a worktree that already has uncommitted work.
    wt.write("a.txt", "a agent pre\n");
    wt.write("pre-untracked.txt", "pre\n");
    let wt_index = wt.git(&["ls-files", "-s"]);
    let start = snapshot_tree(&wt.dir).unwrap();
    assert_eq!(snapshot_tree(&wt.dir).unwrap(), start, "same files, same tree");
    assert_eq!(wt.git(&["ls-files", "-s"]), wt_index, "a snapshot stages nothing in the worktree's index");
    assert!(!index_path_of(&wt).with_extension("lock").exists());
    let at_start = files_of(&wt);

    // The turn edits, deletes, adds, commits and leaves an ignored file.
    wt.write("a.txt", "a agent pre\nturn line\n");
    std::fs::remove_file(wt.dir.join("b.txt")).unwrap();
    wt.write("dir/new.txt", "new\n");
    wt.git(&["add", "dir/new.txt"]);
    wt.write("dir/c.txt", "c1\n");
    wt.git(&["commit", "-q", "-am", "agent commit"]);
    wt.write("later-untracked.txt", "later\n");
    wt.write("build.log", "ignored\n");
    let end = snapshot_tree(&wt.dir).unwrap();
    assert_ne!(start, end);

    let d = tree_diff(&wt.dir, &start, &end).unwrap();
    assert_eq!(
        paths(&d),
        ["a.txt", "b.txt", "dir/c.txt", "dir/new.txt", "later-untracked.txt"].map(PathBuf::from),
        "ignored files aren't part of a turn"
    );
    assert_eq!(find(&d, "b.txt").status, FileStatus::Deleted);
    assert_eq!(texts(find(&d, "a.txt"), LineKind::Added), ["turn line"]);
    // a +1, b −1, c +1 −1, new +1, later +1.
    assert_eq!(tree_stats(&wt.dir, &start, &end).unwrap(), (5, 4, 2));

    // Pinned trees survive gc; the refs don't show up as history.
    pin_turn(&r.dir, "turns", 1, "start", &start).unwrap();
    pin_turn(&r.dir, "turns", 1, "end", &end).unwrap();
    r.git(&["gc", "-q", "--prune=now"]);
    r.git(&["cat-file", "-e", &format!("{start}^{{tree}}")]);
    r.git(&["cat-file", "-e", &format!("{end}^{{tree}}")]);
    assert_eq!(r.git(&["log", "--all", "--format=%s"]).lines().filter(|l| *l == "base").count(), 1);

    // Rewind: files back to the start, the ignored file stays, HEAD stays.
    let head = wt.head();
    restore_tree(&r.dir, &wt.dir, &start).unwrap();
    let mut expect = at_start.clone();
    expect.push(("build.log".into(), "ignored\n".into()));
    expect.sort();
    assert_eq!(files_of(&wt), expect);
    assert_eq!(snapshot_tree(&wt.dir).unwrap(), start);
    assert_eq!(wt.head(), head, "Rewind doesn't move the task's branch");
    assert!(wt.git(&["diff", "--cached", "--name-only"]).is_empty(), "the worktree's index is back at HEAD");
    // And forward again to the end.
    restore_tree(&r.dir, &wt.dir, &end).unwrap();
    assert_eq!(snapshot_tree(&wt.dir).unwrap(), end);

    // The user's checkout was never touched.
    assert_eq!(user_state(&r), before, "user's index, worktree diff, stash and untracked unchanged");
    assert_eq!(r.read("a.txt"), "a staged then edited\n");

    assert_eq!(
        changed_paths_between(&wt.dir, &start, &end).unwrap(),
        ["a.txt", "b.txt", "dir/c.txt", "dir/new.txt", "later-untracked.txt"].map(PathBuf::from)
    );

    // A turn that changed nothing drops just its own refs; another task's stay.
    pin_turn(&r.dir, "turns", 2, "start", &end).unwrap();
    pin_turn(&r.dir, "turns-other", 1, "start", &end).unwrap();
    unpin_turn(&r.dir, "turns", 2).unwrap();
    let refs = r.git(&["for-each-ref", "--format=%(refname)", "refs/qur/"]);
    assert_eq!(refs.lines().collect::<Vec<_>>(), ["refs/qur/turns/turns-other/1/start", "refs/qur/turns/turns/1/end", "refs/qur/turns/turns/1/start"]);

    // Discard/Merge cleanup.
    delete_turn_refs(&r.dir, "turns-other").unwrap();
    delete_turn_refs(&r.dir, "turns").unwrap();
    assert!(r.git(&["for-each-ref", "refs/qur/"]).is_empty());
    delete_turn_refs(&r.dir, "turns").unwrap();
}

#[test]
fn turn_snapshot_of_main_checkout_leaves_it_alone() {
    let (r, _wt) = turn_setup("turns-main");
    let before = user_state(&r);
    let index = std::fs::read(r.dir.join(".git/index")).unwrap();
    let t = snapshot_tree(&r.dir).unwrap();
    let ls = r.git(&["ls-tree", "-r", "--name-only", &t]);
    assert!(ls.lines().any(|l| l == "mine.txt"), "{ls}");
    assert_eq!(user_state(&r), before);
    assert_eq!(std::fs::read(r.dir.join(".git/index")).unwrap(), index, "the user's index file is byte for byte the same");
}

#[test]
fn restore_refuses_the_project_folder() {
    let (r, wt) = turn_setup("turns-refuse");
    let t = snapshot_tree(&wt.dir).unwrap();
    let before = user_state(&r);
    assert!(restore_tree(&r.dir, &r.dir, &t).unwrap_err().contains("project folder"));
    // A main checkout passed as the "worktree" is refused too.
    let other = Repo::new("turns-other");
    assert!(restore_tree(&r.dir, &other.dir, &t).unwrap_err().contains("not a task worktree"));
    assert!(restore_tree(&r.dir, &wt.dir, "--hard").is_err());
    assert!(restore_tree(&r.dir, &wt.dir, "0123456789012345678901234567890123456789").unwrap_err().contains("gone"));
    assert!(turn_ref("../x", 1, "start").is_err() && turn_ref("a b", 1, "end").is_err() && turn_ref("fix-nav", 1, "mid").is_err());
    assert_eq!(turn_ref("fix-nav", 3, "end").unwrap(), "refs/qur/turns/fix-nav/3/end");
    assert_eq!(user_state(&r), before);
}

/// Snapshot timing on a clone of the 5,000-file fixture (`python3
/// spikes/fixtures/gen.py`; `QUR_BIG_REPO=<path>` to clone another copy):
/// `cargo test --release -p qur_git -- --ignored --nocapture big_repo_snapshot`.
#[test]
#[ignore]
fn big_repo_snapshot_timing() {
    let fixture = std::env::var_os("QUR_BIG_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/fixtures/big-repo"));
    assert!(fixture.join(".git").exists(), "run spikes/fixtures/gen.py first");
    let clone = scratch("bigclone-turns");
    sh(Path::new("/"), &["clone", "-q", "--local", fixture.to_str().unwrap(), clone.to_str().unwrap()]);
    configure(&clone);
    let mut r = Repo { dir: clone, extra: Vec::new() };
    let wt = r.worktree("bench");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let t = std::time::Instant::now();
    let start = snapshot_tree(&wt.dir).unwrap();
    let first = t.elapsed();
    let files: Vec<String> = wt.git(&["ls-files"]).lines().filter(|l| !l.ends_with(".png")).take(50).map(String::from).collect();
    for f in &files {
        let p = wt.dir.join(f);
        let mut s = std::fs::read_to_string(&p).unwrap_or_default();
        s.push_str("// turn edit\n");
        std::fs::write(&p, s).unwrap();
    }
    wt.write("brand/new/file.rs", "fn main() {}\n");
    let mut times = Vec::new();
    let mut end = String::new();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        end = snapshot_tree(&wt.dir).unwrap();
        times.push(t.elapsed());
    }
    times.sort();
    let t = std::time::Instant::now();
    let stats = tree_stats(&wt.dir, &start, &end).unwrap();
    let st = t.elapsed();
    let t = std::time::Instant::now();
    restore_tree(&r.dir, &wt.dir, &start).unwrap();
    let rt = t.elapsed();
    eprintln!(
        "big-repo snapshot: first {first:?}; after 51 changes min {:?} median {:?} max {:?}; tree_stats {stats:?} in {st:?}; restore {rt:?}",
        times[0], times[2], times[4]
    );
    assert_eq!(stats.0, 51);
    assert_eq!(snapshot_tree(&wt.dir).unwrap(), start);
}
