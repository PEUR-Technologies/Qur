//! Assets compiled into the binary: brand marks, UI icons, the Inter UI font.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! embed {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/", $path)))),*]
    };
}

static FILES: &[(&str, &[u8])] = embed![
    "brand/svg/qur-symbol-black.svg",
    "brand/svg/qur-symbol-white.svg",
    "brand/svg/qur-symbol-small-black.svg",
    "brand/svg/qur-symbol-small-white.svg",
    "brand/svg/qur-horizontal-black.svg",
    "brand/svg/qur-horizontal-white.svg",
    "icons/chevron-right.svg",
    "icons/chevron-down.svg",
    "icons/close.svg",
    "icons/check.svg",
    "icons/plus.svg",
];

/// Inter (SIL OFL 1.1, assets/fonts/Inter-LICENSE.txt), subset to Latin,
/// punctuation, arrows and the macOS key symbols.
pub static FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
];

pub const UI_FONT: &str = "Inter";

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

pub mod icon {
    pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
    pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
    pub const CLOSE: &str = "icons/close.svg";
    pub const CHECK: &str = "icons/check.svg";
    pub const PLUS: &str = "icons/plus.svg";
}
