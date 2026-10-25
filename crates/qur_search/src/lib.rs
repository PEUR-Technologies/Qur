//! Project search (M6, Cmd+Shift+F): every text file under a root that git
//! doesn't ignore, matched with a literal or regex query, in parallel. Uses
//! only crates Qur already links (`ignore`, `regex`). Blocking: call it off the
//! UI thread.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use regex::{Regex, RegexBuilder};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl Query {
    pub fn compile(&self) -> Result<Regex, String> {
        let body = if self.regex { self.pattern.clone() } else { regex::escape(&self.pattern) };
        let body = if self.whole_word { format!(r"\b(?:{body})\b") } else { body };
        RegexBuilder::new(&body)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .size_limit(1 << 22)
            .build()
            .map_err(|e| e.to_string().lines().last().unwrap_or("invalid pattern").trim().to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LineMatch {
    /// 0-based line number.
    pub line: usize,
    /// The line's text (cut at `MAX_LINE` chars).
    pub text: String,
    /// Byte ranges of the matches within `text` (empty for a match past the cut).
    pub ranges: Vec<Range<usize>>,
    /// 0-based column (in chars) of the line's first match, also when it lies past the cut.
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileMatches {
    /// Relative to the search root.
    pub path: PathBuf,
    pub lines: Vec<LineMatch>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub files: Vec<FileMatches>,
    pub matches: usize,
    pub files_searched: usize,
    /// Stopped at `max_matches`.
    pub truncated: bool,
}

const MAX_FILE: u64 = 4 << 20;
const MAX_LINE: usize = 400;

/// Search `root`. Stops early when `cancel` is set (a newer query) or after
/// `max_matches`. Files are in path order; lines in file order.
pub fn search(root: &Path, query: &Query, max_matches: usize, cancel: &AtomicBool) -> Result<Results, String> {
    if query.pattern.is_empty() {
        return Ok(Results::default());
    }
    let re = query.compile()?;
    let found = Mutex::new(Vec::<FileMatches>::new());
    let count = AtomicUsize::new(0);
    let searched = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    let walker = ignore::WalkBuilder::new(root).hidden(false).filter_entry(|e| e.file_name() != ".git").threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8)).build_parallel();
    walker.run(|| {
        let re = re.clone();
        let (found, count, searched, truncated) = (&found, &count, &searched, &truncated);
        Box::new(move |entry| {
            if cancel.load(Ordering::Relaxed) || truncated.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            let Ok(entry) = entry else { return ignore::WalkState::Continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) || entry.metadata().map(|m| m.len() > MAX_FILE).unwrap_or(true) {
                return ignore::WalkState::Continue;
            }
            searched.fetch_add(1, Ordering::Relaxed);
            let Some(lines) = search_file(entry.path(), &re) else { return ignore::WalkState::Continue };
            let n: usize = lines.iter().map(|l| l.ranges.len()).sum();
            if count.fetch_add(n, Ordering::Relaxed) + n >= max_matches {
                truncated.store(true, Ordering::Relaxed);
            }
            let path = entry.path().strip_prefix(root).unwrap_or(entry.path()).to_path_buf();
            found.lock().unwrap_or_else(|e| e.into_inner()).push(FileMatches { path, lines });
            ignore::WalkState::Continue
        })
    });
    let mut files = found.into_inner().unwrap_or_else(|e| e.into_inner());
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Results { files, matches: count.into_inner(), files_searched: searched.into_inner(), truncated: truncated.into_inner() })
}

/// Matching lines of one file, or None (no match, binary, unreadable).
fn search_file(path: &Path, re: &Regex) -> Option<Vec<LineMatch>> {
    let bytes = std::fs::read(path).ok()?;
    // Binary: a NUL in the first 8 KB, like git and ripgrep.
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut out: Vec<LineMatch> = Vec::new();
    let mut line_starts: Option<Vec<usize>> = None;
    for m in re.find_iter(text) {
        if m.start() == m.end() {
            continue;
        }
        let starts = line_starts.get_or_insert_with(|| std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect());
        let line = starts.partition_point(|&s| s <= m.start()) - 1;
        let ls = starts[line];
        let le = starts.get(line + 1).map(|e| e - 1).unwrap_or(text.len());
        let full = text[ls..le].trim_end_matches('\r');
        let cut = floor_char(full, MAX_LINE);
        let r = (m.start() - ls).min(cut)..(m.end() - ls).min(le - ls).min(cut);
        match out.last_mut() {
            Some(l) if l.line == line => l.ranges.push(r),
            _ => {
                let col = text[ls..m.start()].chars().count();
                out.push(LineMatch { line, text: full[..cut].to_string(), ranges: vec![r], col })
            }
        }
    }
    // A match past the cut (a long minified line) still counts and still opens at its
    // column; it just has no highlight in the shortened text.
    (!out.is_empty()).then_some(out)
}

fn floor_char(s: &str, max: usize) -> usize {
    let mut i = s.len().min(max);
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Replace every match in `files` (relative to `root`; normally the files of
/// the last search). `replacement` may use `$1` / `${name}` in regex mode.
/// Returns (files changed, replacements).
/// Empty matches (`x*`, `^`) are left alone, as search never shows them. Each file is
/// rewritten atomically next to itself, keeping its permissions; a symlink keeps
/// pointing at its (rewritten) target. Files gone or no longer text are skipped.
pub fn replace_all(root: &Path, query: &Query, replacement: &str, files: &[PathBuf]) -> Result<(usize, usize), String> {
    let re = query.compile()?;
    let (mut changed, mut total) = (0, 0);
    for rel in files {
        let path = root.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let mut n = 0;
        let new = re.replace_all(&text, |caps: &regex::Captures| {
            let m = caps.get(0).expect("group 0");
            if m.start() == m.end() {
                return String::new();
            }
            n += 1;
            if query.regex {
                let mut out = String::new();
                caps.expand(replacement, &mut out);
                out
            } else {
                replacement.to_string()
            }
        });
        if n == 0 || new == text {
            continue;
        }
        let err = |e: std::io::Error| format!("{}: {e}", rel.display());
        let target = std::fs::canonicalize(&path).map_err(err)?;
        let perms = std::fs::metadata(&target).map_err(err)?.permissions();
        // `.qur-replace.tmp` is a name the file watcher knows to skip.
        let tmp = target.with_extension("qur-replace.tmp");
        std::fs::write(&tmp, new.as_bytes()).map_err(err)?;
        let _ = std::fs::set_permissions(&tmp, perms);
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err(e));
        }
        changed += 1;
        total += n;
    }
    Ok((changed, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-search-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn q(p: &str) -> Query {
        Query { pattern: p.into(), ..Default::default() }
    }

    #[test]
    fn finds_lines_respects_gitignore_and_skips_binary() {
        let d = tmp("basic");
        std::process::Command::new("git").arg("init").arg("-q").arg(&d).status().unwrap();
        std::fs::write(d.join(".gitignore"), "out/\n").unwrap();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir_all(d.join("out")).unwrap();
        std::fs::write(d.join("src/a.rs"), "fn main() {\n    let todo = 1; // TODO fix\n}\nTODO again\n").unwrap();
        std::fs::write(d.join("out/gen.rs"), "TODO in ignored\n").unwrap();
        std::fs::write(d.join("blob.bin"), b"TODO\0binary").unwrap();
        let r = search(&d, &q("todo"), 1000, &AtomicBool::new(false)).unwrap();
        assert_eq!(r.files.len(), 1, "{r:?}");
        assert_eq!(r.files[0].path, PathBuf::from("src/a.rs"));
        let lines: Vec<usize> = r.files[0].lines.iter().map(|l| l.line).collect();
        assert_eq!(lines, vec![1, 3]);
        assert_eq!(r.files[0].lines[0].ranges.len(), 2, "both hits on line 2");
        assert_eq!(r.matches, 3);
        let cs = Query { case_sensitive: true, ..q("TODO") };
        assert_eq!(search(&d, &cs, 1000, &AtomicBool::new(false)).unwrap().matches, 2);
        let ww = Query { whole_word: true, ..q("tod") };
        assert_eq!(search(&d, &ww, 1000, &AtomicBool::new(false)).unwrap().matches, 0);
        let rx = Query { regex: true, ..q(r"let \w+ =") };
        assert_eq!(search(&d, &rx, 1000, &AtomicBool::new(false)).unwrap().matches, 1);
        assert!(Query { regex: true, ..q("(") }.compile().is_err());
        let lit = q("(");
        assert!(lit.compile().is_ok(), "literal mode escapes");
    }

    #[test]
    fn truncates_and_replaces() {
        let d = tmp("replace");
        for i in 0..5 {
            std::fs::write(d.join(format!("f{i}.txt")), "foo foo\nbar\n").unwrap();
        }
        let r = search(&d, &q("foo"), 3, &AtomicBool::new(false)).unwrap();
        assert!(r.truncated);
        let all = search(&d, &q("foo"), 10_000, &AtomicBool::new(false)).unwrap();
        let files: Vec<PathBuf> = all.files.iter().map(|f| f.path.clone()).collect();
        let (changed, n) = replace_all(&d, &q("foo"), "b$z", &files).unwrap();
        assert_eq!((changed, n), (5, 10));
        assert_eq!(std::fs::read_to_string(d.join("f0.txt")).unwrap(), "b$z b$z\nbar\n", "literal mode keeps $");
        let rx = Query { regex: true, ..q(r"b\$(z)") };
        replace_all(&d, &rx, "[$1]", &files).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("f0.txt")).unwrap(), "[z] [z]\nbar\n");
    }

    fn read(d: &Path, f: &str) -> String {
        std::fs::read_to_string(d.join(f)).unwrap()
    }

    #[test]
    fn replace_skips_empty_matches_and_counts_real_ones() {
        let d = tmp("empty");
        std::fs::write(d.join("a.txt"), "xaaz\nb\n").unwrap();
        let files = vec![PathBuf::from("a.txt")];
        // `a*` also matches the empty string between every character; only "aa" counts.
        let rx = Query { regex: true, ..q("a*") };
        assert_eq!(replace_all(&d, &rx, "<$0>", &files).unwrap(), (1, 1));
        assert_eq!(read(&d, "a.txt"), "x<aa>z\nb\n");
        // Nothing but empty matches: the file is untouched.
        assert_eq!(replace_all(&d, &Query { regex: true, ..q("^") }, "#", &files).unwrap(), (0, 0));
        assert_eq!(read(&d, "a.txt"), "x<aa>z\nb\n");
        // Replacing a match with itself changes nothing and isn't counted.
        assert_eq!(replace_all(&d, &q("b"), "b", &files).unwrap(), (0, 0));
    }

    #[test]
    fn replace_options_line_endings_and_unicode() {
        let d = tmp("opts");
        std::fs::write(d.join("w.txt"), "cat concat Cat\r\nCAT\r\n").unwrap();
        std::fs::write(d.join("u.txt"), "Straße STRASSE ÉCOLE école\n").unwrap();
        let files = vec![PathBuf::from("w.txt"), PathBuf::from("u.txt")];
        // Whole word, case-insensitive (the default): "concat" stays; CRLF stays.
        let ww = Query { whole_word: true, ..q("cat") };
        assert_eq!(replace_all(&d, &ww, "dog", &files).unwrap(), (1, 3));
        assert_eq!(read(&d, "w.txt"), "dog concat dog\r\ndog\r\n");
        // Case-insensitive matching is Unicode-aware.
        assert_eq!(replace_all(&d, &q("école"), "school", &files).unwrap(), (1, 2));
        assert_eq!(read(&d, "u.txt"), "Straße STRASSE school school\n");
        // Case-sensitive leaves other cases alone.
        let cs = Query { case_sensitive: true, ..q("dog") };
        std::fs::write(d.join("w.txt"), "dog Dog\n").unwrap();
        assert_eq!(replace_all(&d, &cs, "cat", &files).unwrap(), (1, 1));
        assert_eq!(read(&d, "w.txt"), "cat Dog\n");
        // Named groups in regex mode.
        let rx = Query { regex: true, ..q(r"(?P<w>\w+) Dog") };
        replace_all(&d, &rx, "Dog ${w}", &files).unwrap();
        assert_eq!(read(&d, "w.txt"), "Dog cat\n");
        // A bad regex is an error, and nothing is written.
        assert!(replace_all(&d, &Query { regex: true, ..q("(") }, "x", &files).is_err());
    }

    #[test]
    fn replace_keeps_permissions_and_symlinks_and_skips_missing_files() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("perm");
        std::fs::write(d.join("run.sh"), "echo old\n").unwrap();
        std::fs::set_permissions(d.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(d.join("real.txt"), "old\n").unwrap();
        std::os::unix::fs::symlink("real.txt", d.join("link.txt")).unwrap();
        std::fs::write(d.join("blob.bin"), [0xff, 0xfe, b'o', b'l', b'd']).unwrap();
        let files: Vec<PathBuf> = ["run.sh", "link.txt", "gone.txt", "blob.bin"].iter().map(PathBuf::from).collect();
        assert_eq!(replace_all(&d, &q("old"), "new", &files).unwrap(), (2, 2));
        assert_eq!(read(&d, "run.sh"), "echo new\n");
        let mode = std::fs::metadata(d.join("run.sh")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "the executable bit survives");
        assert!(std::fs::symlink_metadata(d.join("link.txt")).unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(read(&d, "real.txt"), "new\n");
        assert_eq!(std::fs::read(d.join("blob.bin")).unwrap()[2..], *b"old", "not UTF-8: skipped");
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().filter_map(Result::ok).map(|e| e.file_name()).collect();
        assert!(!left.iter().any(|n| n.to_string_lossy().ends_with(".tmp")), "no temp files left: {left:?}");
    }

    #[test]
    fn search_edge_cases() {
        let d = tmp("edges");
        let long = format!("{}needle{}\n", "é".repeat(300), "x".repeat(10));
        std::fs::write(d.join("long.txt"), &long).unwrap();
        std::fs::write(d.join("crlf.txt"), "one needle\r\ntwo\r\n").unwrap();
        std::fs::write(d.join(".hidden"), "needle\n").unwrap();
        let r = search(&d, &q("needle"), 100, &AtomicBool::new(false)).unwrap();
        let paths: Vec<String> = r.files.iter().map(|f| f.path.to_string_lossy().into_owned()).collect();
        assert_eq!(paths, [".hidden", "crlf.txt", "long.txt"], "dotfiles are searched, results sorted");
        let crlf = &r.files[1].lines[0];
        assert_eq!(crlf.text, "one needle", "no trailing \\r");
        // A match past the 400-byte cut is clamped to the cut, on a char boundary.
        let l = &r.files[2].lines[0];
        assert!(l.text.len() <= 400 && l.text.is_char_boundary(l.text.len()));
        assert!(l.ranges.iter().all(|r| r.end <= l.text.len()));
        assert_eq!((l.col, r.matches), (300, 3), "past the cut: still found, at its column");
        assert_eq!(crlf.col, 4);
        // An empty pattern finds nothing; a set cancel flag stops at once.
        assert_eq!(search(&d, &q(""), 100, &AtomicBool::new(false)).unwrap(), Results::default());
        assert!(search(&d, &q("needle"), 100, &AtomicBool::new(true)).unwrap().files.is_empty());
    }

    /// M6 acceptance: search over the 5,000-file fixture in under 300 ms.
    #[test]
    #[ignore]
    fn big_repo_search_timing() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/fixtures/big-repo");
        assert!(fixture.join(".git").exists(), "run spikes/fixtures/gen.py first");
        let mut times = Vec::new();
        let mut last = Results::default();
        for _ in 0..7 {
            let t = std::time::Instant::now();
            last = search(&fixture, &q("file"), 20_000, &AtomicBool::new(false)).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        let cold = times[0];
        let mut settled = times[1..].to_vec();
        settled.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "big-repo search: {} files searched, {} matches in {} files; cold {cold:.1} ms, settled median {:.1} ms, max {:.1} ms",
            last.files_searched,
            last.matches,
            last.files.len(),
            settled[settled.len() / 2],
            settled[settled.len() - 1]
        );
        assert!(last.files_searched >= 5000, "{}", last.files_searched);
        assert!(settled[settled.len() / 2] < 300.0);
    }
}
