//! Editor model tests. Cursor notation in fixtures: `|` caret, `«` selection
//! anchor, `»` selection head (pairs in order). The primary is the last marker
//! unless the test sets it.

use std::cell::Cell;
use std::time::{Duration, Instant};

use super::*;

thread_local! {
    static CLOCK: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn fake_now() -> Instant {
    CLOCK.with(|c| *c.get().get_or_insert_with(Instant::now))
}

fn tick(ms: u64) {
    let now = fake_now();
    CLOCK.with(|c| c.set(Some(now + Duration::from_millis(ms))));
}

fn ed(src: &str) -> Editor {
    let mut text = String::new();
    let mut marks: Vec<(char, usize)> = Vec::new();
    let mut chars = 0usize;
    for c in src.chars() {
        match c {
            '|' | '«' | '»' => marks.push((c, chars)),
            _ => {
                text.push(c);
                chars += 1;
            }
        }
    }
    let mut e = Editor::from_text(&text);
    e.now = fake_now;
    let mut sels = Vec::new();
    let mut anchor = None;
    for (m, at) in marks {
        let p = e.to_point(at);
        match m {
            '|' => sels.push(Selection::caret(p)),
            '«' => anchor = Some(p),
            _ => sels.push(Selection { anchor: anchor.take().expect("« before »"), head: p, goal: None }),
        }
    }
    if !sels.is_empty() {
        let n = sels.len();
        e.set_selections(sels, n - 1);
    }
    e
}

fn show(e: &Editor) -> String {
    let mut text: Vec<String> = e.rope.to_string().chars().map(String::from).collect();
    text.push(String::new());
    let mut marks: Vec<(usize, &str)> = Vec::new();
    for s in &e.sels {
        let (a, h) = (e.to_char(s.anchor), e.to_char(s.head));
        if a == h {
            marks.push((a, "|"));
        } else {
            marks.push((a, "«"));
            marks.push((h, "»"));
        }
    }
    marks.sort_by_key(|m| std::cmp::Reverse(m.0));
    for (at, m) in marks {
        text[at].insert_str(0, m);
    }
    text.concat()
}

fn cx<'a>() -> Ctx<'a> {
    Ctx {
        page_rows: 10,
        clipboard_in: None,
        clipboard_out: None,
        line_comment: Some("//"),
        block_comment: Some(("/*", "*/")),
        indent_unit: "    ",
        matching_bracket: None,
        expand: None,
    }
}

fn run(e: &mut Editor, cmd: Command) -> Outcome {
    e.run(cmd, &mut cx())
}

fn check(src: &str, cmd: Command, want: &str) {
    let mut e = ed(src);
    run(&mut e, cmd.clone());
    assert_eq!(show(&e), want, "{cmd:?} on {src:?}");
}

fn ins(s: &str) -> Command {
    Command::Insert(s.into())
}

// --- Typing -------------------------------------------------------------------

#[test]
fn insert_multi_cursor_and_replace_selection() {
    check("a|b|c", ins("x"), "ax|bx|c");
    check("«abc» d", ins("x"), "x| d");
    check("a|b", ins("1\r\n2"), "a1\n2|b");
}

#[test]
fn overlapping_selections_merge() {
    let mut e = ed("«abc»");
    e.set_selections(
        vec![
            Selection { anchor: Point { line: 0, col: 0 }, head: Point { line: 0, col: 2 }, goal: None },
            Selection { anchor: Point { line: 0, col: 1 }, head: Point { line: 0, col: 3 }, goal: None },
            Selection::caret(Point { line: 0, col: 3 }),
        ],
        1,
    );
    assert_eq!(show(&e), "«abc»");
    assert_eq!(e.selections().len(), 1);
}

#[test]
fn backspace_variants() {
    check("ab|c", Command::Backspace, "a|c");
    check("ab\n|c", Command::Backspace, "ab|c");
    check("|abc", Command::Backspace, "|abc");
    // Soft tab: deletes back to the previous indent stop.
    check("        |x", Command::Backspace, "    |x");
    check("      |x", Command::Backspace, "    |x");
    // Emoji with skin tone and a combining mark are one grapheme.
    check("a👍🏽|", Command::Backspace, "a|");
    check("e\u{301}|x", Command::Backspace, "|x");
    check("a|b|c", Command::Backspace, "|c");
}

#[test]
fn delete_forward_and_word_deletes() {
    check("ab|\ncd", Command::Delete, "ab|cd");
    check("|👍🏽x", Command::Delete, "|x");
    check("foo bar|", Command::DeleteWordLeft, "foo |");
    check("foo| bar", Command::DeleteWordRight, "foo|");
    check("    abc|def", Command::DeleteToLineStart, "|def");
    check("ab\n|cd", Command::DeleteToLineStart, "ab|cd");
}

#[test]
fn newline_keeps_indent() {
    check("    foo|", Command::Newline, "    foo\n    |");
    check("  |foo", Command::Newline, "  \n  |foo");
}

#[test]
fn delete_line() {
    check("a\nb|c\nd", Command::DeleteLine, "a\nd|");
    check("a\nbc|", Command::DeleteLine, "a|");
    check("o|nly", Command::DeleteLine, "|");
    // Two cursors on one line delete it once.
    check("a\nb|c|d\ne", Command::DeleteLine, "a\ne|");
}

// --- Movement -------------------------------------------------------------------

#[test]
fn word_movement() {
    check("foo.bar  baz|", Command::WordLeft { select: false }, "foo.bar  |baz");
    check("foo.bar  |baz", Command::WordLeft { select: false }, "foo.|bar  baz");
    check("foo.|bar", Command::WordLeft { select: false }, "foo|.bar");
    check("|foo.bar", Command::WordRight { select: false }, "foo|.bar");
    check("foo|  bar", Command::WordRight { select: false }, "foo  bar|");
    check("ab|\ncd", Command::WordRight { select: false }, "ab\n|cd");
    check("foo |bar", Command::WordRight { select: true }, "foo «bar»");
}

#[test]
fn smart_home_and_end() {
    check("    ab|c", Command::LineStart { select: false }, "    |abc");
    check("    |abc", Command::LineStart { select: false }, "|    abc");
    check("a|bc\nd", Command::LineEnd { select: true }, "a«bc»\nd");
}

#[test]
fn left_right_collapse_selection() {
    check("a«bc»d", Command::Left { select: false }, "a|bcd");
    check("a«bc»d", Command::Right { select: false }, "abc|d");
    check("a|b", Command::Right { select: true }, "a«b»");
}

#[test]
fn vertical_keeps_goal_column() {
    let mut e = ed("abcdef|\nab\nabcdef");
    run(&mut e, Command::Down { select: false });
    assert_eq!(show(&e), "abcdef\nab|\nabcdef");
    run(&mut e, Command::Down { select: false });
    assert_eq!(show(&e), "abcdef\nab\nabcdef|");
    run(&mut e, Command::Down { select: false });
    assert_eq!(show(&e), "abcdef\nab\nabcdef|");
    run(&mut e, Command::Up { select: false });
    run(&mut e, Command::Up { select: false });
    run(&mut e, Command::Up { select: false });
    assert_eq!(show(&e), "|abcdef\nab\nabcdef");
}

#[test]
fn vertical_moves_by_visual_rows_when_wrapped() {
    let mut e = ed("hello brave new world|\nx");
    e.set_wrap_width(Some(10));
    run(&mut e, Command::Up { select: false });
    // From "world|" (row 2 col 5) to row 1 col 5: "brave| new ".
    assert_eq!(show(&e), "hello brave| new world\nx");
    run(&mut e, Command::PageDown { select: false });
    assert_eq!(show(&e), "hello brave new world\nx|");
}

#[test]
fn doc_start_end_and_go_to_line() {
    check("a\nb|\nc", Command::DocStart { select: false }, "|a\nb\nc");
    check("a\nb|\nc", Command::DocEnd { select: true }, "a\nb«\nc»");
    check("a|\nb\nc", Command::GoToLine(3), "a\nb\n|c");
    check("a|\nb\nc", Command::GoToLine(99), "a\nb\n|c");
    check("a\nb|\nc", Command::GoToLine(0), "|a\nb\nc");
}

// --- Line operations ------------------------------------------------------------

#[test]
fn tab_indent_outdent() {
    check("ab|", Command::Tab, "ab  |");
    check("|ab", Command::Tab, "    |ab");
    check("«a\nb»\n\nc", Command::Tab, "«    a\n    b»\n\nc");
    check("    a|\n  b", Command::Outdent, "a|\n  b");
    check("«    a\n  b»", Command::Backtab, "«a\nb»");
    check("\ta|", Command::Outdent, "a|");
    check("a|", Command::Indent, "    a|");
}

#[test]
fn line_comment_toggle() {
    check("  a|\n    b", Command::ToggleLineComment, "  // a|\n    b");
    check("«  a\n    b»", Command::ToggleLineComment, "«  // a\n  //   b»");
    check("«  // a\n  //   b»", Command::ToggleLineComment, "«  a\n    b»");
    check("// x|", Command::ToggleLineComment, "x|");
    // Blank lines are skipped when there are code lines.
    check("«a\n\nb»", Command::ToggleLineComment, "«// a\n\n// b»");
}

#[test]
fn line_comment_falls_back_to_block() {
    let mut e = ed("  <p>x</p>|");
    let mut c = cx();
    c.line_comment = None;
    c.block_comment = Some(("<!--", "-->"));
    e.run(Command::ToggleLineComment, &mut c);
    assert_eq!(e.rope.to_string(), "  <!-- <p>x</p> -->");
}

#[test]
fn block_comment_toggle() {
    check("a «b» c", Command::ToggleBlockComment, "a «/* b */» c");
    check("a «/* b */» c", Command::ToggleBlockComment, "a «b» c");
    check("x|", Command::ToggleBlockComment, "x/* | */");
}

#[test]
fn move_and_copy_lines() {
    check("a\nb|\nc", Command::MoveLineUp, "b|\na\nc");
    check("a\nb|\nc", Command::MoveLineDown, "a\nc\nb|");
    check("a|\nb", Command::MoveLineUp, "a|\nb");
    check("a\n«b\nc»\nd", Command::MoveLineUp, "«b\nc»\na\nd");
    check("a|\nb", Command::CopyLineDown, "a\na|\nb");
    check("a|\nb", Command::CopyLineUp, "a|\na\nb");
    check("a\nb|", Command::CopyLineDown, "a\nb\nb|");
    // Two separate blocks both move.
    check("a\nb|\nc\nd|", Command::MoveLineUp, "b|\na\nd|\nc");
}

#[test]
fn insert_line_below_above() {
    check("  a|b\nc", Command::InsertLineBelow, "  ab\n  |\nc");
    check("  a|b", Command::InsertLineAbove, "  |\n  ab");
}

#[test]
fn trim_trailing_whitespace() {
    check("a  \nb\t|\nc", Command::TrimTrailingWhitespace, "a\nb|\nc");
}

// --- Selection ------------------------------------------------------------------

#[test]
fn select_line_extends() {
    let mut e = ed("a|b\ncd\nef");
    run(&mut e, Command::SelectLine);
    assert_eq!(show(&e), "«ab\n»cd\nef");
    run(&mut e, Command::SelectLine);
    assert_eq!(show(&e), "«ab\ncd\n»ef");
    check("a\nb|", Command::SelectLine, "a\n«b»");
}

#[test]
fn select_all_and_single_cursor() {
    check("a|b\nc", Command::SelectAll, "«ab\nc»");
    check("a|b|c", Command::SingleCursor, "ab|c");
    check("a«b»c", Command::SingleCursor, "ab|c");
}

#[test]
fn add_next_match() {
    let mut e = ed("fo|o bar foo foo");
    run(&mut e, Command::AddNextMatch);
    assert_eq!(show(&e), "«foo» bar foo foo");
    run(&mut e, Command::AddNextMatch);
    assert_eq!(show(&e), "«foo» bar «foo» foo");
    run(&mut e, Command::AddNextMatch);
    run(&mut e, Command::AddNextMatch); // wraps; nothing left
    assert_eq!(show(&e), "«foo» bar «foo» «foo»");
    run(&mut e, ins("x"));
    assert_eq!(show(&e), "x| bar x| x|");
}

#[test]
fn select_all_matches() {
    let mut e = ed("ab| ab xab");
    run(&mut e, Command::SelectAllMatches);
    assert_eq!(show(&e), "«ab» «ab» x«ab»");
    assert_eq!(e.primary().start(), Point { line: 0, col: 0 });
}

#[test]
fn add_cursor_above_below() {
    let mut e = ed("abc\nab|c\nabc");
    run(&mut e, Command::AddCursorAbove);
    assert_eq!(show(&e), "ab|c\nab|c\nabc");
    run(&mut e, Command::AddCursorBelow);
    assert_eq!(show(&e), "ab|c\nab|c\nab|c");
    run(&mut e, Command::AddCursorBelow); // already at the last row
    assert_eq!(e.selections().len(), 3);
}

#[test]
fn click_drag_word_line() {
    let mut e = ed("hello world|");
    e.click(Point { line: 0, col: 2 }, false, false);
    assert_eq!(show(&e), "he|llo world");
    e.click(Point { line: 0, col: 8 }, true, false);
    assert_eq!(show(&e), "he«llo wo»rld");
    e.click(Point { line: 0, col: 0 }, false, true);
    assert_eq!(e.selections().len(), 2);
    e.click(Point { line: 0, col: 0 }, false, true); // toggles off
    assert_eq!(e.selections().len(), 1);
    e.select_word_at(Point { line: 0, col: 7 });
    assert_eq!(show(&e), "hello «world»");
    e.select_line_at(Point { line: 0, col: 1 });
    assert_eq!(show(&e), "«hello world»");
    e.click(Point { line: 0, col: 3 }, false, false);
    e.drag_to(Point { line: 0, col: 1 });
    assert_eq!(show(&e), "h»el«lo world");
}

#[test]
fn matching_bracket() {
    check("f|(a, (b)) x", Command::JumpToMatchingBracket, "f(a, (b)|) x");
    check("f(a, (b))| x", Command::JumpToMatchingBracket, "f|(a, (b)) x");
    // Syntax-aware lookup wins when provided.
    let mut e = ed("|{ }");
    let f = |b: usize| (b == 0).then_some(2..3);
    let mut c = cx();
    c.matching_bracket = Some(&f);
    e.run(Command::JumpToMatchingBracket, &mut c);
    assert_eq!(show(&e), "{ |}");
}

// --- Clipboard ------------------------------------------------------------------

#[test]
fn copy_paste_whole_line() {
    let mut e = ed("ab|c\ndef");
    let mut c = cx();
    e.run(Command::Copy, &mut c);
    assert_eq!(c.clipboard_out, Some(("abc\n".to_string(), true)));
    e.click(Point { line: 1, col: 1 }, false, false);
    let mut c = cx();
    c.clipboard_in = Some("abc\n");
    e.run(Command::Paste, &mut c);
    assert_eq!(show(&e), "abc\nabc\nd|ef");
}

#[test]
fn cut_selection_and_whole_line() {
    let mut e = ed("a«bc»d");
    let mut c = cx();
    e.run(Command::Cut, &mut c);
    assert_eq!(c.clipboard_out, Some(("bc".to_string(), false)));
    assert_eq!(show(&e), "a|d");

    let mut e = ed("x\nab|c\ny");
    let mut c = cx();
    e.run(Command::Cut, &mut c);
    assert_eq!(c.clipboard_out, Some(("abc\n".to_string(), true)));
    assert_eq!(show(&e), "x\ny|");
}

#[test]
fn multi_cursor_paste_distributes_lines() {
    let mut e = ed("a|\nb|\nc|");
    let mut c = cx();
    c.clipboard_in = Some("1\n2\n3");
    e.run(Command::Paste, &mut c);
    assert_eq!(show(&e), "a1|\nb2|\nc3|");
    // Mismatched count pastes everything at each cursor.
    let mut e = ed("a|b|");
    let mut c = cx();
    c.clipboard_in = Some("1\n2\n3");
    e.run(Command::Paste, &mut c);
    assert_eq!(e.rope.to_string(), "a1\n2\n3b1\n2\n3");
}

// --- Undo -----------------------------------------------------------------------

#[test]
fn undo_groups_typing_within_a_second() {
    let mut e = ed("|");
    for ch in ["a", "b", "c"] {
        run(&mut e, ins(ch));
        tick(100);
    }
    run(&mut e, Command::Undo);
    assert_eq!(show(&e), "|");
    run(&mut e, Command::Redo);
    assert_eq!(show(&e), "abc|");

    let mut e = ed("|");
    run(&mut e, ins("a"));
    tick(1500);
    run(&mut e, ins("b"));
    run(&mut e, Command::Undo);
    assert_eq!(show(&e), "a|");

    // Movement breaks the group.
    let mut e = ed("|");
    run(&mut e, ins("a"));
    run(&mut e, Command::Left { select: false });
    run(&mut e, Command::Right { select: false });
    run(&mut e, ins("b"));
    run(&mut e, Command::Undo);
    assert_eq!(show(&e), "a|");

    // Backspaces group, but not with typing.
    let mut e = ed("abc|");
    run(&mut e, Command::Backspace);
    run(&mut e, Command::Backspace);
    run(&mut e, ins("x"));
    run(&mut e, Command::Undo);
    assert_eq!(show(&e), "a|");
    run(&mut e, Command::Undo);
    assert_eq!(show(&e), "abc|");
}

#[test]
fn undo_restores_multi_cursor_selections() {
    let mut e = ed("a«b»c|d");
    run(&mut e, ins("X"));
    assert_eq!(show(&e), "aX|cX|d");
    run(&mut e, Command::Undo);
    assert_eq!(show(&e), "a«b»c|d");
    run(&mut e, Command::Redo);
    assert_eq!(show(&e), "aX|cX|d");
}

#[test]
fn dirty_tracks_save_point() {
    let dir = scratch_dir("dirty");
    let path = dir.join("f.txt");
    std::fs::write(&path, "x").unwrap();
    let mut e = Editor::open(&path, "    ").unwrap();
    e.now = fake_now;
    assert!(!e.is_dirty());
    run(&mut e, ins("a"));
    assert!(e.is_dirty());
    e.save().unwrap();
    assert!(!e.is_dirty());
    run(&mut e, ins("b")); // would group with "a" if save didn't break it
    assert!(e.is_dirty());
    run(&mut e, Command::Undo);
    assert!(!e.is_dirty());
    run(&mut e, Command::Undo);
    assert!(e.is_dirty());
    run(&mut e, Command::Redo);
    assert!(!e.is_dirty());
    run(&mut e, Command::Redo);
    assert!(e.is_dirty());
}

// --- Files ----------------------------------------------------------------------

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("qur_editor_test_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn crlf_bom_and_permissions_round_trip() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir("crlf");
    let path = dir.join("w.txt");
    std::fs::write(&path, "\u{feff}one\r\ntwo\r\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).unwrap();
    let mut e = Editor::open(&path, "    ").unwrap();
    assert!(e.is_crlf());
    assert_eq!(e.rope.to_string(), "one\ntwo\n");
    e.click(Point { line: 1, col: 3 }, false, false);
    run(&mut e, ins("!"));
    e.save().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), "\u{feff}one\r\ntwo!\r\n".as_bytes());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o751);
    // No temp files left behind.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn every_encoding_saves_back_byte_for_byte() {
    use encoding::{CHOICES, encode, label};
    let dir = scratch_dir("enc");
    for (i, (enc, bom)) in CHOICES.into_iter().enumerate() {
        let path = dir.join(format!("f{i}.txt"));
        let bytes = encode("Grüße “x” €\r\nzwei\r\n", enc, bom).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let mut e = Editor::open(&path, "    ").unwrap();
        assert_eq!(e.encoding(), (enc, bom), "{}", label(enc, bom));
        assert!(e.is_crlf() && !e.is_lossy());
        assert_eq!(e.rope.to_string(), "Grüße “x” €\nzwei\n");
        assert_eq!(e.read_disk_text().unwrap(), "Grüße “x” €\nzwei\n");
        e.save().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{}", label(enc, bom));
    }
}

#[test]
fn invalid_utf8_opens_as_windows_1252() {
    let dir = scratch_dir("latin");
    let path = dir.join("l.txt");
    std::fs::write(&path, b"caf\xE9\n\x93q\x94\n").unwrap();
    let mut e = Editor::open(&path, "    ").unwrap();
    assert_eq!(e.encoding(), (encoding::Encoding::Windows1252, false));
    assert_eq!(e.rope.to_string(), "café\n“q”\n");
    assert!(!e.is_lossy());
    e.click(Point { line: 1, col: 3 }, false, false);
    run(&mut e, ins("!"));
    e.save().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"caf\xE9\n\x93q\x94!\n");
    // A char 1252 can't store: the save fails and the file is untouched.
    run(&mut e, ins("✓"));
    let err = e.save().unwrap_err();
    assert!(err.to_string().contains("Windows-1252"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), b"caf\xE9\n\x93q\x94!\n");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn binary_and_undecodable_files_are_lossy() {
    let dir = scratch_dir("bin");
    // NULs and a lone CR: opens (as Windows-1252) but can't save back exactly.
    let path = dir.join("b.bin");
    std::fs::write(&path, b"\x7FELF\0\x01\r\x02\xFF\n").unwrap();
    let e = Editor::open(&path, "    ").unwrap();
    assert_eq!(e.encoding().0, encoding::Encoding::Windows1252);
    assert!(e.is_lossy());
    // Binary without stray CRs round-trips through Windows-1252.
    std::fs::write(&path, b"\0\x01\x02\xFF\n").unwrap();
    assert!(!Editor::open(&path, "    ").unwrap().is_lossy());
    // UTF-8 forced on bytes that aren't: replacement chars, lossy.
    let path = dir.join("t.txt");
    std::fs::write(&path, b"a\xFFb").unwrap();
    let e = Editor::open_with(&path, "    ", Some((encoding::Encoding::Utf8, false))).unwrap();
    assert_eq!(e.rope.to_string(), "a\u{FFFD}b");
    assert!(e.is_lossy());
    assert_eq!(e.forced_encoding(), Some((encoding::Encoding::Utf8, false)));
    // Mixed line endings in a text file aren't lossy (evened out on save, as before).
    std::fs::write(&path, b"a\r\nb\nc\r\n").unwrap();
    assert!(!Editor::open(&path, "    ").unwrap().is_lossy());
}

#[test]
fn line_ending_conversion_is_undoable() {
    let dir = scratch_dir("eol");
    let path = dir.join("e.txt");
    std::fs::write(&path, "a\nb\n").unwrap();
    let mut e = Editor::open(&path, "    ").unwrap();
    e.now = fake_now;
    assert!(!e.is_crlf() && !e.is_dirty());
    assert!(e.set_line_endings(true));
    assert!(!e.set_line_endings(true));
    assert!(e.is_crlf() && e.is_dirty());
    run(&mut e, Command::Undo);
    assert!(!e.is_crlf() && !e.is_dirty());
    run(&mut e, Command::Redo);
    assert!(e.is_crlf() && e.is_dirty());
    e.save().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"a\r\nb\r\n");
    assert!(!e.is_dirty());
    assert!(e.set_line_endings(false));
    e.save().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"a\nb\n");
}

#[test]
fn encoding_conversion_marks_dirty_and_undoes() {
    use encoding::Encoding;
    let dir = scratch_dir("conv");
    let path = dir.join("c.txt");
    std::fs::write(&path, "héllo\n").unwrap();
    let mut e = Editor::open(&path, "    ").unwrap();
    e.now = fake_now;
    assert_eq!(e.set_encoding(Encoding::Utf16Le, true), Ok(true));
    assert!(e.is_dirty());
    e.save().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), encoding::encode("héllo\n", Encoding::Utf16Le, true).unwrap());
    assert_eq!(e.set_encoding(Encoding::Utf16Le, true), Ok(false));
    run(&mut e, ins("✓"));
    assert_eq!(e.set_encoding(Encoding::Windows1252, false), Err('✓'));
    assert_eq!(e.encoding(), (Encoding::Utf16Le, true));
    run(&mut e, Command::Undo);
    assert_eq!(e.set_encoding(Encoding::Windows1252, true), Ok(true));
    // No BOM for Windows-1252.
    assert_eq!(e.encoding(), (Encoding::Windows1252, false));
    run(&mut e, Command::Undo);
    assert_eq!(e.encoding(), (Encoding::Utf16Le, true));
    assert!(!e.is_dirty());
}

#[test]
fn indent_detection() {
    assert_eq!(Editor::from_text("a\n  b\n    c\n  d\n").indent_unit(), "  ");
    assert_eq!(Editor::from_text("a\n\tb\n\t\tc\n").indent_unit(), "\t");
    assert_eq!(Editor::from_text("a\n    b\n        c\n").indent_unit(), "    ");
    let dir = scratch_dir("indent");
    let path = dir.join("f.ts");
    std::fs::write(&path, "flat\nfile\n").unwrap();
    assert_eq!(Editor::open(&path, "  ").unwrap().indent_unit(), "  ");
}

#[test]
fn ropey_counts_only_lf_line_breaks() {
    // Guards against feature unification turning on ropey's `unicode_lines`
    // (tree-sitter rows would no longer match buffer lines).
    assert_eq!(Rope::from_str("a\u{2028}b\u{85}c\rd").len_lines(), 1);
}

// --- Find -----------------------------------------------------------------------

#[test]
fn find_plain_word_regex() {
    let mut e = ed("|Foo foo food\nfoo");
    assert_eq!(e.set_find("foo", FindOptions::default()), Ok(4));
    let opts = FindOptions { case_sensitive: true, ..Default::default() };
    assert_eq!(e.set_find("foo", opts), Ok(3));
    let opts = FindOptions { whole_word: true, ..Default::default() };
    assert_eq!(e.set_find("foo", opts), Ok(3));
    let opts = FindOptions { regex: true, ..Default::default() };
    assert_eq!(e.set_find("fo+d?", opts), Ok(4));
    assert!(e.set_find("(", opts).is_err());
    assert_eq!(e.find_matches_in_lines(0..9).len(), 0);
    assert_eq!(e.set_find("", FindOptions::default()), Ok(0));
}

#[test]
fn find_next_prev_wraps_and_status() {
    let mut e = ed("a x\nb x|\nc x");
    e.set_find("x", FindOptions::default()).unwrap();
    assert_eq!(e.find_status(), (0, 3));
    e.find_next();
    assert_eq!(show(&e), "a x\nb x\nc «x»");
    assert_eq!(e.find_status(), (3, 3));
    e.find_next();
    assert_eq!(show(&e), "a «x»\nb x\nc x");
    e.find_prev();
    assert_eq!(show(&e), "a x\nb x\nc «x»");
    e.find_prev();
    assert_eq!(show(&e), "a x\nb «x»\nc x");
}

#[test]
fn replace_one_and_all_with_groups() {
    let mut e = ed("|let a = 1; let b = 2;");
    let opts = FindOptions { regex: true, ..Default::default() };
    e.set_find(r"let (\w)", opts).unwrap();
    e.find_next();
    e.replace_one("const $1");
    assert_eq!(e.rope.to_string(), "const a = 1; let b = 2;");
    assert_eq!(show(&e), "const a = 1; «let b» = 2;");
    let n = e.replace_all("var $1");
    assert_eq!(n, 1);
    assert_eq!(e.rope.to_string(), "const a = 1; var b = 2;");

    let mut e = ed("x x x|");
    e.set_find("x", FindOptions::default()).unwrap();
    assert_eq!(e.replace_all("yy"), 3);
    assert_eq!(e.rope.to_string(), "yy yy yy");
    run(&mut e, Command::Undo);
    assert_eq!(e.rope.to_string(), "x x x");
}

#[test]
fn find_cache_stays_correct_after_edits() {
    let mut e = ed("ab|\nab\nab");
    e.set_find("ab", FindOptions::default()).unwrap();
    assert_eq!(e.find_status().1, 3);
    run(&mut e, ins("ab"));
    run(&mut e, Command::Newline);
    run(&mut e, ins("zab"));
    run(&mut e, Command::DeleteLine);
    let cached = e.find_matches_in_lines(0..100);
    let total = e.find_status().1;
    let mut fresh = Editor::from_text(&e.rope.to_string());
    fresh.set_find("ab", FindOptions::default()).unwrap();
    assert_eq!(cached, fresh.find_matches_in_lines(0..100));
    assert_eq!(total, fresh.find_status().1);
}

// --- Edits for tree-sitter --------------------------------------------------------

/// Apply byte edits to a plain String the way tree-sitter would see them, and
/// check positions and points against that reference after every command.
#[test]
fn edits_reconstruct_text_exactly() {
    let mut e = ed("fn main() {\n    let x = 1;|\n}\n日本 👍🏽 ok");
    let mut reference = e.rope.to_string();
    let mut c = cx();
    let script: Vec<Command> = vec![
        Command::Newline,
        ins("let y = \"é\";"),
        Command::AddCursorAbove,
        ins("Ω"),
        Command::SingleCursor,
        Command::SelectLine,
        Command::CopyLineDown,
        Command::MoveLineUp,
        Command::ToggleLineComment,
        Command::Undo,
        Command::Redo,
        Command::DocEnd { select: false },
        Command::Backspace,
        Command::Backspace,
        Command::SelectAll,
        Command::Indent,
        Command::Undo,
        Command::Undo,
        Command::TrimTrailingWhitespace,
        Command::DeleteLine,
        Command::Undo,
    ];
    e.edit_texts.clear();
    for cmd in script {
        e.run(cmd.clone(), &mut c);
        let texts = std::mem::take(&mut e.edit_texts);
        let edits = e.take_edits();
        assert_eq!(texts.len(), edits.len());
        for (ed, text) in edits.iter().zip(texts) {
            assert_eq!(ed.start_point, point_of(&reference, ed.start_byte), "{cmd:?} start");
            assert_eq!(ed.old_end_point, point_of(&reference, ed.old_end_byte), "{cmd:?} old end");
            assert_eq!(ed.new_end_byte - ed.start_byte, text.len(), "{cmd:?} new end byte");
            reference.replace_range(ed.start_byte..ed.old_end_byte, &text);
            assert_eq!(ed.new_end_point, point_of(&reference, ed.new_end_byte), "{cmd:?} new end");
        }
        assert_eq!(reference, e.rope.to_string(), "{cmd:?}: replayed edits diverge");
    }
}

fn point_of(s: &str, byte: usize) -> (usize, usize) {
    let before = &s[..byte];
    let row = before.matches('\n').count();
    let col = byte - before.rfind('\n').map_or(0, |i| i + 1);
    (row, col)
}

/// Stronger check: replay each command's edits with the real inserted text
/// (taken from the rope via `new_end_byte`) and compare full text.
#[test]
fn edit_points_match_inserted_text() {
    let mut e = ed("a|b\ncd");
    let mut c = cx();
    let before = e.rope.to_string();
    e.run(ins("x\ny"), &mut c);
    let edits = e.take_edits();
    assert_eq!(edits.len(), 1);
    let ed0 = edits[0];
    assert_eq!(ed0.start_point, (0, 1));
    assert_eq!(ed0.old_end_point, (0, 1));
    assert_eq!(ed0.new_end_point, (1, 1));
    let after = e.rope.to_string();
    let mut rebuilt = before[..ed0.start_byte].to_string();
    rebuilt.push_str(&after[ed0.start_byte..ed0.new_end_byte]);
    rebuilt.push_str(&before[ed0.old_end_byte..]);
    assert_eq!(rebuilt, after);
    // Multi-cursor: applied back to front, so each edit is valid at its time.
    let mut e = ed("|a|b|");
    e.run(ins("é"), &mut c);
    let starts: Vec<usize> = e.take_edits().iter().map(|x| x.start_byte).collect();
    assert_eq!(starts, vec![2, 1, 0]);
}

// --- Wrap -----------------------------------------------------------------------

#[test]
fn wrap_updates_with_edits_and_width() {
    let mut e = ed("|short");
    e.set_wrap_width(Some(10));
    assert_eq!(e.wrap().row_count(), 1);
    run(&mut e, ins("a long line that wraps "));
    let fresh = WrapMap::new(e.rope(), Some(10), 4);
    assert_eq!(e.wrap().row_count(), fresh.row_count());
    assert_eq!(e.wrap().rows(0..99), fresh.rows(0..99));
    e.set_wrap_width(None);
    assert_eq!(e.wrap().row_count(), 1);
    let rows = WrapMap::new(&Rope::from_str("日本語漢字 👍🏽👍🏽"), Some(6), 4).rows(0..9);
    assert!(rows.len() >= 2);
    assert!(rows.iter().skip(1).all(|r| !r.first));
}

// --- Performance (release only: cargo test -p qur_editor --release -- --nocapture) ---

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/fixtures/large.ts")
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
#[cfg_attr(debug_assertions, ignore = "perf: run with --release")]
fn perf_large_file() {
    let path = fixture();
    if !path.exists() {
        eprintln!("skip: run spikes/fixtures/gen.py");
        return;
    }
    // Warm the page cache.
    let _ = std::fs::read(&path);

    let t = Instant::now();
    let mut e = Editor::open(&path, "  ").unwrap();
    e.set_wrap_width(Some(100));
    let open = t.elapsed();

    e.click(Point { line: 5000, col: 10 }, false, false);
    let mut c = cx();
    let mut samples = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        e.run(ins("x"), &mut c);
        let _ = e.take_edits();
        samples.push(t.elapsed());
    }
    samples.sort();
    let insert_p50 = samples[100];
    let insert_max = samples[199];

    let t = Instant::now();
    let n = e.set_find("cache", FindOptions::default()).unwrap();
    let find = t.elapsed();

    // Typing with the find bar open: only the edited line is recomputed.
    let t = Instant::now();
    e.run(ins("y"), &mut c);
    let _ = e.find_status();
    let type_with_find = t.elapsed();

    let t = Instant::now();
    e.set_wrap_width(Some(80));
    let rewrap = t.elapsed();

    eprintln!(
        "open+wrap {:.2} ms | insert p50 {:.4} ms max {:.4} ms | find {:.2} ms ({n} matches) | type+find {:.3} ms | set_wrap_width {:.2} ms",
        ms(open),
        ms(insert_p50),
        ms(insert_max),
        ms(find),
        ms(type_with_find),
        ms(rewrap)
    );
    // Shared CI runners are slower and noisy (15.7 ms open+wrap seen), so CI only prints the numbers.
    if std::env::var_os("CI").is_none() {
        assert!(ms(open) < 15.0, "open+wrap {:.2} ms", ms(open));
        assert!(ms(insert_p50) < 0.2, "insert {:.4} ms", ms(insert_p50));
        assert!(ms(find) < 5.0, "find {:.2} ms", ms(find));
        assert!(ms(type_with_find) < 1.0, "type with find {:.3} ms", ms(type_with_find));
        assert!(ms(rewrap) < 10.0, "set_wrap_width {:.2} ms", ms(rewrap));
    }
}

// --- Soft-wrap Home/End, Expand/Shrink Selection ---------------------------

#[test]
fn home_end_by_visual_row_when_wrapped() {
    // Width 10: rows "hello ", "brave new ", "world".
    let mut e = ed("hello bra|ve new world\nx");
    e.set_wrap_width(Some(10));
    run(&mut e, Command::LineEnd { select: false });
    assert_eq!(show(&e), "hello brave new| world\nx", "row end, before the break");
    run(&mut e, Command::LineEnd { select: false });
    assert_eq!(show(&e), "hello brave new world|\nx", "again: the line end");
    run(&mut e, Command::LineStart { select: false });
    assert_eq!(show(&e), "hello brave new |world\nx", "row start");
    run(&mut e, Command::LineStart { select: false });
    assert_eq!(show(&e), "|hello brave new world\nx", "again: smart home");

    let mut e = ed("hello bra|ve new world");
    e.set_wrap_width(Some(10));
    run(&mut e, Command::LineEnd { select: true });
    assert_eq!(show(&e), "hello bra«ve new» world");
    // First row: plain smart home.
    let mut e = ed("  hel|lo brave new world");
    e.set_wrap_width(Some(10));
    run(&mut e, Command::LineStart { select: false });
    assert_eq!(show(&e), "  |hello brave new world");
    // No wrap: the line ends, as before.
    check("hello bra|ve new world", Command::LineEnd { select: false }, "hello brave new world|");
}

/// Expand through fixed "syntax nodes" (byte ranges), smallest first.
fn run_expand(e: &mut Editor, nodes: &[Range<usize>], cmd: Command) -> Outcome {
    let f = |r: Range<usize>| {
        nodes.iter().filter(|n| n.start <= r.start && n.end >= r.end && **n != r).min_by_key(|n| n.len()).cloned()
    };
    let mut c = cx();
    c.expand = Some(&f);
    e.run(cmd, &mut c)
}

#[test]
fn expand_and_shrink_through_syntax_nodes() {
    // let x = foo(bar);
    // 0   4   8  11 15
    let nodes = [8..11, 12..15, 11..16, 8..16, 0..17];
    let mut e = ed("let x = foo(b|ar);");
    let steps = ["let x = foo(«bar»);", "let x = foo«(bar)»;", "let x = «foo(bar)»;", "«let x = foo(bar);»"];
    for want in steps {
        run_expand(&mut e, &nodes, Command::ExpandSelection);
        assert_eq!(show(&e), want);
    }
    // Nothing bigger: no change, nothing pushed.
    let out = run_expand(&mut e, &nodes, Command::ExpandSelection);
    assert!(!out.selections_changed);
    for want in steps.iter().rev().skip(1).chain(["let x = foo(b|ar);"].iter()) {
        run_expand(&mut e, &nodes, Command::ShrinkSelection);
        assert_eq!(show(&e), *want);
    }
    // An empty stack shrinks nothing.
    let out = run_expand(&mut e, &nodes, Command::ShrinkSelection);
    assert!(!out.selections_changed);
    assert_eq!(show(&e), "let x = foo(b|ar);");
}

#[test]
fn expand_multi_cursor_falls_back_to_word_line_file() {
    let mut e = ed("a|b cd\nef g|h");
    run(&mut e, Command::ExpandSelection);
    assert_eq!(show(&e), "«ab» cd\nef «gh»");
    run(&mut e, Command::ExpandSelection);
    assert_eq!(show(&e), "«ab cd»\n«ef gh»");
    run(&mut e, Command::ExpandSelection);
    assert_eq!(show(&e), "«ab cd\nef gh»", "merged into one");
    run(&mut e, Command::ShrinkSelection);
    assert_eq!(show(&e), "«ab cd»\n«ef gh»", "split again");
    run(&mut e, Command::ShrinkSelection);
    run(&mut e, Command::ShrinkSelection);
    assert_eq!(show(&e), "a|b cd\nef g|h");
    assert_eq!(e.selections().len(), 2);
}

#[test]
fn shrink_forgets_after_a_move_or_an_edit() {
    let mut e = ed("a|b cd");
    run(&mut e, Command::ExpandSelection);
    run(&mut e, Command::Right { select: false });
    assert_eq!(show(&e), "ab| cd");
    run(&mut e, Command::ShrinkSelection);
    assert_eq!(show(&e), "ab| cd", "the stack is stale after a move");

    let mut e = ed("a|b cd");
    run(&mut e, Command::ExpandSelection);
    run(&mut e, Command::ExpandSelection);
    run(&mut e, ins("x"));
    run(&mut e, Command::ShrinkSelection);
    assert_eq!(show(&e), "x|", "and after an edit");
    // A fresh expand starts a new stack.
    run(&mut e, Command::ExpandSelection);
    assert_eq!(show(&e), "«x»");
    run(&mut e, Command::ShrinkSelection);
    assert_eq!(show(&e), "x|");
}

// --- Change markers (diff.rs) and hunk revert ---------------------------------

fn hunks_of(old: &str, new: &str) -> Vec<diff::LineHunk> {
    let (ol, nl) = (diff::line_ranges(old), diff::line_ranges(new));
    diff::diff_lines(&diff::text_hashes(old, &ol), &diff::text_hashes(new, &nl))
}

fn hk(old: Range<usize>, new: Range<usize>) -> diff::LineHunk {
    diff::LineHunk { old, new }
}

#[test]
fn line_diff_hunks() {
    use diff::HunkKind::*;
    assert_eq!(hunks_of("a\nb\nc\n", "a\nb\nc\n"), vec![]);
    // Changed, added, removed lines, each its own hunk.
    let hs = hunks_of("a\nb\nc\nd\ne\nf\n", "a\nB\nc\nd\nnew\ne\n");
    assert_eq!(hs, vec![hk(1..2, 1..2), hk(4..4, 4..5), hk(5..6, 6..6)]);
    assert_eq!(hs.iter().map(|h| h.kind()).collect::<Vec<_>>(), vec![Modified, Added, Removed]);
    // A new file is one added hunk; an emptied one, one removed hunk.
    assert_eq!(hunks_of("", "x\ny\n"), vec![hk(0..0, 0..2)]);
    assert_eq!(hunks_of("x\ny\n", ""), vec![hk(0..2, 0..0)]);
    // Lines compare with their newline: a dropped final newline shows.
    assert_eq!(hunks_of("a\nb\n", "a\nb"), vec![hk(1..2, 1..2)]);
    // Insert at the top, delete at the end.
    assert_eq!(hunks_of("b\nc\n", "a\nb\n"), vec![hk(0..0, 0..1), hk(1..2, 2..2)]);
    // Rope hashes match text hashes, across chunk boundaries too.
    let big: String = (0..3000).map(|i| format!("line {i} {}\n", "x".repeat(i % 50))).collect();
    let rope = Rope::from_str(&big);
    assert_eq!(diff::rope_hashes(&rope), diff::text_hashes(&big, &diff::line_ranges(&big)));
    assert_eq!(diff::real_lines(&Rope::from_str("a\nb")), 2);
    assert_eq!(diff::real_lines(&Rope::from_str("a\nb\n")), 2);
    assert_eq!(diff::real_lines(&Rope::from_str("")), 0);
}

#[test]
fn line_diff_falls_back_to_one_hunk() {
    // More edits than MAX_D: the changed middle becomes one hunk.
    let n = diff::MAX_D + 10;
    let old: String = (0..n).map(|i| format!("o{i}\n")).collect();
    let new: String = (0..n).map(|i| format!("n{i}\n")).collect();
    let (old, new) = (format!("top\n{old}bottom\n"), format!("top\n{new}bottom\n"));
    assert_eq!(hunks_of(&old, &new), vec![hk(1..1 + n, 1..1 + n)]);
}

/// Reverting every hunk (last first, so earlier line numbers hold) gives
/// back the base text exactly, and each revert is one undo step.
#[test]
fn revert_hunks_restores_base() {
    let cases = [
        ("a\nb\nc\nd\ne\nf\n", "a\nB\nc\nd\nnew\ne\n"),
        ("", "x\ny\n"),
        ("x\ny\n", ""),
        ("a\nb\n", "a\nb"),
        ("a\nb", "a\nb\nc\n"),
        ("fn main() {\n    one();\n    two();\n}\n", "// header\nfn main() {\n    two();\n    three();\n}\n"),
    ];
    for (base, cur) in cases {
        let mut e = Editor::from_text(cur);
        let lines = diff::line_ranges(base);
        let hs = hunks_of(base, cur);
        assert!(!hs.is_empty(), "{base:?} -> {cur:?}");
        for h in hs.iter().rev() {
            let text = if h.old.is_empty() { "" } else { &base[lines[h.old.start].start..lines[h.old.end - 1].end] };
            assert!(e.replace_lines(h.new.clone(), text).text_changed);
        }
        assert_eq!(e.rope().to_string(), base, "{cur:?}");
        assert!(hunks_of(base, &e.rope().to_string()).is_empty());
        // Undo every revert: back to the agent's text.
        for _ in &hs {
            run(&mut e, Command::Undo);
        }
        assert_eq!(e.rope().to_string(), cur);
    }
    // One hunk of several: only that one goes back; the caret lands on it.
    let mut e = Editor::from_text("a\nB\nc\nd\nnew\ne\n");
    e.replace_lines(4..5, "");
    assert_eq!(e.rope().to_string(), "a\nB\nc\nd\ne\n");
    assert_eq!(e.primary().head, Point { line: 4, col: 0 });
}

/// Timing: `cargo test --release -p qur_editor -- --ignored --nocapture diff_timing`.
#[test]
#[ignore]
fn diff_timing() {
    let old: String = (0..10_000).map(|i| format!("    let value_{i} = compute({i}, \"some text\");\n")).collect();
    let new: String = old
        .lines()
        .enumerate()
        .filter(|(i, _)| i % 333 != 0)
        .map(|(i, l)| if i % 50 == 0 { format!("{l} // changed\n") } else { format!("{l}\n") })
        .collect();
    let (ol, rope) = (diff::line_ranges(&old), Rope::from_str(&new));
    let base = diff::text_hashes(&old, &ol);
    let t = Instant::now();
    let hs = diff::diff_lines(&base, &diff::rope_hashes(&rope));
    println!("10k lines, {} hunks: {:.2} ms", hs.len(), t.elapsed().as_secs_f64() * 1000.0);
}
