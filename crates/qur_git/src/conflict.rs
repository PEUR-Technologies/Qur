//! Conflict markers in merged text, and ours/theirs/both resolution.

/// One conflict region; line numbers are 0-based and point at the marker lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// `<<<<<<< ours`
    pub start: usize,
    /// `||||||| base` (diff3 / zdiff3 style only)
    pub base: Option<usize>,
    /// `=======`
    pub mid: usize,
    /// `>>>>>>> theirs`
    pub end: usize,
    pub ours_label: String,
    pub theirs_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resolution {
    Ours,
    Theirs,
    Both,
}

fn strip_eol(l: &str) -> &str {
    let l = l.strip_suffix('\n').unwrap_or(l);
    l.strip_suffix('\r').unwrap_or(l)
}

/// `Some(label)` when `line` is `<marker>` alone or `<marker> label`.
fn marker<'a>(line: &'a str, m: &str) -> Option<&'a str> {
    let l = strip_eol(line);
    let rest = l.strip_prefix(m)?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ').map(|s| s.trim())
    }
}

/// Every well-formed conflict in `text`, in order. Unbalanced markers are skipped.
pub fn find_conflicts(text: &str) -> Vec<Conflict> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < lines.len() {
        let Some(ours) = marker(lines[i], "<<<<<<<") else {
            i += 1;
            continue;
        };
        let start = i;
        let mut base = None;
        let mut mid = None;
        let mut j = i + 1;
        while j < lines.len() {
            let l = lines[j];
            if marker(l, "<<<<<<<").is_some() {
                // A new conflict starts before this one closed: restart there.
                i = j;
                continue 'outer;
            }
            if mid.is_none() && base.is_none() && marker(l, "|||||||").is_some() {
                base = Some(j);
            } else if mid.is_none() && strip_eol(l) == "=======" {
                mid = Some(j);
            } else if let (Some(m), Some(theirs)) = (mid, marker(l, ">>>>>>>")) {
                out.push(Conflict {
                    start,
                    base,
                    mid: m,
                    end: j,
                    ours_label: ours.to_string(),
                    theirs_label: theirs.to_string(),
                });
                i = j + 1;
                continue 'outer;
            }
            j += 1;
        }
        break;
    }
    out
}

/// `text` with conflict `i` replaced by the chosen side(s). Every other byte,
/// line endings included, is kept. Out-of-range `i` returns `text` unchanged.
pub fn resolve(text: &str, i: usize, r: Resolution) -> String {
    let Some(c) = find_conflicts(text).into_iter().nth(i) else {
        return text.to_string();
    };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let ours = &lines[c.start + 1..c.base.unwrap_or(c.mid)];
    let theirs = &lines[c.mid + 1..c.end];
    let mut out = String::with_capacity(text.len());
    lines[..c.start].iter().for_each(|l| out.push_str(l));
    match r {
        Resolution::Ours => ours.iter().for_each(|l| out.push_str(l)),
        Resolution::Theirs => theirs.iter().for_each(|l| out.push_str(l)),
        Resolution::Both => ours.iter().chain(theirs).for_each(|l| out.push_str(l)),
    }
    let tail = &lines[c.end + 1..];
    tail.iter().for_each(|l| out.push_str(l));
    out
}
