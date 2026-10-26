//! Files pasted or dropped into a prompt, and what Qur shows of a file that
//! isn't text (the file viewer). Images go to the agent as images; any
//! other file as a link to it, which the agent reads itself.
//!
//! Qur decodes the images it draws itself (`thumbnail`): GPUI keeps every
//! `gpui::Image` handed to `img()` decoded at full size until told to drop
//! it, and a 1600×3065 screenshot is 20 MB of pixels. An `Arc<RenderImage>`
//! skips that cache, so Qur decodes at about the size it's drawn.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The API refuses an image over 5 MB once base64-encoded: about 3.75 MB of
/// bytes. Bigger ones are scaled down before they're sent.
const MAX_SEND: usize = 3_700_000;
/// Scaled down to this on the longest side (the API shrinks larger ones anyway).
const MAX_SIDE: u32 = 2048;
/// An image file bigger than this is attached as a plain file link.
const MAX_READ: u64 = 64 << 20;
/// The box an attachment's thumbnail is decoded to: twice the largest it's
/// drawn (under a sent message, at most 320 × 96).
pub const THUMB: (u32, u32) = (640, 192);

/// Something clicked to look at: a file (an image or binary opens in the
/// viewer, text in the code modal), or an image that only lives in memory
/// (a pasted screenshot).
#[derive(Clone)]
pub enum Preview {
    File(PathBuf),
    Image { label: gpui::SharedString, image: Arc<gpui::Image> },
}

/// The format GPUI can draw for a file's extension, for the viewer.
pub fn viewable(path: &Path) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat as F;
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => F::Png,
        "jpg" | "jpeg" => F::Jpeg,
        "gif" => F::Gif,
        "webp" => F::Webp,
        "bmp" => F::Bmp,
        "tif" | "tiff" => F::Tiff,
        "ico" => F::Ico,
        "svg" => F::Svg,
        _ => return None,
    })
}

/// An image file the agent can be sent as an image (SVG and icons go as files).
pub fn sendable_path(path: &Path) -> bool {
    use gpui::ImageFormat as F;
    matches!(viewable(path), Some(F::Png | F::Jpeg | F::Gif | F::Webp | F::Bmp | F::Tiff))
        && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_READ)
}

/// Image bytes as the agent takes them: PNG, JPEG, GIF and WebP as they are;
/// TIFF and BMP (some apps copy those) as PNG; anything over the size limit
/// scaled down, as JPEG if PNG is still too big. None if it doesn't decode.
/// Slow for big images: call it off the main thread.
pub fn sendable_image(bytes: Vec<u8>) -> Option<Arc<gpui::Image>> {
    use image::ImageFormat as I;
    let format = image::guess_format(&bytes).ok()?;
    let gpui_format = match format {
        I::Png => gpui::ImageFormat::Png,
        I::Jpeg => gpui::ImageFormat::Jpeg,
        I::Gif => gpui::ImageFormat::Gif,
        I::WebP => gpui::ImageFormat::Webp,
        _ => gpui::ImageFormat::Bmp,
    };
    if gpui_format != gpui::ImageFormat::Bmp && bytes.len() <= MAX_SEND {
        return Some(Arc::new(gpui::Image::from_bytes(gpui_format, bytes)));
    }
    let mut img = image::load_from_memory_with_format(&bytes, format).ok()?;
    if bytes.len() > MAX_SEND && img.width().max(img.height()) > MAX_SIDE {
        img = img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle);
    }
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), I::Png).ok()?;
    if out.len() <= MAX_SEND {
        return Some(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, out)));
    }
    out.clear();
    image::DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut Cursor::new(&mut out), I::Jpeg).ok()?;
    Some(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Jpeg, out)))
}

/// The copied image behind copied text, when the text is only a stand-in.
/// GPUI's clipboard read returns a copied string before a copied image, and
/// "Copy Image" in a browser puts the image's address (or an empty string)
/// beside the image. So when `text`, what GPUI read, is blank or just a URL
/// and the pasteboard also holds a PNG, JPEG or TIFF, that image's bytes.
/// Any other text stays a text paste.
pub fn image_behind_text(text: &str) -> Option<Vec<u8>> {
    if !stands_in_for_image(text) {
        return None;
    }
    pasteboard_image(true)
}

/// Whether `image_behind_text` would find an image, without reading it.
pub fn has_image_behind_text(text: &str) -> bool {
    stands_in_for_image(text) && pasteboard_image(false).is_some()
}

/// Blank, or a single URL (what browsers copy beside an image).
fn stands_in_for_image(text: &str) -> bool {
    let t = text.trim();
    t.is_empty() || (!t.contains(char::is_whitespace) && ["http://", "https://", "file://", "data:", "blob:"].iter().any(|p| t.starts_with(p)))
}

/// The general pasteboard's image, if it has one: PNG first, then JPEG,
/// then TIFF (big; `sendable_image` turns it into PNG). With `read` false,
/// only whether there is one (`Some` of nothing).
#[cfg(target_os = "macos")]
fn pasteboard_image(read: bool) -> Option<Vec<u8>> {
    image_in(&objc2_app_kit::NSPasteboard::generalPasteboard(), read)
}

#[cfg(target_os = "macos")]
fn image_in(pb: &objc2_app_kit::NSPasteboard, read: bool) -> Option<Vec<u8>> {
    use objc2_app_kit::{NSPasteboardTypePNG, NSPasteboardTypeTIFF};
    use objc2_foundation::NSString;
    let types = pb.types()?;
    let jpeg = NSString::from_str("public.jpeg");
    let wanted: [&NSString; 3] = unsafe { [NSPasteboardTypePNG, &jpeg, NSPasteboardTypeTIFF] };
    let mut present = wanted.into_iter().filter(|t| types.containsObject(t));
    if !read {
        return present.next().map(|_| Vec::new());
    }
    present.find_map(|t| pb.dataForType(t).map(|d| d.to_vec()).filter(|b| !b.is_empty()))
}

#[cfg(not(target_os = "macos"))]
fn pasteboard_image(_read: bool) -> Option<Vec<u8>> {
    None
}

/// Image bytes decoded to fit `max_w` × `max_h` pixels (never scaled up),
/// turned upright, in GPUI's BGRA order. Passed to `img()`, it's drawn
/// without GPUI's asset cache; `cx.drop_image` frees its texture once it's
/// gone for good. None if it doesn't decode. Decoding holds the full image
/// for a moment: call it off the main thread.
pub fn thumbnail(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Arc<gpui::RenderImage>> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    let (max_w, max_h) = (max_w.max(1), max_h.max(1));
    let big = u64::from(img.width()) * u64::from(img.height()) > 1 << 20;
    if img.width() > max_w || img.height() > max_h {
        img = img.thumbnail(max_w, max_h);
    }
    let mut buf = img.into_rgba8();
    for px in buf.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let out = Arc::new(gpui::RenderImage::new(vec![image::Frame::new(buf)]));
    if big {
        trim_memory();
    }
    Some(out)
}

/// Hand freed memory back to the system. A decoded image is tens of MB for
/// a moment; macOS's allocator keeps such freed blocks in a cache that still
/// counts toward the app's footprint, so this is called after a big decode
/// (and once the image viewer closes).
pub fn trim_memory() {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means every zone; a goal of 0 means as much as possible.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    }
}

/// Where an image that only lives in memory (pasted, or sent by the agent)
/// is saved so it opens as a file: `qur-<kind>-<ms>-<n>.<ext>` in the temp
/// folder.
pub fn temp_image_path(kind: &str, format: gpui::ImageFormat) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    std::env::temp_dir().join(format!("qur-{kind}-{ms}-{n}.{}", format.extension()))
}

/// Width and height in pixels, read from the header (None for SVG or junk).
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .filter(|(w, h)| *w > 0 && *h > 0)
}

/// The name shown on a chip: the file name, with a slash for a folder.
pub fn chip_name(path: &Path) -> String {
    let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
    if path.is_dir() { format!("{name}/") } else { name }
}

/// "812 B", "14.2 KB", "3.1 MB".
pub fn human_size(n: u64) -> String {
    match n {
        0..1000 => format!("{n} B"),
        1000..1_000_000 => format!("{:.1} KB", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.1} GB", n as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_blank_text_or_a_url_stands_in_for_an_image() {
        for t in ["", "  \n", "https://example.com/cat.png", " http://x.test/a?b=c\n", "data:image/png;base64,AAAA", "blob:https://x.test/1", "file:///tmp/a.png"] {
            assert!(stands_in_for_image(t), "{t:?}");
        }
        for t in ["cat", "see https://example.com/cat.png", "https://a.test https://b.test", "www.example.com", "/tmp/a.png"] {
            assert!(!stands_in_for_image(t), "{t:?}");
        }
    }

    /// "Copy Image" in a browser, on a pasteboard of its own (never the
    /// general one, which is yours): the image is found behind the URL.
    #[cfg(target_os = "macos")]
    #[test]
    fn finds_the_image_beside_a_copied_url() {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF};
        use objc2_foundation::{NSData, NSString};
        let name = NSString::from_str(&format!("qur-test-{}-{:?}", std::process::id(), std::time::Instant::now()));
        let pb = NSPasteboard::pasteboardWithName(&name);
        let (png_type, tiff_type, string_type) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF, NSPasteboardTypeString) };
        pb.clearContents();
        pb.setString_forType(&NSString::from_str("https://example.com/cat.png"), string_type);
        assert!(image_in(&pb, false).is_none());
        let bytes = png(3, 2);
        pb.setData_forType(Some(&NSData::with_bytes(b"tiff")), tiff_type);
        pb.setData_forType(Some(&NSData::with_bytes(&bytes)), png_type);
        assert_eq!(image_in(&pb, false), Some(Vec::new()));
        // PNG before TIFF.
        assert_eq!(image_in(&pb, true), Some(bytes));
        pb.clearContents();
        assert!(image_in(&pb, true).is_none());
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(w, h).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn small_images_go_as_they_are() {
        let bytes = png(3, 2);
        let img = sendable_image(bytes.clone()).unwrap();
        assert_eq!(img.format, gpui::ImageFormat::Png);
        assert_eq!(img.bytes(), &bytes[..]);
        assert_eq!(image_size(&bytes), Some((3, 2)));
    }

    #[test]
    fn other_formats_become_png() {
        let mut bmp = Vec::new();
        image::RgbImage::new(4, 4).write_to(&mut Cursor::new(&mut bmp), image::ImageFormat::Bmp).unwrap();
        let img = sendable_image(bmp).unwrap();
        assert_eq!(img.format, gpui::ImageFormat::Png);
        assert_eq!(image_size(img.bytes()), Some((4, 4)));
        assert!(sendable_image(b"not an image".to_vec()).is_none());
    }

    #[test]
    fn oversized_images_shrink_under_the_limit() {
        // Noise doesn't compress: a 3000×3000 PNG of it is far over 3.7 MB.
        let mut seed = 1u32;
        let noise = image::RgbImage::from_fn(3000, 3000, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            image::Rgb([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8])
        });
        let mut big = Vec::new();
        noise.write_to(&mut Cursor::new(&mut big), image::ImageFormat::Png).unwrap();
        assert!(big.len() > MAX_SEND);
        let img = sendable_image(big).unwrap();
        assert!(img.bytes().len() <= MAX_SEND);
        let (w, h) = image_size(img.bytes()).unwrap();
        assert_eq!(w.max(h), MAX_SIDE);
    }

    #[test]
    fn thumbnails_fit_the_box_and_never_grow() {
        let size = |t: &gpui::RenderImage| (t.size(0).width.0, t.size(0).height.0);
        let big = png(1600, 3065);
        assert_eq!(size(&thumbnail(&big, THUMB.0, THUMB.1).unwrap()), (100, 192));
        assert_eq!(size(&thumbnail(&big, 4000, 1000).unwrap()).1, 1000);
        assert_eq!(size(&thumbnail(&big, 500, 4000).unwrap()).0, 500);
        assert_eq!(size(&thumbnail(&png(3, 2), THUMB.0, THUMB.1).unwrap()), (3, 2));
        assert!(thumbnail(b"not an image", 10, 10).is_none());
    }

    #[test]
    fn thumbnails_are_bgra() {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255])).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        let t = thumbnail(&out, 64, 64).unwrap();
        assert_eq!(&t.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn temp_paths_never_repeat() {
        let a = temp_image_path("paste", gpui::ImageFormat::Png);
        assert_ne!(a, temp_image_path("paste", gpui::ImageFormat::Png));
        assert_eq!(a.extension().unwrap(), "png");
    }

    #[test]
    fn viewable_by_extension() {
        assert_eq!(viewable(Path::new("a/B.PNG")), Some(gpui::ImageFormat::Png));
        assert_eq!(viewable(Path::new("logo.svg")), Some(gpui::ImageFormat::Svg));
        assert_eq!(viewable(Path::new("main.rs")), None);
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(14_200), "14.2 KB");
    }
}
