use super::*;
use std::fs;

/// A fresh, canonical temp folder (FSEvents reports canonical paths).
fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("qur-watch-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// A repo-shaped fixture with rules at three depths and every kind of ignore file.
fn fixture(name: &str) -> PathBuf {
    let d = tmp(name);
    write(&d, ".git/HEAD", "ref: refs/heads/main\n");
    write(&d, ".git/info/exclude", "*.local\n");
    write(&d, ".gitignore", "target/\n*.log\n");
    write(&d, "web/.gitignore", "dist/\n!keep.log\n");
    write(&d, "web/src/.gitignore", "*.gen.ts\n");
    write(&d, "docs/.ignore", "draft.md\n");
    d
}

fn ign(i: &mut Ignores, rel: &str) -> bool {
    i.is_ignored(Path::new(rel), false)
}

#[test]
fn nested_rules_like_the_tree() {
    let d = fixture("rules");
    let mut i = Ignores::new(&d);
    // Root .gitignore.
    assert!(ign(&mut i, "target/debug/x"), "inside an ignored folder");
    assert!(i.is_ignored(Path::new("target"), true));
    assert!(ign(&mut i, "a.log"));
    assert!(ign(&mut i, "web/deep/b.log"), "root patterns apply at any depth");
    // Nested .gitignore, one and two levels down.
    assert!(ign(&mut i, "web/dist/app.js"));
    assert!(!ign(&mut i, "dist/app.js"), "web/.gitignore doesn't reach outside web/");
    assert!(ign(&mut i, "web/src/api.gen.ts"));
    assert!(!ign(&mut i, "web/api.gen.ts"), "web/src rules stay in web/src");
    // A deeper `!` re-includes what a shallower file ignores.
    assert!(!ign(&mut i, "web/keep.log"));
    assert!(ign(&mut i, "web/other.log"));
    // .ignore, .git/info/exclude.
    assert!(ign(&mut i, "docs/draft.md"));
    assert!(!ign(&mut i, "docs/final.md"));
    assert!(ign(&mut i, "web/src/secrets.local"));
    // Plain files.
    for p in ["src/main.rs", "web/src/app.ts", "README.md", ".gitignore", "web/.gitignore"] {
        assert!(!ign(&mut i, p), "{p}");
    }
}

#[test]
fn matchers_are_cached_and_invalidated() {
    let d = fixture("cache");
    let mut i = Ignores::new(&d);
    assert!(!ign(&mut i, "web/src/notes.txt"));
    let cached = i.cached();
    assert_eq!(cached, 3, "root, web, web/src");
    for _ in 0..100 {
        ign(&mut i, "web/src/notes.txt");
    }
    assert_eq!(i.cached(), cached, "no reloads per event");

    // Editing a nested .gitignore only takes effect after invalidation (the watcher
    // invalidates on the .gitignore's own event).
    write(&d, "web/src/.gitignore", "*.gen.ts\nnotes.txt\n");
    assert!(!ign(&mut i, "web/src/notes.txt"), "still the cached rules");
    assert!(i.invalidate(Path::new("web/src/.gitignore")));
    assert!(ign(&mut i, "web/src/notes.txt"));

    // A new .gitignore in a folder that had none.
    assert!(!ign(&mut i, "api/x.tmp"));
    write(&d, "api/.gitignore", "*.tmp\n");
    assert!(i.invalidate(Path::new("api/.gitignore")));
    assert!(ign(&mut i, "api/x.tmp"));

    // A deleted one stops applying.
    fs::remove_file(d.join("web/.gitignore")).unwrap();
    i.invalidate(Path::new("web/.gitignore"));
    assert!(!ign(&mut i, "web/dist/app.js"));

    // info/exclude reloads.
    write(&d, ".git/info/exclude", "*.bak\n");
    assert!(i.invalidate(Path::new(".git/info/exclude")));
    assert!(ign(&mut i, "x.bak") && !ign(&mut i, "x.local"));

    assert!(!i.invalidate(Path::new("src/main.rs")), "ordinary files don't invalidate");
}

#[test]
fn filter_drops_git_internals_temp_files_and_ignored() {
    let d = fixture("filter");
    let mut f = Filter::new(&d);
    let ok = |f: &mut Filter, rel: &str| f.accept(d.join(rel)).is_some();
    assert!(ok(&mut f, ".git/HEAD") && ok(&mut f, ".git/index"));
    assert!(!ok(&mut f, ".git/objects/ab/cdef") && !ok(&mut f, ".git/index.lock"));
    assert!(!ok(&mut f, "vendor/lib/.git/HEAD"), "nested repos' git folders");
    assert!(!ok(&mut f, "src/.main.rs.qur-123.tmp"), "editor save temp");
    assert!(!ok(&mut f, "src/main.qur-replace.tmp"), "Replace All temp");
    assert!(!ok(&mut f, "web/dist/app.js"));
    assert!(ok(&mut f, "src/main.rs") && ok(&mut f, "web/.gitignore") && ok(&mut f, ".gitignore"));
    assert!(f.accept(d.clone()).is_none(), "the root itself only comes from rescans");
    assert!(f.accept(PathBuf::from("/elsewhere/x")).is_none());
}

#[test]
fn qur_temp_names() {
    assert!(is_qur_temp(".main.rs.qur-4242.tmp"));
    assert!(is_qur_temp("a.qur-replace.tmp"));
    assert!(!is_qur_temp("main.rs.qur-4242.tmp"), "the editor's temp is a dotfile");
    assert!(!is_qur_temp(".main.rs.qur-.tmp"));
    assert!(!is_qur_temp(".main.rs.qur-12a.tmp"));
    assert!(!is_qur_temp("notes.tmp") && !is_qur_temp(".env"));
}

/// Collects batches until `done` holds for everything seen so far, or the timeout.
fn collect(rx: &mut Changes, timeout: Duration, done: impl Fn(&[PathBuf]) -> bool) -> Vec<PathBuf> {
    let t0 = Instant::now();
    let mut seen = Vec::new();
    while t0.elapsed() < timeout {
        match rx.try_recv() {
            Ok(batch) => {
                seen.extend(batch);
                if done(&seen) {
                    break;
                }
            }
            Err(e) if e.is_closed() => break,
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    seen
}

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn fsevents_move_into_an_ignored_folder_reads_as_a_delete() {
    // `mv src/a.rs target/` (an agent "deleting" by moving into an ignored folder): the
    // old path is reported (it no longer exists), the new one isn't.
    let d = fixture("fsev-mv");
    write(&d, "src/a.rs", "x");
    std::thread::sleep(Duration::from_millis(100));
    let (_w, mut rx) = Watcher::new(&d, Duration::from_millis(50)).unwrap();
    fs::create_dir_all(d.join("target")).unwrap();
    let (from, to) = (d.join("src/a.rs"), d.join("target/a.rs"));
    fs::rename(&from, &to).unwrap();
    let seen = collect(&mut rx, WAIT, |s| s.contains(&from));
    assert!(seen.contains(&from), "{seen:?}");
    assert!(!seen.iter().any(|p| p.starts_with(d.join("target"))), "{seen:?}");
}

#[test]
fn fsevents_respect_nested_gitignore_live() {
    let d = fixture("fsev-ign");
    std::thread::sleep(Duration::from_millis(100));
    let (_w, mut rx) = Watcher::new(&d, Duration::from_millis(50)).unwrap();
    let sentinel = d.join("web/src/app.ts");
    write(&d, "web/dist/app.js", "x");
    write(&d, "web/src/api.gen.ts", "x");
    write(&d, "target/debug/out", "x");
    write(&d, "docs/draft.md", "x");
    write(&d, "web/src/.app.ts.qur-1.tmp", "x");
    write(&d, "web/src/app.ts", "x");
    let seen = collect(&mut rx, WAIT, |s| s.contains(&sentinel));
    assert!(seen.contains(&sentinel), "{seen:?}");
    // The ignored folders themselves are created too; nothing under them may show.
    for bad in ["web/dist", "web/src/api.gen.ts", "target", "docs/draft.md", "web/src/.app.ts.qur-1.tmp"] {
        assert!(!seen.iter().any(|p| p.starts_with(d.join(bad))), "{bad} leaked: {seen:?}");
    }

    // Changing a nested .gitignore takes effect for the next event.
    let gi = d.join("web/src/.gitignore");
    fs::write(&gi, "*.gen.ts\n*.snap\n").unwrap();
    let seen = collect(&mut rx, WAIT, |s| s.contains(&gi));
    assert!(seen.contains(&gi), "the .gitignore change itself is reported: {seen:?}");
    let snap = d.join("web/src/ui.snap");
    let sentinel = d.join("web/src/ui.ts");
    fs::write(&snap, "x").unwrap();
    fs::write(&sentinel, "x").unwrap();
    let seen = collect(&mut rx, WAIT, |s| s.contains(&sentinel));
    assert!(seen.contains(&sentinel) && !seen.contains(&snap), "{seen:?}");
}

#[test]
fn not_a_directory() {
    let d = tmp("notdir");
    fs::write(d.join("f"), "x").unwrap();
    assert!(Watcher::new(&d.join("f"), Duration::from_millis(50)).is_err());
    assert!(Watcher::new(&d.join("missing"), Duration::from_millis(50)).is_err());
}
