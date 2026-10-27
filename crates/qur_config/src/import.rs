//! Settings → Import from: another terminal's config as `[terminal]` keys,
//! so moving to Qur keeps your font, colors and habits.

use std::path::{Path, PathBuf};

use crate::Value;

/// The terminals Qur can import from: (id, name).
pub const SOURCES: [(&str, &str); 3] = [("ghostty", "Ghostty"), ("kitty", "kitty"), ("alacritty", "Alacritty")];

/// Where `source` keeps its config, if it's there.
pub fn find(source: &str) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let candidates = match source {
        "ghostty" => vec![xdg.join("ghostty/config"), home.join("Library/Application Support/com.mitchellh.ghostty/config")],
        "kitty" => vec![xdg.join("kitty/kitty.conf")],
        "alacritty" => vec![xdg.join("alacritty/alacritty.toml"), home.join(".alacritty.toml")],
        _ => vec![],
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// `source`'s config text as `[terminal]` keys and values. `dir` is the
/// config's folder (Ghostty themes live beside it). Unknown keys are skipped.
pub fn terminal_keys(source: &str, text: &str, dir: &Path) -> Vec<(&'static str, Value)> {
    match source {
        "ghostty" => ghostty(text, dir),
        "kitty" => kitty(text),
        "alacritty" => alacritty(text),
        _ => Vec::new(),
    }
}

/// `#rrggbb` from `rrggbb`, `#rrggbb` or `0xrrggbb`.
fn color(v: &str) -> Option<Value> {
    let h = v.trim().trim_matches(['"', '\'']).trim_start_matches('#').trim_start_matches("0x");
    (h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{}", h.to_ascii_lowercase()).into())
}

fn num(v: &str) -> Option<Value> {
    // Ghostty's and kitty's paddings may give two numbers: the first is used.
    v.split([',', ' ']).find(|s| !s.is_empty())?.trim().parse::<f64>().ok().map(Value::Float)
}

fn truthy(v: &str) -> bool {
    !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "no" | "never" | "off" | "0" | "none")
}

/// Keys in file order, the last of a repeated one winning; palette entries
/// collected into one list.
struct Out {
    keys: Vec<(&'static str, Value)>,
    palette: Vec<(usize, Value)>,
}

impl Out {
    fn new() -> Out {
        Out { keys: Vec::new(), palette: Vec::new() }
    }
    fn put(&mut self, k: &'static str, v: Option<Value>) {
        if let Some(v) = v {
            self.keys.retain(|(x, _)| *x != k);
            self.keys.push((k, v));
        }
    }
    fn color(&mut self, n: usize, v: &str) {
        if let (true, Some(c)) = (n < 16, color(v)) {
            self.palette.retain(|(i, _)| *i != n);
            self.palette.push((n, c));
        }
    }
    fn done(mut self) -> Vec<(&'static str, Value)> {
        if !self.palette.is_empty() {
            self.palette.sort_by_key(|(i, _)| *i);
            let list = self.palette.into_iter().map(|(i, c)| format!("{i}={}", if let Value::Str(c) = c { c } else { String::new() })).collect();
            self.keys.push(("palette", Value::List(list)));
        }
        self.keys
    }
}

fn ghostty(text: &str, dir: &Path) -> Vec<(&'static str, Value)> {
    let mut o = Out::new();
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "font-family" if !v.is_empty() => o.put("font_family", Some(v.into())),
            "font-size" => o.put("font_size", num(v)),
            "window-padding-x" => o.put("padding_x", num(v)),
            "window-padding-y" => o.put("padding_y", num(v)),
            "cursor-style" => o.put("cursor_style", Some(if v.starts_with("bar") { "bar" } else if v.starts_with("underline") { "underline" } else { "block" }.into())),
            "cursor-style-blink" => o.put("cursor_blink", Some(truthy(v).into())),
            "cursor-color" => o.put("cursor_color", color(v)),
            "foreground" => o.put("foreground", color(v)),
            "background" => o.put("background", color(v)),
            "selection-foreground" => o.put("selection_foreground", color(v)),
            "selection-background" => o.put("selection_background", color(v)),
            "palette" => {
                if let Some((n, c)) = v.split_once('=') {
                    o.color(n.trim().parse().unwrap_or(99), c);
                }
            }
            // A theme file beside the config (Ghostty's built-in themes live in its app).
            "theme" => {
                let name = v.rsplit(',').next().unwrap_or(v).trim_start_matches("light:").trim_start_matches("dark:").trim();
                let p = if Path::new(name).is_absolute() { PathBuf::from(name) } else { dir.join("themes").join(name) };
                o.put("theme", p.is_file().then(|| p.to_string_lossy().into_owned().into()));
            }
            "bold-is-bright" => o.put("bold_is_bright", Some(truthy(v).into())),
            "macos-option-as-alt" => o.put("option_as_alt", Some(truthy(v).into())),
            "copy-on-select" => o.put("copy_on_select", Some(truthy(v).into())),
            "mouse-scroll-multiplier" => o.put("scroll_multiplier", num(v)),
            "focus-follows-mouse" => o.put("focus_follows_mouse", Some(truthy(v).into())),
            "command" if !v.is_empty() => o.put("shell", Some(v.into())),
            _ => {}
        }
    }
    o.done()
}

fn kitty(text: &str) -> Vec<(&'static str, Value)> {
    let mut o = Out::new();
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = line.split_once(char::is_whitespace) else { continue };
        let v = v.trim();
        match k {
            "font_family" if v != "monospace" => o.put("font_family", Some(v.into())),
            "font_size" => o.put("font_size", num(v)),
            "window_padding_width" => {
                o.put("padding_y", num(v));
                // Two to four values: top, right, bottom, left.
                let x = v.split_whitespace().nth(1).unwrap_or(v);
                o.put("padding_x", num(x));
            }
            "cursor_shape" => o.put("cursor_style", Some(match v { "beam" => "bar", "underline" => "underline", _ => "block" }.into())),
            "cursor_blink_interval" => o.put("cursor_blink", Some((v.parse::<f64>().unwrap_or(-1.) != 0.).into())),
            "cursor" => o.put("cursor_color", color(v)),
            "foreground" => o.put("foreground", color(v)),
            "background" => o.put("background", color(v)),
            "selection_foreground" => o.put("selection_foreground", color(v)),
            "selection_background" => o.put("selection_background", color(v)),
            "copy_on_select" => o.put("copy_on_select", Some(truthy(v).into())),
            "macos_option_as_alt" => o.put("option_as_alt", Some(truthy(v).into())),
            "wheel_scroll_multiplier" => o.put("scroll_multiplier", num(v)),
            "focus_follows_mouse" => o.put("focus_follows_mouse", Some(truthy(v).into())),
            "shell" if v != "." => o.put("shell", Some(v.into())),
            k => {
                if let Some(n) = k.strip_prefix("color").and_then(|n| n.parse().ok()) {
                    o.color(n, v);
                }
            }
        }
    }
    o.done()
}

fn alacritty(text: &str) -> Vec<(&'static str, Value)> {
    let mut o = Out::new();
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return Vec::new() };
    let get = |path: &str| {
        let mut item = doc.as_item();
        for k in path.split('.') {
            item = item.get(k)?;
        }
        Some(item)
    };
    let s = |path: &str| get(path).and_then(|i| i.as_str()).map(str::to_string);
    let n = |path: &str| get(path).and_then(|i| i.as_float().or_else(|| i.as_integer().map(|v| v as f64))).map(Value::Float);
    o.put("font_family", s("font.normal.family").map(Into::into));
    o.put("font_size", n("font.size"));
    o.put("padding_x", n("window.padding.x"));
    o.put("padding_y", n("window.padding.y"));
    let shape = s("cursor.style.shape").or_else(|| s("cursor.style"));
    o.put("cursor_style", shape.map(|v| match v.to_ascii_lowercase().as_str() { "beam" => "bar", "underline" => "underline", _ => "block" }.into()));
    o.put("cursor_blink", s("cursor.style.blinking").map(|v| matches!(v.to_ascii_lowercase().as_str(), "on" | "always").into()));
    for (key, path) in [
        ("foreground", "colors.primary.foreground"),
        ("background", "colors.primary.background"),
        ("cursor_color", "colors.cursor.cursor"),
        ("selection_foreground", "colors.selection.text"),
        ("selection_background", "colors.selection.background"),
    ] {
        o.put(key, s(path).as_deref().and_then(color));
    }
    for (base, set) in [(0, "normal"), (8, "bright")] {
        for (i, name) in ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"].iter().enumerate() {
            if let Some(c) = s(&format!("colors.{set}.{name}")) {
                o.color(base + i, &c);
            }
        }
    }
    o.put("option_as_alt", s("window.option_as_alt").map(|v| (v != "None").into()));
    o.put("copy_on_select", get("selection.save_to_clipboard").and_then(|i| i.as_bool()).map(Into::into));
    o.put("scroll_multiplier", n("scrolling.multiplier"));
    let shell = s("terminal.shell").or_else(|| s("terminal.shell.program")).or_else(|| s("shell.program")).or_else(|| s("shell"));
    o.put("shell", shell.map(Into::into));
    o.done()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(v: &[(&str, Value)]) -> Vec<String> {
        v.iter()
            .map(|(k, v)| match v {
                Value::Str(s) => format!("{k}={s:?}"),
                Value::Float(f) => format!("{k}={f:?}"),
                Value::Bool(b) => format!("{k}={b}"),
                Value::Int(i) => format!("{k}={i}"),
                Value::List(l) => format!("{k}={l:?}"),
            })
            .collect()
    }

    #[test]
    fn ghostty_config() {
        let dir = std::env::temp_dir().join("qur-import-test");
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("themes/mine"), "background = 000000\n").unwrap();
        let text = "# c\nfont-family = \"JetBrains Mono\"\nfont-size = 14\nwindow-padding-x = 8,12\ncursor-style = bar\npalette = 1=ff5555\npalette = 0=#000000\nbackground = 282a36\ntheme = mine\nmacos-option-as-alt = left\nunknown = 1\n";
        let out = show(&terminal_keys("ghostty", text, &dir));
        assert_eq!(
            out,
            [
                "font_family=\"JetBrains Mono\"",
                "font_size=14.0",
                "padding_x=8.0",
                "cursor_style=\"bar\"",
                "background=\"#282a36\"",
                &format!("theme={:?}", dir.join("themes/mine").to_string_lossy()),
                "option_as_alt=true",
                "palette=[\"0=#000000\", \"1=#ff5555\"]",
            ]
        );
        assert!(terminal_keys("ghostty", "theme = Dracula\n", &dir).is_empty(), "built-in theme names are skipped");
    }

    #[test]
    fn kitty_config() {
        let text = "font_family Fira Code\nfont_size 13.5\ncursor_shape beam\ncursor_blink_interval 0\ncolor1 #cc0000\nwindow_padding_width 4 10\ncopy_on_select clipboard\nshell .\n";
        let out = show(&terminal_keys("kitty", text, Path::new("/")));
        assert_eq!(
            out,
            ["font_family=\"Fira Code\"", "font_size=13.5", "cursor_style=\"bar\"", "cursor_blink=false", "padding_y=4.0", "padding_x=10.0", "copy_on_select=true", "palette=[\"1=#cc0000\"]"]
        );
    }

    #[test]
    fn alacritty_config() {
        let text = "[font]\nsize = 12\n[font.normal]\nfamily = \"Menlo\"\n[cursor.style]\nshape = \"Underline\"\nblinking = \"On\"\n[colors.primary]\nbackground = \"0x1d1f21\"\n[colors.bright]\nred = \"#ff0000\"\n[window]\noption_as_alt = \"Both\"\n[terminal.shell]\nprogram = \"/opt/homebrew/bin/fish\"\n";
        let out = show(&terminal_keys("alacritty", text, Path::new("/")));
        assert_eq!(
            out,
            [
                "font_family=\"Menlo\"",
                "font_size=12.0",
                "cursor_style=\"underline\"",
                "cursor_blink=true",
                "background=\"#1d1f21\"",
                "option_as_alt=true",
                "shell=\"/opt/homebrew/bin/fish\"",
                "palette=[\"9=#ff0000\"]",
            ]
        );
        assert!(terminal_keys("alacritty", "not toml [", Path::new("/")).is_empty());
    }
}
