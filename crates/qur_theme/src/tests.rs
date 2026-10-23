use super::*;
use std::time::Instant;

fn reg() -> &'static Registry {
    Registry::global()
}

#[test]
fn all_builtins_parse_once_and_fast() {
    let bytes: usize = BUILTIN.iter().map(|(_, s)| s.len()).sum();
    let t = Instant::now();
    let r = Registry::load().expect("themes parse");
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("parsed {} themes ({} KB embedded) in {ms:.2} ms", r.themes.len(), bytes / 1024);
    assert_eq!(r.themes.len(), 17);
    assert!(bytes < 200 * 1024, "embedded themes are {bytes} bytes");
    let names: Vec<_> = r.themes.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names[0], "Qur Dark");
    assert_eq!(names[1], "Qur Light");
    let mut dedup = names.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), names.len(), "duplicate theme names");
    // Picker order: Qur, Popular, Accessibility.
    let groups: Vec<_> = r.themes.iter().map(|t| t.group).collect();
    let rank = |g: &str| ["Qur", "Popular", "Accessibility"].iter().position(|x| *x == g).unwrap();
    assert!(groups.windows(2).all(|w| rank(w[0]) <= rank(w[1])), "{groups:?}");
}

#[test]
fn credits_on_every_ported_theme() {
    for t in reg().all() {
        match t.group {
            "Popular" => {
                let c = t.credit.as_ref().unwrap_or_else(|| panic!("{} has no credit", t.name));
                assert!(["MIT", "Apache-2.0"].contains(&c.license), "{}: license {}", t.name, c.license);
                assert!(c.url.starts_with("https://github.com/"), "{}", t.name);
            }
            _ => assert!(t.credit.is_none(), "{} is Qur's own", t.name),
        }
    }
}

#[test]
fn resolve_auto_named_and_unknown() {
    let r = reg();
    assert_eq!(r.resolve(&ThemeChoice::Auto, true).name, "Qur Dark");
    assert_eq!(r.resolve(&ThemeChoice::Auto, false).name, "Qur Light");
    assert_eq!(r.resolve(&ThemeChoice::Named("Nord".into()), false).name, "Nord");
    assert_eq!(r.resolve(&ThemeChoice::Named("tokyo night".into()), false).name, "Tokyo Night");
    assert_eq!(r.resolve(&ThemeChoice::Named("No Such Theme".into()), true).name, "Qur Dark");
    assert_eq!(r.resolve(&ThemeChoice::Named("No Such Theme".into()), false).name, "Qur Light");
}

#[test]
fn syntax_dotted_fallback() {
    let t = reg().get("One Dark").unwrap();
    // Exact.
    assert_eq!(t.syntax_style("string.escape").color, t.syntax_style("escape").color);
    // "function.method.call" -> "function".
    assert_eq!(t.syntax_style("function.method.call"), t.syntax_style("function"));
    // Unknown names -> editor text.
    assert_eq!(t.syntax_style("nonsense").color, Some(t.editor.text));
    // Italic comments carried through.
    assert!(t.syntax_style("comment").italic);
    // Qur Dark: bold keywords.
    assert!(reg().get("Qur Dark").unwrap().syntax_style("keyword").bold);
}

#[test]
fn partial_theme_falls_back_to_qur_by_luminance() {
    let dark = Theme::from_toml(
        r##"
        name = "Partial Dark"
        [ui]
        window_bg = "#101418"
        text = "#e0e0e0"
        [syntax]
        keyword = "#ff00ff"
        "##,
    )
    .unwrap();
    let qd = reg().get("Qur Dark").unwrap();
    assert_eq!(dark.appearance, Appearance::Dark);
    assert_eq!(dark.ui.window_bg, Rgba::rgb(0x101418));
    assert_eq!(dark.ui.text, Rgba::rgb(0xe0e0e0));
    assert_eq!(dark.ui.sidebar_bg, qd.ui.sidebar_bg); // missing -> Qur Dark
    assert_eq!(dark.terminal.ansi, qd.terminal.ansi);
    assert_eq!(dark.accents[3].base, qd.accents[3].base);
    assert_eq!(dark.syntax_style("keyword").color, Some(Rgba::rgb(0xff00ff)));
    assert_eq!(dark.syntax_style("string"), qd.syntax_style("string")); // from fallback

    let light = Theme::from_toml(
        r##"
        name = "Partial Light"
        accents = ["#123456"]
        [ui]
        editor_bg = "#fafafa"
        "##,
    )
    .unwrap();
    let ql = reg().get("Qur Light").unwrap();
    assert_eq!(light.appearance, Appearance::Light);
    assert_eq!(light.ui.text, ql.ui.text);
    assert_eq!(light.accents[0].base, Rgba::rgb(0x123456));
    assert_eq!(light.accents[1].base, ql.accents[1].base);

    assert!(Theme::from_toml("name = 3").is_err());
    assert!(Theme::from_toml("name = \"x\"\n[ui]\nwindow_bg = \"blue\"").is_err());
    assert!(Theme::from_toml("name = \"x\"\n[ui]\nnot_a_token = \"#000000\"").is_err());
}

#[test]
fn qur_bases_are_complete() {
    // build() with no base reports every missing token; load() would fail.
    for file in ["qur_dark", "qur_light"] {
        let src = BUILTIN.iter().find(|(f, _)| *f == file).unwrap().1;
        let raw: schema::RawTheme = toml_edit::de::from_str(src).unwrap();
        build(raw, None).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn logo_variant_by_background() {
    assert!(reg().get("Qur Dark").unwrap().logo_is_white());
    assert!(!reg().get("Qur Light").unwrap().logo_is_white());
    for t in reg().all() {
        assert_eq!(t.logo_is_white(), t.appearance == Appearance::Dark, "{}", t.name);
    }
}

// ---------------------------------------------------------------------------
// Contrast rules (handoff 12a). Failures name theme, token and ratio.
// ---------------------------------------------------------------------------

fn check(fails: &mut Vec<String>, theme: &Theme, what: &str, fg: Rgba, bg_name: &str, bg: Rgba, min: f32) {
    let fg = blend(fg, bg);
    let r = contrast_ratio(fg, bg);
    if r < min {
        fails.push(format!("{}: {what} on {bg_name} = {r:.2}:1 (needs {min})", theme.name));
    }
}

#[test]
fn contrast_rules_hold_for_every_theme() {
    let mut fails = Vec::new();
    for t in reg().all() {
        let body = if t.high_contrast { 7.0 } else { 4.5 };
        let u = &t.ui;
        let surfaces = [
            ("window_bg", u.window_bg),
            ("sidebar_bg", u.sidebar_bg),
            ("editor_bg", u.editor_bg),
            ("panel_bg", u.panel_bg),
            ("overlay_bg", u.overlay_bg),
        ];
        for (bg_name, bg) in surfaces {
            check(&mut fails, t, "ui.text", u.text, bg_name, bg, body);
            check(&mut fails, t, "ui.text_secondary", u.text_secondary, bg_name, bg, 4.5);
            check(&mut fails, t, "ui.text_muted", u.text_muted, bg_name, bg, 3.0);
        }
        check(&mut fails, t, "editor.text", t.editor.text, "editor_bg", u.editor_bg, body);
        for (i, a) in t.accents.iter().enumerate() {
            check(&mut fails, t, &format!("accent[{i}]"), a.base, "sidebar_bg", u.sidebar_bg, 3.0);
            check(&mut fails, t, &format!("accent[{i}]"), a.base, "editor_bg", u.editor_bg, 3.0);
            // Row text stays readable on the strongest heat tint.
            check(&mut fails, t, &format!("ui.text on accent[{i}].heat[3]"), u.text, "heat", a.heat[3], 4.5);
        }
        for (name, min) in [("keyword", 4.5), ("string", 4.5), ("function", 4.5), ("type", 4.5), ("comment", 3.0)] {
            let c = t.syntax_style(name).color.unwrap();
            let min = if t.high_contrast && name != "comment" { 7.0 } else { min };
            check(&mut fails, t, &format!("syntax.{name}"), c, "editor_bg", u.editor_bg, min);
        }
    }
    assert!(fails.is_empty(), "contrast failures:\n{}", fails.join("\n"));
}

#[test]
fn accents_are_distinguishable() {
    let mut fails = Vec::new();
    for t in reg().all() {
        let mut min = f32::MAX;
        for i in 0..6 {
            for j in i + 1..6 {
                let d = delta_e(t.accents[i].base, t.accents[j].base);
                min = min.min(d);
                if d < 15.0 {
                    fails.push(format!(
                        "{}: accents {i} {} and {j} {} differ by CIEDE2000 {d:.1} (needs 15)",
                        t.name, t.accents[i].base, t.accents[j].base
                    ));
                }
            }
        }
        eprintln!("{:22} min accent dE2000 {min:5.1}", t.name);
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

// ---------------------------------------------------------------------------
// Translucent windows (decision 8)
// ---------------------------------------------------------------------------

#[test]
fn opacity_floors_keep_text_readable_on_any_desktop() {
    let mut worst = 0.0f32;
    for t in reg().all() {
        let floor = t.min_opacity();
        assert!((0.5..=1.0).contains(&floor));
        eprintln!("{:22} lowest readable opacity {floor:.2}", t.name);
        worst = worst.max(floor);
        let u = &t.ui;
        let a = (floor * 255.0).round() as u8;
        for desktop in [Rgba(0xffffffff), Rgba(0x000000ff)] {
            for (name, s) in [("window_bg", u.window_bg), ("sidebar_bg", u.sidebar_bg), ("editor_bg", u.editor_bg)] {
                let bg = blend(s.with_alpha(a), desktop);
                let r = contrast_ratio(u.text, bg);
                assert!(r >= 4.5, "{} at opacity {floor}: ui.text on {name} over {desktop} = {r:.2}", t.name);
            }
        }
    }
    eprintln!("one floor for every theme would be {worst:.2}; slider floor MIN_OPACITY = {MIN_OPACITY}");
    // The slider is 60–100% everywhere (decision 38); the Qur themes (the
    // defaults) stay readable across most of it.
    assert_eq!(MIN_OPACITY, 0.6);
    for name in ["Qur Dark", "Qur Light"] {
        assert!(reg().get(name).unwrap().min_opacity() <= 0.65, "{name}");
    }
}

#[test]
fn heat_tints_are_visible_and_ordered() {
    for t in reg().all() {
        for (i, a) in t.accents.iter().enumerate() {
            // Built-ins guarantee the strongest level can reach MIN_HEAT.
            let min_tint = blend(a.base.with_alpha((schema::MIN_HEAT * 255.0).round() as u8), t.ui.sidebar_bg);
            assert!(contrast_ratio(t.ui.text, min_tint) >= 4.5, "{}: accent[{i}] can't reach MIN_HEAT", t.name);
            let strongest = delta_e(a.heat[3], t.ui.sidebar_bg);
            assert!(strongest >= 4.0, "{}: accent[{i}] heat 4 barely visible (dE {strongest:.1})", t.name);
            let d: Vec<f32> = a.heat.iter().map(|h| delta_e(*h, t.ui.sidebar_bg)).collect();
            assert!(d.windows(2).all(|w| w[0] <= w[1]), "{}: accent[{i}] heat not increasing {d:?}", t.name);
        }
    }
}
