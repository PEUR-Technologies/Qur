//! The theme TOML schema. Every field is optional in the file; whatever is
//! missing comes from the fallback Qur theme (Qur Light or Qur Dark, chosen
//! by background luminance). Qur Light/Dark themselves must be complete.

use crate::{Accent, Credit, DiffColors, EditorColors, Rgba, Style, TerminalColors, UiColors};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Strongest heat tint: the accent blended into the sidebar at 31%...
pub(crate) const MAX_HEAT: f32 = 0.31;
/// ...unless that would drop row text below 4.5:1, in which case the
/// strongest level is the highest strength that keeps it. Built-in themes
/// guarantee at least this much (tools/gen_themes.py), so heat stays visible.
#[cfg(test)]
pub(crate) const MIN_HEAT: f32 = 0.15;
/// The four heat levels as fractions of the strongest one.
const HEAT_LEVELS: [f32; 4] = [0.32, 0.55, 0.77, 1.0];
/// Alpha of an accent's glow (pulse/outline halo).
pub(crate) const GLOW_ALPHA: u8 = 0x59; // 35%

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTheme {
    pub name: Option<String>,
    pub appearance: Option<String>,
    pub group: Option<String>,
    pub high_contrast: Option<bool>,
    pub accents: Option<Vec<Hex>>,
    pub credit: Option<RawCredit>,
    #[serde(default)]
    pub ui: RawUi,
    #[serde(default)]
    pub editor: RawEditor,
    #[serde(default)]
    pub diff: RawDiff,
    #[serde(default)]
    pub terminal: RawTerminal,
    #[serde(default)]
    pub syntax: BTreeMap<String, RawStyle>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCredit {
    pub author: String,
    pub license: String,
    pub url: String,
}

impl RawCredit {
    /// Themes are parsed once per process, so leaking these few strings is fine.
    pub fn into_credit(self) -> Credit {
        Credit {
            author: Box::leak(self.author.into_boxed_str()),
            license: Box::leak(self.license.into_boxed_str()),
            url: Box::leak(self.url.into_boxed_str()),
        }
    }
}

/// A `#rrggbb[aa]` string in TOML.
#[derive(Clone, Copy)]
pub(crate) struct Hex(pub Rgba);

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgba::parse(&s).map(Hex).ok_or_else(|| serde::de::Error::custom(format!("bad color {s:?}")))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum RawStyle {
    Color(Hex),
    Full {
        color: Option<Hex>,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        italic: bool,
    },
}

impl RawStyle {
    pub fn style(&self) -> Style {
        match *self {
            RawStyle::Color(c) => Style { color: Some(c.0), bold: false, italic: false },
            RawStyle::Full { color, bold, italic } => Style { color: color.map(|c| c.0), bold, italic },
        }
    }
}

/// Defines a `Raw*` struct of optional colors plus `resolve`, which fills
/// gaps from a base (fallback) set, or reports the missing names.
macro_rules! token_set {
    ($raw:ident => $full:ident { $($field:ident),* $(,)? }) => {
        #[derive(Deserialize, Default)]
        #[serde(deny_unknown_fields)]
        pub(crate) struct $raw {
            $(pub $field: Option<Hex>,)*
        }

        impl $raw {
            pub fn resolve(&self, base: Option<&$full>, missing: &mut Vec<String>) -> $full {
                $full {
                    $($field: match (self.$field, base) {
                        (Some(c), _) => c.0,
                        (None, Some(b)) => b.$field,
                        (None, None) => {
                            missing.push(concat!(stringify!($raw), ".", stringify!($field)).into());
                            Rgba(0)
                        }
                    },)*
                }
            }
        }
    };
}

token_set!(RawUi => UiColors {
    window_bg, top_bar_bg, sidebar_bg, editor_bg, panel_bg, drawer_bg, overlay_bg, border,
    border_strong, text, text_secondary, text_muted, text_on_accent, hover_bg, active_bg,
    selection, selection_inactive, focus_ring, button_bg, button_text, button_primary_bg,
    button_primary_text, permission_bar_bg, notification_bg, error, warning, success,
});

token_set!(RawEditor => EditorColors {
    text, cursor, current_line, bracket_match, search_match, search_match_active, gutter_text,
    gutter_text_active, whitespace, indent_guide,
});

token_set!(RawDiff => DiffColors {
    added_line, removed_line, modified_line, added_word, removed_word, added_text, removed_text,
});

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTerminal {
    pub ansi: Option<Vec<Hex>>,
    pub foreground: Option<Hex>,
    pub background: Option<Hex>,
    pub cursor: Option<Hex>,
    pub selection: Option<Hex>,
}

impl RawTerminal {
    pub fn resolve(&self, base: Option<&TerminalColors>, missing: &mut Vec<String>) -> TerminalColors {
        let ansi = match (&self.ansi, base) {
            (Some(v), _) if v.len() == 16 => std::array::from_fn(|i| v[i].0),
            (Some(v), _) => {
                missing.push(format!("terminal.ansi (has {} colors, needs 16)", v.len()));
                base.map(|b| b.ansi).unwrap_or([Rgba(0); 16])
            }
            (None, Some(b)) => b.ansi,
            (None, None) => {
                missing.push("terminal.ansi".into());
                [Rgba(0); 16]
            }
        };
        let mut pick = |v: Option<Hex>, b: Option<Rgba>, name: &str| match (v, b) {
            (Some(c), _) => c.0,
            (None, Some(b)) => b,
            (None, None) => {
                missing.push(format!("terminal.{name}"));
                Rgba(0)
            }
        };
        TerminalColors {
            ansi,
            foreground: pick(self.foreground, base.map(|b| b.foreground), "foreground"),
            background: pick(self.background, base.map(|b| b.background), "background"),
            cursor: pick(self.cursor, base.map(|b| b.cursor), "cursor"),
            selection: pick(self.selection, base.map(|b| b.selection), "selection"),
        }
    }
}

/// Six accents: the file's list, then the base theme's slots for any gaps.
/// Glow and heat tints are derived against this theme's sidebar.
pub(crate) fn accents(
    raw: Option<&[Hex]>,
    base: Option<&[Accent; 6]>,
    sidebar: Rgba,
    text: Rgba,
    missing: &mut Vec<String>,
) -> [Accent; 6] {
    std::array::from_fn(|i| {
        let color = match (raw.and_then(|r| r.get(i)), base) {
            (Some(c), _) => c.0,
            (None, Some(b)) => b[i].base,
            (None, None) => {
                missing.push(format!("accents[{i}]"));
                Rgba(0)
            }
        };
        derive_accent(color, sidebar, text)
    })
}

pub(crate) fn derive_accent(base: Rgba, sidebar: Rgba, text: Rgba) -> Accent {
    let tint = |s: f32| crate::blend(base.with_alpha((s * 255.0).round() as u8), sidebar);
    let readable = |s: f32| crate::contrast_ratio(text, tint(s)) >= 4.5;
    let mut strongest = MAX_HEAT;
    while strongest > 0.0 && !readable(strongest) {
        strongest -= 0.01;
    }
    Accent {
        base,
        glow: base.with_alpha(GLOW_ALPHA),
        heat: HEAT_LEVELS.map(|l| tint(l * strongest.max(0.0))),
    }
}
