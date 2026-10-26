use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::channel::mpsc::TryRecvError;
use qur_watch::{Changes, Watcher};

const DEBOUNCE: Duration = Duration::from_millis(50);

fn tmp(name: &str) -> PathBuf {
    // std temp_dir is under /var/folders, a symlink to /private/var: this also checks that
    // batches come back under the root as given, not the canonical one.
    let d = std::env::temp_dir().join(format!("qur-watch-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Polls the channel until `done` returns true for the accumulated set or `timeout` passes.
/// Returns (batches with arrival time, union).
fn collect(
    rx: &mut Changes,
    timeout: Duration,
    mut done: impl FnMut(&BTreeSet<PathBuf>) -> bool,
) -> (Vec<(Instant, Vec<PathBuf>)>, BTreeSet<PathBuf>) {
    let end = Instant::now() + timeout;
    let mut batches = Vec::new();
    let mut all = BTreeSet::new();
    while Instant::now() < end {
        match rx.try_recv() {
            Ok(b) => {
                all.extend(b.iter().cloned());
                batches.push((Instant::now(), b));
                if done(&all) {
                    break;
                }
            }
            Err(TryRecvError::Closed) => break,
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    (batches, all)
}

fn start(root: &Path) -> (Watcher, Changes) {
    let w = Watcher::new(root, DEBOUNCE).unwrap();
    // Let the FSEvents stream settle so the temp dir's own creation doesn't leak in.
    std::thread::sleep(Duration::from_millis(100));
    let mut rx = w.1;
    while rx.try_recv().is_ok() {}
    (w.0, rx)
}

/// Performs `op`, waits for `expect` paths, returns the latency beyond the debounce.
fn step(rx: &mut Changes, label: &str, expect: &[PathBuf], op: impl FnOnce()) -> Duration {
    let t0 = Instant::now();
    op();
    let (batches, all) = collect(rx, Duration::from_millis(1000), |s| expect.iter().all(|p| s.contains(p)));
    for p in expect {
        assert!(all.contains(p), "{label}: missing {p:?} in {all:?}");
    }
    let arrived = batches.last().unwrap().0 - t0;
    assert!(arrived < DEBOUNCE + Duration::from_millis(200), "{label}: took {arrived:?}");
    let extra = arrived.saturating_sub(DEBOUNCE);
    println!("{label}: batch after {arrived:?} ({extra:?} beyond the {DEBOUNCE:?} debounce)");
    extra
}

#[test]
fn create_modify_delete_rename() {
    let d = tmp("basic");
    let (_w, mut rx) = start(&d);
    let a = d.join("a.txt");
    let b = d.join("sub/b.txt");
    let c = d.join("sub/c.txt");
    let mut extras = vec![
        step(&mut rx, "create", std::slice::from_ref(&a), || fs::write(&a, "1").unwrap()),
        step(&mut rx, "modify", std::slice::from_ref(&a), || fs::write(&a, "22").unwrap()),
        step(&mut rx, "delete", std::slice::from_ref(&a), || fs::remove_file(&a).unwrap()),
        step(&mut rx, "create nested", std::slice::from_ref(&b), || {
            fs::create_dir_all(d.join("sub")).unwrap();
            fs::write(&b, "b").unwrap();
        }),
    ];
    extras.push(step(&mut rx, "rename", &[b.clone(), c.clone()], || fs::rename(&b, &c).unwrap()));
    extras.sort();
    println!(
        "latency beyond debounce: min {:?}, median {:?}, max {:?}",
        extras[0],
        extras[extras.len() / 2],
        extras[extras.len() - 1]
    );
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn ignores_git_internals_and_gitignored() {
    let d = tmp("ignore");
    fs::create_dir_all(d.join(".git/objects/ab")).unwrap();
    fs::create_dir_all(d.join("target")).unwrap();
    fs::write(d.join(".gitignore"), "target/\n*.log\n").unwrap();
    let (_w, mut rx) = start(&d);
    // FSEvents can still report the setup writes just after the stream starts
    // (seen on CI as a stray .gitignore event); let them pass first.
    collect(&mut rx, Duration::from_millis(300), |_| false);

    fs::write(d.join("target/out.o"), "x").unwrap();
    fs::write(d.join("debug.log"), "x").unwrap();
    fs::write(d.join(".git/objects/ab/cdef"), "x").unwrap();
    fs::write(d.join(".git/config"), "x").unwrap();
    fs::write(d.join(".git/HEAD"), "ref: refs/heads/x\n").unwrap();
    fs::write(d.join(".git/index"), "x").unwrap();
    let sentinel = d.join("keep.rs");
    fs::write(&sentinel, "x").unwrap();

    let (_, mut all) = collect(&mut rx, Duration::from_millis(1000), |s| s.contains(&sentinel));
    // Anything still in flight.
    all.extend(collect(&mut rx, Duration::from_millis(150), |_| false).1);
    let want: BTreeSet<PathBuf> =
        [d.join(".git/HEAD"), d.join(".git/index"), sentinel.clone()].into_iter().collect();
    assert_eq!(all, want);

    // Un-ignoring *.log takes effect once the watcher sees the .gitignore change.
    let gi = d.join(".gitignore");
    step(&mut rx, "gitignore change", std::slice::from_ref(&gi), || fs::write(&gi, "target/\n").unwrap());
    let log = d.join("debug.log");
    step(&mut rx, "un-ignored", std::slice::from_ref(&log), || fs::write(&log, "y").unwrap());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn burst_is_debounced() {
    let d = tmp("burst");
    let (_w, mut rx) = start(&d);
    let f = d.join("hot.txt");
    for i in 0..100 {
        fs::write(&f, format!("{i}")).unwrap();
        std::thread::sleep(Duration::from_micros(500));
    }
    let (batches, _) = collect(&mut rx, Duration::from_millis(400), |_| false);
    let n = batches.iter().filter(|(_, b)| b.contains(&f)).count();
    println!("100 writes -> {n} batch(es)");
    assert!((1..=3).contains(&n), "{n} batches");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn drop_ends_the_stream() {
    let d = tmp("drop");
    let (w, mut rx) = start(&d);
    drop(w);
    fs::write(d.join("late.txt"), "x").unwrap();
    let (batches, _) = collect(&mut rx, Duration::from_millis(300), |_| false);
    assert!(batches.is_empty());
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Closed)), "channel should be closed");
    let _ = fs::remove_dir_all(&d);
}
