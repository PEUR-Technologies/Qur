//! Color math: hex parsing, alpha blending, WCAG contrast, CIEDE2000.

use crate::Rgba;

impl Rgba {
    pub const fn rgb(hex: u32) -> Rgba {
        Rgba((hex << 8) | 0xff)
    }

    /// `#rgb`, `#rrggbb` or `#rrggbbaa`.
    pub fn parse(s: &str) -> Option<Rgba> {
        let h = s.strip_prefix('#')?;
        let v = u32::from_str_radix(h, 16).ok()?;
        match h.len() {
            3 => {
                let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
                Some(Rgba((r * 0x11) << 24 | (g * 0x11) << 16 | (b * 0x11) << 8 | 0xff))
            }
            6 => Some(Rgba(v << 8 | 0xff)),
            8 => Some(Rgba(v)),
            _ => None,
        }
    }

    pub const fn with_alpha(self, a: u8) -> Rgba {
        Rgba((self.0 & 0xffff_ff00) | a as u32)
    }

    /// Components as 0..=1 floats (for GPUI's `Rgba`).
    pub fn to_f32(self) -> [f32; 4] {
        [self.r(), self.g(), self.b(), self.a()].map(|c| c as f32 / 255.0)
    }

    /// WCAG relative luminance of the color's RGB (alpha ignored).
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let v = c as f32 / 255.0;
            if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.r()) + 0.7152 * lin(self.g()) + 0.0722 * lin(self.b())
    }
}

impl std::fmt::Display for Rgba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.a() == 0xff {
            write!(f, "#{:06x}", self.0 >> 8)
        } else {
            write!(f, "#{:08x}", self.0)
        }
    }
}

/// `fg` (straight alpha) composited over `bg`; result has `bg`'s alpha.
pub fn blend(fg: Rgba, bg: Rgba) -> Rgba {
    let a = fg.a() as f32 / 255.0;
    let mix = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u32;
    Rgba(mix(fg.r(), bg.r()) << 24 | mix(fg.g(), bg.g()) << 16 | mix(fg.b(), bg.b()) << 8 | bg.a() as u32)
}

pub fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (a.luminance(), b.luminance());
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// sRGB -> CIE L*a*b* (D65).
fn lab(c: Rgba) -> (f64, f64, f64) {
    let lin = |c: u8| {
        let v = c as f64 / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = (0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

pub fn delta_e(a: Rgba, b: Rgba) -> f32 {
    ciede2000(lab(a), lab(b)) as f32
}

/// CIEDE2000 (Sharma, Wu, Dalal 2005), kL = kC = kH = 1.
pub(crate) fn ciede2000((l1, a1, b1): (f64, f64, f64), (l2, a2, b2): (f64, f64, f64)) -> f64 {
    let pow7 = |x: f64| x.powi(7);
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let g = 0.5 * (1.0 - (pow7(c_bar) / (pow7(c_bar) + pow7(25.0))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| if a == 0.0 && b == 0.0 { 0.0 } else { b.atan2(a).to_degrees().rem_euclid(360.0) };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));

    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dh_big = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).to_radians().sin();

    let l_bar = (l1 + l2) / 2.0;
    let c_bar_p = (c1p + c2p) / 2.0;
    let h_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let cos = |d: f64| d.to_radians().cos();
    let t = 1.0 - 0.17 * cos(h_bar - 30.0) + 0.24 * cos(2.0 * h_bar) + 0.32 * cos(3.0 * h_bar + 6.0)
        - 0.20 * cos(4.0 * h_bar - 63.0);
    let d_theta = 30.0 * (-((h_bar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (pow7(c_bar_p) / (pow7(c_bar_p) + pow7(25.0))).sqrt();
    let sl = 1.0 + 0.015 * (l_bar - 50.0).powi(2) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * c_bar_p;
    let sh = 1.0 + 0.015 * c_bar_p * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dh_big / sh).powi(2) + rt * (dc / sc) * (dh_big / sh)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        assert_eq!(Rgba::parse("#ff7a45"), Some(Rgba(0xff7a45ff)));
        assert_eq!(Rgba::parse("#ff7a4580"), Some(Rgba(0xff7a4580)));
        assert_eq!(Rgba::parse("#fff"), Some(Rgba(0xffffffff)));
        assert_eq!(Rgba::parse("ff7a45"), None);
        assert_eq!(Rgba::parse("#ff7a4"), None);
        assert_eq!(Rgba(0xff7a45ff).to_string(), "#ff7a45");
        assert_eq!(Rgba(0xff7a4580).to_string(), "#ff7a4580");
    }

    #[test]
    fn blend_alpha() {
        let half_white = Rgba(0xffffff80);
        assert_eq!(blend(half_white, Rgba::rgb(0x000000)), Rgba::rgb(0x808080));
        assert_eq!(blend(Rgba::rgb(0x123456), Rgba::rgb(0xffffff)), Rgba::rgb(0x123456));
        assert_eq!(blend(Rgba(0x12345600), Rgba::rgb(0xabcdef)), Rgba::rgb(0xabcdef));
    }

    #[test]
    fn wcag_contrast() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(contrast_ratio(Rgba::rgb(0x000000), Rgba::rgb(0xffffff)), 21.0));
        assert!(close(contrast_ratio(Rgba::rgb(0xffffff), Rgba::rgb(0xffffff)), 1.0));
        // WebAIM reference: #767676 on white = 4.54:1.
        assert!(close(contrast_ratio(Rgba::rgb(0x767676), Rgba::rgb(0xffffff)), 4.54));
    }

    /// Sharma, Wu & Dalal (2005) test data, a selection of the 34 pairs.
    #[test]
    fn ciede2000_reference_pairs() {
        type Lab = (f64, f64, f64);
        let pairs: [(Lab, Lab, f64); 12] = [
            ((50.0, 2.6772, -79.7751), (50.0, 0.0, -82.7485), 2.0425),
            ((50.0, 3.1571, -77.2803), (50.0, 0.0, -82.7485), 2.8615),
            ((50.0, 2.8361, -74.0200), (50.0, 0.0, -82.7485), 3.4412),
            ((50.0, -1.3802, -84.2814), (50.0, 0.0, -82.7485), 1.0),
            ((50.0, 0.0, 0.0), (50.0, -1.0, 2.0), 2.3669),
            ((50.0, 2.49, -0.001), (50.0, -2.49, 0.0009), 7.1792),
            ((50.0, 2.5, 0.0), (73.0, 25.0, -18.0), 27.1492),
            ((50.0, 2.5, 0.0), (50.0, 3.2592, 0.3350), 1.0),
            ((60.2574, -34.0099, 36.2677), (60.4626, -34.1751, 39.4387), 1.2644),
            ((22.7233, 20.0904, -46.6940), (23.0331, 14.9730, -42.5619), 2.0373),
            ((90.8027, -2.0831, 1.4410), (91.1528, -1.6435, 0.0447), 1.4441),
            ((2.0776, 0.0795, -1.1350), (0.9033, -0.0636, -0.5514), 0.9082),
        ];
        for (a, b, want) in pairs {
            let got = ciede2000(a, b);
            assert!((got - want).abs() < 1e-4, "{a:?} {b:?}: got {got}, want {want}");
        }
    }

    #[test]
    fn delta_e_on_srgb() {
        assert!(delta_e(Rgba::rgb(0xff0000), Rgba::rgb(0xff0000)) < 1e-6);
        assert!(delta_e(Rgba::rgb(0xff0000), Rgba::rgb(0x00ff00)) > 50.0);
    }
}
