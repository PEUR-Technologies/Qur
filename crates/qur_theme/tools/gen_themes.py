#!/usr/bin/env python3
"""Generate crates/qur_theme/themes/*.toml.

Palette values are copied from each theme's upstream source (URLs below, fetched
2026-10-23). Upstream colors that miss Qur's contrast rules (handoff 12a) are
nudged in lightness, the smallest step that passes; every nudge is written as a
comment into the TOML and listed in docs/THEME_CREDITS.md.

The TOML files are what ships; this script documents how they were made.
Run: python3 crates/qur_theme/tools/gen_themes.py
"""
import colorsys
import math
import os

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "themes")

# ---------------------------------------------------------------------------
# Color math (mirrors src/color.rs)
# ---------------------------------------------------------------------------

def parse(h):
    h = h.lstrip("#")
    if len(h) == 6:
        h += "ff"
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4, 6))


def fmt(c):
    r, g, b, a = c
    return f"#{r:02x}{g:02x}{b:02x}" + ("" if a == 255 else f"{a:02x}")


def hsl(h, s, l, a=1.0):
    r, g, b = colorsys.hls_to_rgb((h % 360) / 360, l / 100, s / 100)
    return fmt((round(r * 255), round(g * 255), round(b * 255), round(a * 255)))


def alpha(c, a):
    r, g, b, _ = parse(c)
    return fmt((r, g, b, round(a * 255)))


def blend(fg, bg):
    fr, fg_, fb, fa = parse(fg)
    br, bg_, bb, ba = parse(bg)
    a = fa / 255
    mix = lambda f, b: round(f * a + b * (1 - a))
    return fmt((mix(fr, br), mix(fg_, bg_), mix(fb, bb), 255))


def lin(v):
    v /= 255
    return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4


def lum(c):
    r, g, b, _ = parse(c)
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)


def contrast(a, b):
    la, lb = lum(a), lum(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def lab(c):
    r, g, b, _ = parse(c)
    r, g, b = lin(r), lin(g), lin(b)
    x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047
    y = (0.2126729 * r + 0.7151522 * g + 0.0721750 * b)
    z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883
    f = lambda t: t ** (1 / 3) if t > 216 / 24389 else (24389 / 27 * t + 16) / 116
    fx, fy, fz = f(x), f(y), f(z)
    return 116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)


def de2000(c1, c2):
    L1, a1, b1 = lab(c1)
    L2, a2, b2 = lab(c2)
    C1, C2 = math.hypot(a1, b1), math.hypot(a2, b2)
    Cb = (C1 + C2) / 2
    G = 0.5 * (1 - math.sqrt(Cb ** 7 / (Cb ** 7 + 25 ** 7)))
    a1p, a2p = (1 + G) * a1, (1 + G) * a2
    C1p, C2p = math.hypot(a1p, b1), math.hypot(a2p, b2)
    h1p = math.degrees(math.atan2(b1, a1p)) % 360
    h2p = math.degrees(math.atan2(b2, a2p)) % 360
    dLp, dCp = L2 - L1, C2p - C1p
    if C1p * C2p == 0:
        dhp = 0
    elif abs(h2p - h1p) <= 180:
        dhp = h2p - h1p
    elif h2p - h1p > 180:
        dhp = h2p - h1p - 360
    else:
        dhp = h2p - h1p + 360
    dHp = 2 * math.sqrt(C1p * C2p) * math.sin(math.radians(dhp / 2))
    Lbp, Cbp = (L1 + L2) / 2, (C1p + C2p) / 2
    if C1p * C2p == 0:
        hbp = h1p + h2p
    elif abs(h1p - h2p) <= 180:
        hbp = (h1p + h2p) / 2
    elif h1p + h2p < 360:
        hbp = (h1p + h2p + 360) / 2
    else:
        hbp = (h1p + h2p - 360) / 2
    T = (1 - 0.17 * math.cos(math.radians(hbp - 30)) + 0.24 * math.cos(math.radians(2 * hbp))
         + 0.32 * math.cos(math.radians(3 * hbp + 6)) - 0.20 * math.cos(math.radians(4 * hbp - 63)))
    dth = 30 * math.exp(-(((hbp - 275) / 25) ** 2))
    Rc = 2 * math.sqrt(Cbp ** 7 / (Cbp ** 7 + 25 ** 7))
    Sl = 1 + 0.015 * (Lbp - 50) ** 2 / math.sqrt(20 + (Lbp - 50) ** 2)
    Sc = 1 + 0.045 * Cbp
    Sh = 1 + 0.015 * Cbp * T
    Rt = -math.sin(math.radians(2 * dth)) * Rc
    return math.sqrt((dLp / Sl) ** 2 + (dCp / Sc) ** 2 + (dHp / Sh) ** 2 + Rt * (dCp / Sc) * (dHp / Sh))


def shift_l(c, dl):
    r, g, b, a = parse(c)
    h, l, s = colorsys.rgb_to_hls(r / 255, g / 255, b / 255)
    l = min(1, max(0, l + dl))
    r, g, b = colorsys.hls_to_rgb(h, l, s)
    return fmt((round(r * 255), round(g * 255), round(b * 255), a))


def fix(c, bgs, minimum, notes, label):
    """Smallest lightness change so `c` reaches `minimum` contrast on every bg."""
    worst = lambda x: min(contrast(x, b) for b in bgs)
    if worst(c) >= minimum:
        return c
    dark_bg = sum(lum(b) for b in bgs) / len(bgs) < 0.18
    step = 0.005 if dark_bg else -0.005
    x = c
    for _ in range(200):
        x = shift_l(x, step)
        if worst(x) >= minimum + 0.02:
            notes.append(f"{label}: {c} -> {x} (contrast {worst(c):.2f} -> {worst(x):.2f})")
            return x
    raise SystemExit(f"cannot fix {label}")

# ---------------------------------------------------------------------------
# Theme assembly
# ---------------------------------------------------------------------------

MIN_HEAT = 0.15   # must match src/schema.rs MIN_HEAT


def build(t):
    notes = []
    ui, ed = t["ui"], t["editor"]
    hc = t.get("high_contrast", False)
    body = 7.0 if hc else 4.5
    surfaces = [ui[k] for k in ("window_bg", "sidebar_bg", "editor_bg", "panel_bg", "overlay_bg")]
    ui["text"] = fix(ui["text"], surfaces, body, notes, "ui.text")
    ed["text"] = fix(ed["text"], [ui["editor_bg"]], body, notes, "editor.text")
    ui["text_secondary"] = fix(ui["text_secondary"], surfaces, 4.5, notes, "ui.text_secondary")
    ui["text_muted"] = fix(ui["text_muted"], surfaces, 3.0, notes, "ui.text_muted")
    syn = t["syntax"]
    for name, minimum in (("keyword", 4.5), ("string", 4.5), ("function", 4.5), ("type", 4.5), ("comment", 3.0)):
        v = syn[name]
        color = v if isinstance(v, str) else v["color"]
        fixed = fix(color, [ui["editor_bg"]], 7.0 if hc and name != "comment" else minimum, notes, f"syntax.{name}")
        syn[name] = fixed if isinstance(v, str) else dict(v, color=fixed)
    # Agent accents: pick 6 of the candidates maximising the minimum pairwise
    # CIEDE2000, after making each readable (>= 3:1) on sidebar and editor.
    cand_notes = {}
    cands = []
    for c in t["accents"]:
        n = []
        cands.append(fix(c, [ui["sidebar_bg"], ui["editor_bg"]], 3.0, n, f"accent {c}"))
        cand_notes[cands[-1]] = n
    best, best_score = None, -1
    from itertools import combinations
    for combo in combinations(cands, 6):
        score = min(de2000(a, b) for a, b in combinations(combo, 2))
        if score > best_score:
            best, best_score = combo, score
    # Keep the candidates' given order (first = most characteristic).
    t["accents_final"] = [c for c in cands if c in best][:6]
    for c in t["accents_final"]:
        notes.extend(cand_notes[c])
    # Heat tints (src/schema.rs) adapt to keep row text at 4.5:1, but must be
    # able to reach at least 15% accent strength to stay visible.
    tints = [blend(alpha(a, MIN_HEAT), ui["sidebar_bg"]) for a in t["accents_final"]]
    ui["text"] = fix(ui["text"], surfaces + tints, body, notes, "ui.text (heat tints)")
    t["accent_min_de"] = best_score
    t["notes"] = notes
    return t


def q(v):
    return f'"{v}"'


def write(t):
    lines = [
        f"# {t['name']} for Qur. Generated by tools/gen_themes.py; edit there.",
    ]
    cr = t.get("credit")
    if cr:
        lines.append(f"# Upstream: {cr['url']} ({cr['license']}, {cr['author']})")
        lines.append(f"# Palette source: {cr['source']}")
    for n in t["notes"]:
        lines.append(f"# contrast adjustment: {n}")
    lines += [
        "",
        f"name = {q(t['name'])}",
        f"appearance = {q(t['appearance'])}",
        f"group = {q(t['group'])}",
        f"high_contrast = {'true' if t.get('high_contrast') else 'false'}",
        f"# min pairwise CIEDE2000 between accents: {t['accent_min_de']:.1f}",
        f"accents = [{', '.join(q(a) for a in t['accents_final'])}]",
    ]
    if cr:
        lines += ["", "[credit]", f"author = {q(cr['author'])}", f"license = {q(cr['license'])}", f"url = {q(cr['url'])}"]
    for section in ("ui", "editor", "diff"):
        lines += ["", f"[{section}]"] + [f"{k} = {q(v)}" for k, v in t[section].items()]
    term = t["terminal"]
    lines += ["", "[terminal]", "ansi = [" + ", ".join(q(c) for c in term["ansi"]) + "]"]
    lines += [f"{k} = {q(v)}" for k, v in term.items() if k != "ansi"]
    lines += ["", "[syntax]"]
    for k, v in t["syntax"].items():
        key = f'"{k}"' if "." in k else k
        if isinstance(v, str):
            lines.append(f"{key} = {q(v)}")
        else:
            parts = [f"color = {q(v['color'])}"] + [f"{f} = true" for f in ("bold", "italic") if v.get(f)]
            lines.append(f"{key} = {{ {', '.join(parts)} }}")
    slug = t["name"].lower().replace(" ", "_")
    with open(os.path.join(OUT, f"{slug}.toml"), "w") as f:
        f.write("\n".join(lines) + "\n")
    return slug


def it(c):
    return {"color": c, "italic": True}


def bold(c):
    return {"color": c, "bold": True}


def diff(green, red, yellow, line_a=0.15, word_a=0.32):
    return {
        "added_line": alpha(green, line_a), "removed_line": alpha(red, line_a),
        "modified_line": alpha(yellow, line_a), "added_word": alpha(green, word_a),
        "removed_word": alpha(red, word_a), "added_text": green, "removed_text": red,
    }


def ui_set(window, top, sidebar, editor, panel, overlay, border, border_strong, text, text2, muted,
           on_accent, hover, active, selection, sel_inactive, focus, button_bg, button_text,
           primary_bg, primary_text, error, warning, success, drawer=None, bar=None, note=None):
    return {
        "window_bg": window, "top_bar_bg": top, "sidebar_bg": sidebar, "editor_bg": editor,
        "panel_bg": panel, "drawer_bg": drawer or panel, "overlay_bg": overlay, "border": border,
        "border_strong": border_strong, "text": text, "text_secondary": text2, "text_muted": muted,
        "text_on_accent": on_accent, "hover_bg": hover, "active_bg": active, "selection": selection,
        "selection_inactive": sel_inactive, "focus_ring": focus, "button_bg": button_bg,
        "button_text": button_text, "button_primary_bg": primary_bg, "button_primary_text": primary_text,
        "permission_bar_bg": bar or overlay, "notification_bg": note or overlay,
        "error": error, "warning": warning, "success": success,
    }


def ed_set(text, cursor, current_line, bracket, match, match_active, gutter, gutter_active, ws, guide):
    return {
        "text": text, "cursor": cursor, "current_line": current_line, "bracket_match": bracket,
        "search_match": match, "search_match_active": match_active, "gutter_text": gutter,
        "gutter_text_active": gutter_active, "whitespace": ws, "indent_guide": guide,
    }


def term(ansi, fg, bg, cursor, selection):
    assert len(ansi) == 16
    return {"ansi": ansi, "foreground": fg, "background": bg, "cursor": cursor, "selection": selection}

# ---------------------------------------------------------------------------
# Themes
# ---------------------------------------------------------------------------

THEMES = []

# --- Qur Dark / Qur Light: brand black and white, monochrome UI -------------
THEMES.append(dict(
    name="Qur Dark", appearance="dark", group="Qur",
    ui=ui_set("#000000", "#000000", "#0a0a0a", "#000000", "#000000", "#141414", "#222222", "#3a3a3a",
              "#ffffff", "#b8b8b8", "#8a8a8a", "#000000", "#161616", "#232323", "#333333", "#262626",
              "#ffffff", "#1a1a1a", "#ffffff", "#ffffff", "#000000", "#ff6b6b", "#f2c14e", "#4ade80",
              drawer="#0a0a0a"),
    editor=ed_set("#ededed", "#ffffff", "#0f0f0f", "#3a3a3a", "#2e2e2e", "#555555", "#6a6a6a", "#d0d0d0",
                  "#2a2a2a", "#1a1a1a"),
    diff=diff("#4ade80", "#ff6b6b", "#f2c14e"),
    terminal=term(["#1a1a1a", "#ff6b6b", "#4ade80", "#f2c14e", "#5b9bff", "#d78bf5", "#5cd6d6", "#d0d0d0",
                   "#5a5a5a", "#ff8a8a", "#74e8a0", "#f7d27a", "#85b5ff", "#e3a8fa", "#8ae6e6", "#ffffff"],
                  "#ededed", "#000000", "#ffffff", "#333333"),
    accents=["#ff7a45", "#3ddc97", "#5b9bff", "#f062c0", "#f5c04a", "#a77bff"],
    syntax={
        "comment": it("#7c7c7c"), "keyword": bold("#ffffff"), "function": "#ffffff",
        "type": "#8fb8de", "constructor": "#8fb8de", "namespace": "#8fb8de",
        "string": "#9ece9a", "string.escape": "#e5b567", "string.regex": "#e5b567", "escape": "#e5b567",
        "number": "#e5b567", "boolean": "#e5b567", "constant": "#e5b567",
        "variable": "#ededed", "variable.builtin": it("#8fb8de"), "variable.parameter": "#d4d4d4",
        "property": "#d4d4d4", "attribute": "#b0b0b0", "tag": bold("#ffffff"), "label": "#b0b0b0",
        "operator": "#b0b0b0", "punctuation": "#8c8c8c",
        "title": bold("#ffffff"), "emphasis": it("#ededed"), "emphasis.strong": bold("#ffffff"),
        "link_uri": "#8fb8de", "link_text": "#ffffff", "text.literal": "#9ece9a", "embedded": "#ededed",
    },
))

THEMES.append(dict(
    name="Qur Light", appearance="light", group="Qur",
    ui=ui_set("#ffffff", "#ffffff", "#f7f7f7", "#ffffff", "#ffffff", "#ffffff", "#e5e5e5", "#c8c8c8",
              "#000000", "#4d4d4d", "#737373", "#ffffff", "#f0f0f0", "#e6e6e6", "#d6d6d6", "#e6e6e6",
              "#000000", "#f2f2f2", "#000000", "#000000", "#ffffff", "#c62828", "#a15c00", "#1b7f3b",
              drawer="#f7f7f7", bar="#f2f2f2", note="#f2f2f2"),
    editor=ed_set("#1a1a1a", "#000000", "#f6f6f6", "#d6d6d6", "#e8e8e8", "#c4c4c4", "#9a9a9a", "#333333",
                  "#d9d9d9", "#ececec"),
    diff=diff("#1b7f3b", "#c62828", "#a15c00", 0.12, 0.25),
    terminal=term(["#000000", "#c62828", "#1b7f3b", "#8a5a00", "#1f5fd1", "#9333b8", "#0e7a86", "#8a8a8a",
                   "#4d4d4d", "#e03b3b", "#22994a", "#a96d00", "#3b78ea", "#ad4ad4", "#1592a0", "#b3b3b3"],
                  "#1a1a1a", "#ffffff", "#000000", "#d6d6d6"),
    accents=["#d9480f", "#0e9f6e", "#2563eb", "#c026d3", "#a16207", "#7c3aed", "#0891b2", "#db2777", "#65a30d"],
    syntax={
        "comment": it("#6b6b6b"), "keyword": bold("#000000"), "function": "#000000",
        "type": "#1f5f99", "constructor": "#1f5f99", "namespace": "#1f5f99",
        "string": "#2e7d32", "string.escape": "#9a5b00", "string.regex": "#9a5b00", "escape": "#9a5b00",
        "number": "#9a5b00", "boolean": "#9a5b00", "constant": "#9a5b00",
        "variable": "#1a1a1a", "variable.builtin": it("#1f5f99"), "variable.parameter": "#333333",
        "property": "#333333", "attribute": "#555555", "tag": bold("#000000"), "label": "#555555",
        "operator": "#4d4d4d", "punctuation": "#6e6e6e",
        "title": bold("#000000"), "emphasis": it("#1a1a1a"), "emphasis.strong": bold("#000000"),
        "link_uri": "#1f5f99", "link_text": "#000000", "text.literal": "#2e7d32", "embedded": "#1a1a1a",
    },
))

# --- One Dark / One Light (atom/atom packages one-*-syntax, one-*-ui) --------
ATOM = dict(author="GitHub Inc. (Atom)", license="MIT", url="https://github.com/atom/atom")


def one(dark):
    if dark:
        m1, m2, m3 = hsl(220, 14, 71), hsl(220, 9, 55), hsl(220, 10, 40)
        h1, h2, h3, h4 = hsl(187, 47, 55), hsl(207, 82, 66), hsl(286, 60, 67), hsl(95, 38, 62)
        h5, h52, h6, h62 = hsl(355, 65, 65), hsl(5, 48, 51), hsl(29, 54, 61), hsl(39, 67, 69)
        bg = hsl(220, 13, 18)
        lvl3, lvl1, border = hsl(220, 13, 15), hsl(220, 13, 24), hsl(220, 13, 10.8)
        uifg = hsl(220, 13, 66)
        sel, cursor = hsl(220, 13, 28), hsl(220, 100, 66)
        gutter = hsl(220, 14, 45)
        ansi = [lvl1, h5, h4, h62, h2, h3, h1, m1, m3, shift_l(h5, .08), shift_l(h4, .08), shift_l(h62, .08),
                shift_l(h2, .08), shift_l(h3, .08), shift_l(h1, .08), "#ffffff"]
        name = "One Dark"
    else:
        m1, m2, m3 = hsl(230, 8, 24), hsl(230, 6, 44), hsl(230, 4, 64)
        h1, h2, h3, h4 = hsl(198, 99, 37), hsl(221, 87, 60), hsl(301, 63, 40), hsl(119, 34, 47)
        h5, h52, h6, h62 = hsl(5, 74, 59), hsl(344, 84, 43), hsl(35, 99, 36), hsl(35, 99, 40)
        bg = hsl(230, 1, 98)
        lvl3, lvl1, border = hsl(230, 1, 92), hsl(230, 1, 100), hsl(230, 1, 86)
        uifg = hsl(230, 1, 26)
        sel, cursor = hsl(230, 1, 90), h2
        gutter = hsl(230, 1, 62)
        ansi = [m1, h5, h4, h62, h2, h3, h1, hsl(230, 1, 86), m3, shift_l(h5, -.08), shift_l(h4, -.08),
                shift_l(h62, -.08), shift_l(h2, -.08), shift_l(h3, -.08), shift_l(h1, -.08), hsl(230, 1, 92)]
        name = "One Light"
    return dict(
        name=name, appearance="dark" if dark else "light", group="Popular",
        credit=dict(ATOM, source=f"packages/{'one-dark' if dark else 'one-light'}-syntax/styles/colors.less and -ui/styles/ui-variables-custom.less; ANSI derived from the syntax hues"),
        ui=ui_set(bg, lvl3, lvl3, bg, lvl3, lvl1, border, shift_l(border, .08 if dark else -.08), uifg, m2, m3,
                  "#ffffff" if not dark else bg, shift_l(lvl3, .03 if dark else -.03), sel, sel,
                  shift_l(sel, -.04 if dark else .04), cursor, lvl1, uifg, cursor, "#ffffff" if not dark else bg,
                  h5, h62, h4),
        editor=ed_set(m1, cursor, alpha(hsl(220, 100, 80) if dark else m1, 0.04 if dark else 0.05), sel,
                      alpha(h62, 0.25), alpha(h62, 0.5), gutter, m1, alpha(m1, 0.15), alpha(m1, 0.10)),
        diff=diff(h4, h5, h62),
        terminal=term(ansi, m1, bg, cursor, sel),
        accents=[h5, h4, h2, h3, h62, h1, h6, h52],
        syntax={
            "comment": it(m3), "keyword": h3, "string": h4, "function": h2, "function.builtin": h1,
            "function.macro": h1, "type": h62, "constructor": h62, "namespace": h62,
            "number": h6, "boolean": h6, "constant": h6, "constant.builtin": h6,
            "variable": m1, "variable.builtin": h5, "variable.parameter": m1, "property": h5,
            "tag": h5, "attribute": h6, "label": h5, "operator": h1, "punctuation": m1,
            "escape": h1, "string.escape": h1, "string.regex": h1, "title": bold(h5),
            "emphasis": it(h3), "emphasis.strong": bold(h6), "link_uri": h1, "link_text": h2,
            "text.literal": h4, "embedded": m1,
        },
    )


THEMES += [one(True), one(False)]

# --- Catppuccin (catppuccin/palette palette.json, docs/style-guide.md) -------
CAT = {
    "latte": {'rosewater': '#dc8a78', 'flamingo': '#dd7878', 'pink': '#ea76cb', 'mauve': '#8839ef', 'red': '#d20f39', 'maroon': '#e64553', 'peach': '#fe640b', 'yellow': '#df8e1d', 'green': '#40a02b', 'teal': '#179299', 'sky': '#04a5e5', 'sapphire': '#209fb5', 'blue': '#1e66f5', 'lavender': '#7287fd', 'text': '#4c4f69', 'subtext1': '#5c5f77', 'subtext0': '#6c6f85', 'overlay2': '#7c7f93', 'overlay1': '#8c8fa1', 'overlay0': '#9ca0b0', 'surface2': '#acb0be', 'surface1': '#bcc0cc', 'surface0': '#ccd0da', 'base': '#eff1f5', 'mantle': '#e6e9ef', 'crust': '#dce0e8'},
    "mocha": {'rosewater': '#f5e0dc', 'flamingo': '#f2cdcd', 'pink': '#f5c2e7', 'mauve': '#cba6f7', 'red': '#f38ba8', 'maroon': '#eba0ac', 'peach': '#fab387', 'yellow': '#f9e2af', 'green': '#a6e3a1', 'teal': '#94e2d5', 'sky': '#89dceb', 'sapphire': '#74c7ec', 'blue': '#89b4fa', 'lavender': '#b4befe', 'text': '#cdd6f4', 'subtext1': '#bac2de', 'subtext0': '#a6adc8', 'overlay2': '#9399b2', 'overlay1': '#7f849c', 'overlay0': '#6c7086', 'surface2': '#585b70', 'surface1': '#45475a', 'surface0': '#313244', 'base': '#1e1e2e', 'mantle': '#181825', 'crust': '#11111b'},
}
CAT_ANSI = {
    "latte": ['#5c5f77', '#d20f39', '#40a02b', '#df8e1d', '#1e66f5', '#ea76cb', '#179299', '#acb0be',
              '#6c6f85', '#de293e', '#49af3d', '#eea02d', '#456eff', '#fe85d8', '#2d9fa8', '#bcc0cc'],
    "mocha": ['#45475a', '#f38ba8', '#a6e3a1', '#f9e2af', '#89b4fa', '#f5c2e7', '#94e2d5', '#a6adc8',
              '#585b70', '#f37799', '#89d88b', '#ebd391', '#74a8fc', '#f2aede', '#6bd7ca', '#bac2de'],
}


def catppuccin(flavor):
    c = CAT[flavor]
    dark = flavor == "mocha"
    return dict(
        name=f"Catppuccin {flavor.capitalize()}", appearance="dark" if dark else "light", group="Popular",
        credit=dict(author="Catppuccin", license="MIT", url="https://github.com/catppuccin/catppuccin",
                    source="catppuccin/palette palette.json (colors, ansiColors); syntax per docs/style-guide.md"),
        ui=ui_set(c["base"], c["crust"], c["mantle"], c["base"], c["mantle"], c["surface0"], c["crust"],
                  c["surface1"], c["text"], c["subtext1"], c["overlay1"], c["base"], c["surface0"],
                  c["surface1"], alpha(c["overlay2"], 0.25), alpha(c["overlay2"], 0.15), c["lavender"],
                  c["surface0"], c["text"], c["mauve"], c["base"], c["red"], c["yellow"], c["green"]),
        editor=ed_set(c["text"], c["rosewater"], alpha(c["text"], 0.10 if dark else 0.06), c["surface2"],
                      alpha(c["teal"], 0.30), alpha(c["red"], 0.40), c["overlay1"], c["lavender"],
                      c["surface1"], c["surface0"]),
        diff=diff(c["green"], c["red"], c["yellow"]),
        terminal=term(CAT_ANSI[flavor], c["text"], c["base"], c["rosewater"], alpha(c["overlay2"], 0.25)),
        accents=[c["peach"], c["green"], c["blue"], c["pink"], c["yellow"], c["mauve"], c["teal"], c["red"]],
        syntax={
            "comment": it(c["overlay2"]), "keyword": c["mauve"], "string": c["green"],
            "escape": c["pink"], "string.escape": c["pink"], "string.regex": c["pink"], "string.special": c["red"],
            "number": c["peach"], "boolean": c["peach"], "constant": c["peach"], "constant.builtin": c["red"],
            "operator": c["sky"], "punctuation": c["overlay2"], "function": c["blue"],
            "function.builtin": c["red"], "function.macro": c["rosewater"], "variable": c["text"],
            "variable.parameter": c["maroon"], "variable.builtin": c["red"], "type": c["yellow"],
            "type.builtin": c["yellow"], "constructor": c["yellow"], "namespace": c["yellow"],
            "property": c["blue"], "attribute": c["yellow"], "tag": c["mauve"], "label": c["sapphire"],
            "title": bold(c["red"]), "emphasis": it(c["text"]), "emphasis.strong": bold(c["text"]),
            "link_uri": c["blue"], "link_text": c["lavender"], "text.literal": c["green"], "embedded": c["text"],
        },
    )


THEMES += [catppuccin("latte"), catppuccin("mocha")]

# --- Tokyo Night (folke/tokyonight.nvim colors/*.lua, extras/*) --------------
TN = dict(author="Folke Lemaitre (tokyonight.nvim)", license="Apache-2.0", url="https://github.com/folke/tokyonight.nvim")


def tokyonight(day):
    if not day:
        p = dict(bg="#1a1b26", bg_dark="#16161e", bg_highlight="#292e42", blue="#7aa2f7", blue1="#2ac3de",
                 blue5="#89ddff", blue6="#b4f9f8", cyan="#7dcfff", comment="#565f89", dark5="#737aa2",
                 fg="#c0caf5", fg_dark="#a9b1d6", fg_gutter="#3b4261", green="#9ece6a", green1="#73daca",
                 magenta="#bb9af7", orange="#ff9e64", purple="#9d7cd8", red="#f7768e", teal="#1abc9c",
                 yellow="#e0af68", border="#15161e", visual="#283457", search="#3d59a1", red1="#db4b4b")
        ansi = ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
                "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5"]
        name, src = "Tokyo Night", "lua/tokyonight/colors/storm.lua + night.lua; extras/kitty/tokyonight_night.conf"
    else:
        p = dict(bg="#e1e2e7", bg_dark="#d0d5e3", bg_highlight="#c4c8da", blue="#2e7de9", blue1="#188092",
                 blue5="#006a83", blue6="#2e5857", cyan="#007197", comment="#848cb5", dark5="#68709a",
                 fg="#3760bf", fg_dark="#6172b0", fg_gutter="#a8aecb", green="#587539", green1="#387068",
                 magenta="#9854f1", orange="#b15c00", purple="#7847bd", red="#f52a65", teal="#118c74",
                 yellow="#8c6c3e", border="#b4b5b9", visual="#b7c1e3", search="#7890dd", red1="#c64343")
        ansi = ["#b4b5b9", "#f52a65", "#587539", "#8c6c3e", "#2e7de9", "#9854f1", "#007197", "#6172b0",
                "#a1a6c5", "#ff4774", "#5c8524", "#a27629", "#358aff", "#a463ff", "#007ea8", "#3760bf"]
        name, src = "Tokyo Night Day", "extras/lua/tokyonight_day.lua; extras/kitty/tokyonight_day.conf"
    dark = not day
    return dict(
        name=name, appearance="light" if day else "dark", group="Popular",
        credit=dict(TN, source=src + "; syntax per lua/tokyonight/groups/{base,treesitter}.lua"),
        ui=ui_set(p["bg"], p["bg_dark"], p["bg_dark"], p["bg"], p["bg_dark"], p["bg_highlight"] if dark else p["bg_dark"],
                  p["border"], p["fg_gutter"], p["fg"], p["fg_dark"], p["dark5"], p["bg_dark"], p["bg_highlight"],
                  p["visual"], p["visual"], shift_l(p["visual"], -.03 if dark else .03), p["blue"], p["bg_highlight"],
                  p["fg"], p["blue"], p["bg_dark"], p["red1"], p["yellow"], p["green"]),
        editor=ed_set(p["fg"], p["fg"], p["bg_highlight"] if dark else alpha(p["bg_highlight"], 0.6), p["fg_gutter"],
                      alpha(p["search"], 0.6), alpha(p["orange"], 0.5), p["fg_gutter"], p["orange"], p["fg_gutter"],
                      alpha(p["fg_gutter"], 0.6)),
        diff=diff(p["green"], p["red"], p["yellow"]),
        terminal=term(ansi, p["fg"], p["bg"], p["fg"], p["visual"]),
        accents=[p["orange"], p["green"], p["blue"], p["magenta"], p["yellow"], p["cyan"], p["red"], p["teal"]],
        syntax={
            "comment": it(p["comment"]), "keyword": it(p["purple"]), "string": p["green"], "function": p["blue"],
            "function.builtin": p["blue1"], "function.macro": p["cyan"], "type": p["blue1"],
            "type.builtin": p["blue1"], "constructor": p["magenta"], "namespace": p["cyan"],
            "constant": p["orange"], "constant.builtin": p["blue1"], "number": p["orange"], "boolean": p["orange"],
            "operator": p["blue5"], "punctuation": p["fg_dark"], "punctuation.delimiter": p["blue5"],
            "punctuation.special": p["blue5"], "variable": p["fg"], "variable.builtin": p["red"],
            "variable.parameter": p["yellow"], "property": p["green1"], "attribute": p["green1"],
            "tag": p["red"], "label": p["blue"], "escape": p["magenta"], "string.escape": p["magenta"],
            "string.regex": p["blue6"], "title": bold(p["blue"]), "emphasis": it(p["fg"]),
            "emphasis.strong": bold(p["fg"]), "link_uri": p["teal"], "link_text": p["teal"],
            "text.literal": p["green"], "embedded": p["fg"],
        },
    )


THEMES += [tokyonight(False), tokyonight(True)]

# --- Gruvbox (morhetz/gruvbox colors/gruvbox.vim) ---------------------------
GB = {'dark0_hard': '#1d2021', 'dark0': '#282828', 'dark0_soft': '#32302f', 'dark1': '#3c3836', 'dark2': '#504945',
      'dark3': '#665c54', 'dark4': '#7c6f64', 'gray': '#928374', 'light0_hard': '#f9f5d7', 'light0': '#fbf1c7',
      'light0_soft': '#f2e5bc', 'light1': '#ebdbb2', 'light2': '#d5c4a1', 'light3': '#bdae93', 'light4': '#a89984',
      'bright_red': '#fb4934', 'bright_green': '#b8bb26', 'bright_yellow': '#fabd2f', 'bright_blue': '#83a598',
      'bright_purple': '#d3869b', 'bright_aqua': '#8ec07c', 'bright_orange': '#fe8019',
      'neutral_red': '#cc241d', 'neutral_green': '#98971a', 'neutral_yellow': '#d79921', 'neutral_blue': '#458588',
      'neutral_purple': '#b16286', 'neutral_aqua': '#689d6a', 'neutral_orange': '#d65d0e',
      'faded_red': '#9d0006', 'faded_green': '#79740e', 'faded_yellow': '#b57614', 'faded_blue': '#076678',
      'faded_purple': '#8f3f71', 'faded_aqua': '#427b58', 'faded_orange': '#af3a03'}


def gruvbox(dark):
    g = GB
    if dark:
        bg0, bg1, bg2, bg3, bg4 = g['dark0'], g['dark1'], g['dark2'], g['dark3'], g['dark4']
        hard = g['dark0_hard']
        fg1, fg4 = g['light1'], g['light4']
        acc = {k: g['bright_' + k] for k in ('red', 'green', 'yellow', 'blue', 'purple', 'aqua', 'orange')}
        ansi = [bg0] + [g['neutral_' + k] for k in ('red', 'green', 'yellow', 'blue', 'purple', 'aqua')] + [fg4,
                g['gray']] + [g['bright_' + k] for k in ('red', 'green', 'yellow', 'blue', 'purple', 'aqua')] + [fg1]
    else:
        bg0, bg1, bg2, bg3, bg4 = g['light0'], g['light1'], g['light2'], g['light3'], g['light4']
        hard = g['light0_soft']
        fg1, fg4 = g['dark1'], g['dark4']
        acc = {k: g['faded_' + k] for k in ('red', 'green', 'yellow', 'blue', 'purple', 'aqua', 'orange')}
        ansi = [bg0] + [g['neutral_' + k] for k in ('red', 'green', 'yellow', 'blue', 'purple', 'aqua')] + [fg4,
                g['gray']] + [g['faded_' + k] for k in ('red', 'green', 'yellow', 'blue', 'purple', 'aqua')] + [fg1]
    return dict(
        name="Gruvbox Dark" if dark else "Gruvbox Light", appearance="dark" if dark else "light", group="Popular",
        credit=dict(author="Pavel Pertsev (morhetz)", license="MIT", url="https://github.com/morhetz/gruvbox",
                    source="colors/gruvbox.vim palette and highlight links; ANSI per the gruvbox terminal palette (neutral/bright|faded)"),
        ui=ui_set(bg0, hard, hard, bg0, hard, bg1, bg1, bg2, fg1, fg4, g['gray'], bg0, bg1, bg2, bg3 if dark else bg2,
                  bg2 if dark else bg1, acc['yellow'], bg1, fg1, acc['yellow'], bg0, acc['red'], acc['yellow'], acc['green']),
        editor=ed_set(fg1, fg1, bg1, bg3, alpha(acc['yellow'], 0.30), alpha(acc['orange'], 0.50), bg4, acc['yellow'],
                      bg2, bg1),
        diff=diff(acc['green'], acc['red'], acc['yellow']),
        terminal=term(ansi, fg1, bg0, fg1, bg3 if dark else bg2),
        accents=[acc['orange'], acc['green'], acc['blue'], acc['purple'], acc['yellow'], acc['aqua'], acc['red']],
        syntax={
            "comment": it(g['gray']), "keyword": acc['red'], "label": acc['red'], "operator": fg1,
            "variable": fg1, "property": acc['blue'], "function": bold(acc['green']), "function.macro": acc['aqua'],
            "namespace": acc['aqua'], "constant": acc['purple'], "number": acc['purple'], "boolean": acc['purple'],
            "type": acc['yellow'], "constructor": acc['yellow'], "string": acc['green'], "escape": acc['orange'],
            "string.escape": acc['orange'], "string.special": acc['orange'], "string.regex": acc['orange'],
            "punctuation": fg4, "tag": acc['aqua'], "attribute": acc['yellow'], "variable.builtin": acc['orange'],
            "variable.parameter": fg1, "title": bold(acc['green']), "emphasis": it(fg1), "emphasis.strong": bold(fg1),
            "link_uri": acc['blue'], "link_text": acc['aqua'], "text.literal": acc['green'], "embedded": fg1,
        },
    )


THEMES += [gruvbox(True), gruvbox(False)]

# --- Nord (nordtheme/nord src/nord.css; nordtheme/visual-studio-code) -------
N = {f"n{i}": c for i, c in enumerate(["#2e3440", "#3b4252", "#434c5e", "#4c566a", "#d8dee9", "#e5e9f0", "#eceff4",
                                       "#8fbcbb", "#88c0d0", "#81a1c1", "#5e81ac", "#bf616a", "#d08770", "#ebcb8b",
                                       "#a3be8c", "#b48ead"])}
THEMES.append(dict(
    name="Nord", appearance="dark", group="Popular",
    credit=dict(author="Sven Greb (Nord)", license="MIT", url="https://github.com/nordtheme/nord",
                source="nordtheme/nord src/nord.css; token and UI mapping per nordtheme/visual-studio-code themes/nord-color-theme.json (MIT)"),
    ui=ui_set(N["n0"], N["n0"], N["n0"], N["n0"], N["n0"], N["n1"], N["n1"], N["n2"], N["n4"], N["n4"], N["n3"],
              N["n0"], N["n1"], N["n2"], alpha(N["n2"], 0.8), alpha(N["n2"], 0.5), N["n8"], N["n1"], N["n4"],
              N["n8"], N["n0"], N["n11"], N["n13"], N["n14"]),
    editor=ed_set(N["n4"], N["n4"], N["n1"], N["n3"], alpha(N["n8"], 0.2), alpha(N["n8"], 0.4), N["n3"], N["n4"],
                  alpha(N["n3"], 0.7), alpha(N["n2"], 0.7)),
    diff=diff(N["n14"], N["n11"], N["n13"]),
    terminal=term([N["n1"], N["n11"], N["n14"], N["n13"], N["n9"], N["n15"], N["n8"], N["n5"],
                   N["n3"], N["n11"], N["n14"], N["n13"], N["n9"], N["n15"], N["n7"], N["n6"]],
                  N["n4"], N["n0"], N["n4"], alpha(N["n2"], 0.8)),
    accents=[N["n12"], N["n14"], N["n8"], N["n15"], N["n13"], N["n11"], N["n7"], N["n10"]],
    syntax={
        "comment": "#616e88", "keyword": N["n9"], "operator": N["n9"], "string": N["n14"], "function": N["n8"],
        "function.builtin": N["n8"], "function.macro": N["n9"], "type": N["n7"], "constructor": N["n7"],
        "namespace": N["n7"], "number": N["n15"], "boolean": N["n9"], "constant": N["n9"],
        "constant.builtin": N["n9"], "escape": N["n13"], "string.escape": N["n13"], "string.regex": N["n13"],
        "punctuation": N["n6"], "variable": N["n4"], "variable.parameter": N["n4"], "variable.builtin": N["n9"],
        "property": N["n4"], "tag": N["n9"], "attribute": N["n7"], "label": N["n9"], "title": bold(N["n8"]),
        "emphasis": it(N["n4"]), "emphasis.strong": bold(N["n4"]), "link_uri": N["n8"], "link_text": N["n8"],
        "text.literal": N["n14"], "embedded": N["n4"],
    },
))

# --- Solarized (altercation/solarized README + vim-colors-solarized) --------
SOL = dict(base03="#002b36", base02="#073642", base01="#586e75", base00="#657b83", base0="#839496",
           base1="#93a1a1", base2="#eee8d5", base3="#fdf6e3", yellow="#b58900", orange="#cb4b16", red="#dc322f",
           magenta="#d33682", violet="#6c71c4", blue="#268bd2", cyan="#2aa198", green="#859900")


def solarized(dark):
    s = SOL
    if dark:
        bg, bg_hl, emph, body, faint = s["base03"], s["base02"], s["base1"], s["base0"], s["base01"]
    else:
        bg, bg_hl, emph, body, faint = s["base3"], s["base2"], s["base01"], s["base00"], s["base1"]
    # Solarized's own 16-color terminal table (README): black=base02, brblack=base03, ...
    ansi = [s["base02"], s["red"], s["green"], s["yellow"], s["blue"], s["magenta"], s["cyan"], s["base2"],
            s["base03"], s["orange"], s["base01"], s["base00"], s["base0"], s["violet"], s["base1"], s["base3"]]
    return dict(
        name="Solarized Dark" if dark else "Solarized Light", appearance="dark" if dark else "light", group="Popular",
        credit=dict(author="Ethan Schoonover", license="MIT", url="https://github.com/altercation/solarized",
                    source="README.md palette table and 16-color mapping; syntax per vim-colors-solarized/colors/solarized.vim"),
        ui=ui_set(bg, bg_hl, bg_hl, bg, bg_hl, bg_hl, shift_l(bg_hl, .04 if dark else -.06), faint, emph, body, faint,
                  bg, shift_l(bg_hl, .03 if dark else -.03), shift_l(bg_hl, .06 if dark else -.06), bg_hl if dark else s["base2"],
                  bg_hl, s["blue"], bg_hl, emph, s["blue"], s["base3"], s["red"], s["yellow"], s["green"]),
        editor=ed_set(body, emph, bg_hl, faint, alpha(s["yellow"], 0.30), alpha(s["yellow"], 0.55), faint, emph,
                      bg_hl, bg_hl),
        diff=diff(s["green"], s["red"], s["yellow"]),
        terminal=term(ansi, body, bg, emph, bg_hl),
        accents=[s["orange"], s["green"], s["blue"], s["magenta"], s["yellow"], s["violet"], s["cyan"], s["red"]],
        syntax={
            "comment": it(faint), "keyword": s["green"], "label": s["green"], "operator": s["green"],
            "string": s["cyan"], "constant": s["cyan"], "number": s["cyan"], "boolean": s["cyan"],
            "function": s["blue"], "variable": body, "property": s["blue"], "variable.parameter": body,
            "type": s["yellow"], "constructor": s["yellow"], "namespace": s["orange"], "function.macro": s["orange"],
            "attribute": s["orange"], "escape": s["red"], "string.escape": s["red"], "string.regex": s["red"],
            "string.special": s["red"], "variable.builtin": s["red"], "constant.builtin": s["red"],
            "punctuation": body, "tag": s["blue"], "title": bold(s["orange"]), "emphasis": it(body),
            "emphasis.strong": bold(emph), "link_uri": s["violet"], "link_text": s["violet"],
            "text.literal": s["cyan"], "embedded": body,
        },
    )


THEMES += [solarized(True), solarized(False)]

# --- GitHub (primer/github-vscode-theme src/theme.js + @primer/primitives 7.10.0)
GH_SCALE = {
    "light": {'gray': ['#f6f8fa', '#eaeef2', '#d0d7de', '#afb8c1', '#8c959f', '#6e7781', '#57606a', '#424a53', '#32383f', '#24292f'], 'blue': ['#ddf4ff', '#b6e3ff', '#80ccff', '#54aeff', '#218bff', '#0969da', '#0550ae', '#033d8b', '#0a3069', '#002155'], 'green': ['#dafbe1', '#aceebb', '#6fdd8b', '#4ac26b', '#2da44e', '#1a7f37', '#116329', '#044f1e', '#003d16', '#002d11'], 'yellow': ['#fff8c5', '#fae17d', '#eac54f', '#d4a72c', '#bf8700', '#9a6700', '#7d4e00', '#633c01', '#4d2d00', '#3b2300'], 'orange': ['#fff1e5', '#ffd8b5', '#ffb77c', '#fb8f44', '#e16f24', '#bc4c00', '#953800', '#762c00', '#5c2200', '#471700'], 'red': ['#ffebe9', '#ffcecb', '#ffaba8', '#ff8182', '#fa4549', '#cf222e', '#a40e26', '#82071e', '#660018', '#4c0014'], 'purple': ['#fbefff', '#ecd8ff', '#d8b9ff', '#c297ff', '#a475f9', '#8250df', '#6639ba', '#512a97', '#3e1f79', '#2e1461'], 'pink': ['#ffeff7', '#ffd3eb', '#ffadda', '#ff80c8', '#e85aad', '#bf3989', '#99286e', '#772057', '#611347', '#4d0336']},
    "dark": {'gray': ['#f0f6fc', '#c9d1d9', '#b1bac4', '#8b949e', '#6e7681', '#484f58', '#30363d', '#21262d', '#161b22', '#0d1117'], 'blue': ['#cae8ff', '#a5d6ff', '#79c0ff', '#58a6ff', '#388bfd', '#1f6feb', '#1158c7', '#0d419d', '#0c2d6b', '#051d4d'], 'green': ['#aff5b4', '#7ee787', '#56d364', '#3fb950', '#2ea043', '#238636', '#196c2e', '#0f5323', '#033a16', '#04260f'], 'yellow': ['#f8e3a1', '#f2cc60', '#e3b341', '#d29922', '#bb8009', '#9e6a03', '#845306', '#693e00', '#4b2900', '#341a00'], 'orange': ['#ffdfb6', '#ffc680', '#ffa657', '#f0883e', '#db6d28', '#bd561d', '#9b4215', '#762d0a', '#5a1e02', '#3d1300'], 'red': ['#ffdcd7', '#ffc1ba', '#ffa198', '#ff7b72', '#f85149', '#da3633', '#b62324', '#8e1519', '#67060c', '#490202'], 'purple': ['#eddeff', '#e2c5ff', '#d2a8ff', '#bc8cff', '#a371f7', '#8957e5', '#6e40c9', '#553098', '#3c1e70', '#271052'], 'pink': ['#ffdaec', '#ffbedd', '#ff9bce', '#f778ba', '#db61a2', '#bf4b8a', '#9e3670', '#7d2457', '#5e103e', '#42062a']},
}
GH_ANSI = {
    "light": ['#24292f', '#cf222e', '#116329', '#4d2d00', '#0969da', '#8250df', '#1b7c83', '#6e7781',
              '#57606a', '#a40e26', '#1a7f37', '#633c01', '#218bff', '#a475f9', '#3192aa', '#8c959f'],
    "dark": ['#484f58', '#ff7b72', '#3fb950', '#d29922', '#58a6ff', '#bc8cff', '#39c5cf', '#b1bac4',
             '#6e7681', '#ffa198', '#56d364', '#e3b341', '#79c0ff', '#d2a8ff', '#56d4dd', '#ffffff'],
}


def github(dark):
    k = "dark" if dark else "light"
    sc = GH_SCALE[k]
    ld = lambda light, dark_: dark_ if dark else light   # theme.js lightDark(light, dark)
    if dark:
        canvas, inset, subtle, overlay = "#0d1117", "#010409", "#161b22", "#161b22"
        fg, muted, fsubtle, border, bmuted = "#e6edf3", "#7d8590", "#6e7681", "#30363d", "#21262d"
        accent_fg = "#2f81f7"
    else:
        canvas, inset, subtle, overlay = "#ffffff", "#f6f8fa", "#f6f8fa", "#ffffff"
        fg, muted, fsubtle, border, bmuted = "#1f2328", "#656d76", "#6e7781", "#d0d7de", "#d8dee4"
        accent_fg = "#0969da"
    return dict(
        name="GitHub Dark" if dark else "GitHub Light", appearance=k, group="Popular",
        credit=dict(author="GitHub Inc. (Primer)", license="MIT", url="https://github.com/primer/github-vscode-theme",
                    source="src/theme.js + src/colors.js overrides; colors from @primer/primitives@7.10.0 dist/json/colors/{light,dark}.json (MIT)"),
        ui=ui_set(canvas, inset, inset, canvas, inset, overlay, border, ld(sc['gray'][3], sc['gray'][5]), fg, muted, fsubtle,
                  "#ffffff", subtle, ld(sc['gray'][1], sc['gray'][7]), alpha(ld(sc['blue'][3], sc['blue'][4]), 0.4),
                  alpha(ld(sc['blue'][3], sc['blue'][4]), 0.2), accent_fg, subtle, fg, ld("#1f883d", sc['green'][5]),
                  "#ffffff", ld(sc['red'][5], sc['red'][3]), ld(sc['yellow'][5], sc['yellow'][3]), ld(sc['green'][5], sc['green'][3])),
        editor=ed_set(fg, accent_fg, ld(sc['gray'][1], sc['gray'][8]), alpha(sc['green'][3], 0.25),
                      alpha(ld(sc['yellow'][1], sc['yellow'][1]), 0.5 if not dark else 0.25), alpha(ld(sc['yellow'][2], sc['yellow'][3]), 0.6),
                      sc['gray'][4], fg, ld(sc['gray'][3], sc['gray'][5]), ld(sc['gray'][2], sc['gray'][7])),
        diff=diff(ld(sc['green'][5], sc['green'][3]), ld(sc['red'][5], sc['red'][3]), ld(sc['yellow'][5], sc['yellow'][3])),
        terminal=term(GH_ANSI[k], fg, canvas, accent_fg, alpha(ld(sc['blue'][3], sc['blue'][4]), 0.4)),
        accents=[ld(sc['orange'][5], sc['orange'][3]), ld(sc['green'][5], sc['green'][3]), ld(sc['blue'][5], sc['blue'][3]),
                 ld(sc['pink'][5], sc['pink'][3]), ld(sc['yellow'][5], sc['yellow'][3]), ld(sc['purple'][5], sc['purple'][3]),
                 ld(sc['red'][5], sc['red'][3])],
        syntax={
            "comment": ld(sc['gray'][5], sc['gray'][3]), "keyword": ld(sc['red'][5], sc['red'][3]),
            "constant": ld(sc['blue'][6], sc['blue'][2]), "number": ld(sc['blue'][6], sc['blue'][2]),
            "boolean": ld(sc['blue'][6], sc['blue'][2]), "property": ld(sc['blue'][6], sc['blue'][2]),
            "type": ld(sc['orange'][6], sc['orange'][2]), "constructor": ld(sc['orange'][6], sc['orange'][2]),
            "namespace": ld(sc['orange'][6], sc['orange'][2]), "variable": fg, "variable.parameter": fg,
            "variable.builtin": ld(sc['blue'][6], sc['blue'][2]), "function": ld(sc['purple'][5], sc['purple'][2]),
            "tag": ld(sc['green'][6], sc['green'][1]), "attribute": ld(sc['blue'][6], sc['blue'][2]),
            "string": ld(sc['blue'][8], sc['blue'][1]), "string.regex": ld(sc['blue'][8], sc['blue'][1]),
            "escape": ld(sc['green'][6], sc['green'][1]), "string.escape": ld(sc['green'][6], sc['green'][1]),
            "operator": ld(sc['red'][5], sc['red'][3]), "punctuation": fg, "label": ld(sc['blue'][6], sc['blue'][2]),
            "title": bold(ld(sc['blue'][6], sc['blue'][2])), "emphasis": it(fg), "emphasis.strong": bold(fg),
            "link_uri": ld(sc['blue'][8], sc['blue'][1]), "link_text": ld(sc['blue'][6], sc['blue'][2]),
            "text.literal": ld(sc['blue'][8], sc['blue'][1]), "embedded": fg,
        },
    )


THEMES += [github(True), github(False)]

# --- High contrast (Qur-designed, monochrome bases, 7:1 text) ---------------
THEMES.append(dict(
    name="High Contrast Dark", appearance="dark", group="Accessibility", high_contrast=True,
    ui=ui_set("#000000", "#000000", "#000000", "#000000", "#000000", "#000000", "#8a8a8a", "#ffffff",
              "#ffffff", "#e6e6e6", "#bdbdbd", "#000000", "#1f1f1f", "#2e2e2e", "#264f78", "#1f3a57",
              "#f5c400", "#000000", "#ffffff", "#ffffff", "#000000", "#ff8080", "#ffd75f", "#7cfc7c"),
    editor=ed_set("#ffffff", "#ffffff", "#141414", "#5a5a5a", "#4d3d00", "#806600", "#bdbdbd", "#ffffff",
                  "#5a5a5a", "#3d3d3d"),
    diff=diff("#7cfc7c", "#ff8080", "#ffd75f", 0.2, 0.4),
    terminal=term(["#000000", "#ff8080", "#7cfc7c", "#ffd75f", "#82b4ff", "#e39cff", "#6ff0f0", "#e6e6e6",
                   "#8a8a8a", "#ffa3a3", "#a6ffa6", "#ffe48f", "#a8ccff", "#f0c2ff", "#a3ffff", "#ffffff"],
                  "#ffffff", "#000000", "#ffffff", "#264f78"),
    accents=["#ff9a5c", "#5cf29c", "#7ab0ff", "#ff7ad9", "#ffe066", "#c49cff", "#5ee8f0"],
    syntax={
        "comment": it("#bdbdbd"), "keyword": bold("#ffffff"), "function": "#ffe066", "type": "#7ad6ff",
        "constructor": "#7ad6ff", "namespace": "#7ad6ff", "string": "#9cff9c", "number": "#ffb38a",
        "boolean": "#ffb38a", "constant": "#ffb38a", "escape": "#ffb38a", "string.escape": "#ffb38a",
        "string.regex": "#ffb38a", "variable": "#ffffff", "variable.builtin": "#7ad6ff", "property": "#e6e6e6",
        "attribute": "#e6e6e6", "tag": bold("#ffffff"), "label": "#e6e6e6", "operator": "#ffffff",
        "punctuation": "#e6e6e6", "title": bold("#ffffff"), "emphasis": it("#ffffff"),
        "emphasis.strong": bold("#ffffff"), "link_uri": "#7ad6ff", "link_text": "#ffe066",
        "text.literal": "#9cff9c", "embedded": "#ffffff",
    },
))

THEMES.append(dict(
    name="High Contrast Light", appearance="light", group="Accessibility", high_contrast=True,
    ui=ui_set("#ffffff", "#ffffff", "#ffffff", "#ffffff", "#ffffff", "#ffffff", "#6e6e6e", "#000000",
              "#000000", "#1a1a1a", "#4d4d4d", "#ffffff", "#ebebeb", "#dcdcdc", "#add6ff", "#d6e9ff",
              "#0f4a85", "#ffffff", "#000000", "#000000", "#ffffff", "#b00020", "#7a4f00", "#0f6b2e"),
    editor=ed_set("#000000", "#000000", "#f0f0f0", "#b5b5b5", "#ffe58a", "#ffc94d", "#4d4d4d", "#000000",
                  "#b5b5b5", "#d6d6d6"),
    diff=diff("#0f6b2e", "#b00020", "#7a4f00", 0.15, 0.3),
    terminal=term(["#000000", "#b00020", "#0f6b2e", "#7a4f00", "#0b3d91", "#7b1fa2", "#00595e", "#4d4d4d",
                   "#333333", "#c62828", "#1b5e20", "#8a5a00", "#0d47a1", "#8e24aa", "#00695c", "#6e6e6e"],
                  "#000000", "#ffffff", "#000000", "#add6ff"),
    accents=["#b33c00", "#00753a", "#0b4fc4", "#b0177d", "#7a5c00", "#6a1fd1", "#007a87"],
    syntax={
        "comment": it("#4d4d4d"), "keyword": bold("#000000"), "function": "#0b3d91", "type": "#5c2d91",
        "constructor": "#5c2d91", "namespace": "#5c2d91", "string": "#0f5c2a", "number": "#8a3200",
        "boolean": "#8a3200", "constant": "#8a3200", "escape": "#8a3200", "string.escape": "#8a3200",
        "string.regex": "#8a3200", "variable": "#000000", "variable.builtin": "#0b3d91", "property": "#1a1a1a",
        "attribute": "#1a1a1a", "tag": bold("#000000"), "label": "#1a1a1a", "operator": "#000000",
        "punctuation": "#1a1a1a", "title": bold("#000000"), "emphasis": it("#000000"),
        "emphasis.strong": bold("#000000"), "link_uri": "#0b3d91", "link_text": "#0b3d91",
        "text.literal": "#0f5c2a", "embedded": "#000000",
    },
))


def main():
    os.makedirs(OUT, exist_ok=True)
    report = []
    for t in THEMES:
        build(t)
        write(t)
        report.append((t["name"], t["accent_min_de"], t["notes"]))
    for name, de, notes in report:
        print(f"{name:22s} accents min dE {de:5.1f}  adjustments: {len(notes)}")
        for n in notes:
            print("    ", n)


if __name__ == "__main__":
    main()
