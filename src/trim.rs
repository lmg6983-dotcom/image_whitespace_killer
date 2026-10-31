//! Background detection and content bounding-box computation.
//!
//! Equivalent of ImageMagick's `-fuzz N% -bordercolor <bg> -trim`: every
//! row/column at the edges whose pixels are all "close enough" to the
//! background colour is removed.

use image::RgbaImage;
use serde::Serialize;

/// Axis-aligned rectangle in pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Requested background colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    /// A fixed colour (RGB).
    Color([u8; 3]),
    /// Guess from the four corner pixels (majority vote, top-left wins ties).
    Auto,
}

/// Parse a background spec: `white`, `black`, `auto`, `#RGB`, `#RRGGBB`, `RRGGBB`.
pub fn parse_background(s: &str) -> Result<Background, String> {
    let t = s.trim().to_ascii_lowercase();
    match t.as_str() {
        "auto" => return Ok(Background::Auto),
        "white" => return Ok(Background::Color([255, 255, 255])),
        "black" => return Ok(Background::Color([0, 0, 0])),
        _ => {}
    }
    let hex = t.strip_prefix('#').unwrap_or(&t);
    let expanded: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        _ => {
            return Err(format!(
                "invalid colour '{s}' (use white, black, auto, #RGB or #RRGGBB)"
            ));
        }
    };
    let byte = |i: usize| u8::from_str_radix(&expanded[i..i + 2], 16);
    match (byte(0), byte(2), byte(4)) {
        (Ok(r), Ok(g), Ok(b)) => Ok(Background::Color([r, g, b])),
        _ => Err(format!(
            "invalid colour '{s}' (use white, black, auto, #RGB or #RRGGBB)"
        )),
    }
}

pub fn hex_color(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Composite a pixel over `under` (straight alpha).
#[inline]
fn flatten(px: [u8; 4], under: [u8; 3]) -> [u8; 3] {
    let a = px[3] as u32;
    if a == 255 {
        return [px[0], px[1], px[2]];
    }
    let mix = |c: u8, u: u8| ((c as u32 * a + u as u32 * (255 - a) + 127) / 255) as u8;
    [
        mix(px[0], under[0]),
        mix(px[1], under[1]),
        mix(px[2], under[2]),
    ]
}

/// Resolve `Background::Auto` using the corner pixels.
pub fn resolve_background(img: &RgbaImage, bg: Background) -> [u8; 3] {
    match bg {
        Background::Color(c) => c,
        Background::Auto => {
            let (w, h) = img.dimensions();
            if w == 0 || h == 0 {
                return [255, 255, 255];
            }
            let white = [255, 255, 255];
            let corners = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
                .map(|(x, y)| flatten(img.get_pixel(x, y).0, white));
            let mut best = corners[0];
            let mut best_count = 0;
            for c in corners {
                let count = corners.iter().filter(|&&o| o == c).count();
                if count > best_count {
                    best = c;
                    best_count = count;
                }
            }
            best
        }
    }
}

/// Bounding box of everything that is *not* background.
///
/// `fuzz_percent` follows ImageMagick's meaning: the maximum RGB distance,
/// normalised to 0–100 %, for a pixel to still count as background.
/// Returns `None` when the whole image is background (blank page).
pub fn content_bbox(img: &RgbaImage, bg: [u8; 3], fuzz_percent: f64) -> Option<Rect> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    // distance = sqrt((dr² + dg² + db²) / 3), compared against fuzz * 255.
    let limit = fuzz_percent.clamp(0.0, 100.0) / 100.0 * 255.0;
    let threshold = (limit * limit * 3.0).floor() as u32;

    let is_bg = |x: u32, y: u32| -> bool {
        let c = flatten(img.get_pixel(x, y).0, bg);
        let d = |i: usize| {
            let v = c[i] as i32 - bg[i] as i32;
            (v * v) as u32
        };
        d(0) + d(1) + d(2) <= threshold
    };
    let row_has_content = |y: u32| (0..w).any(|x| !is_bg(x, y));

    let top = (0..h).find(|&y| row_has_content(y))?;
    let bottom = (top..h).rev().find(|&y| row_has_content(y)).unwrap_or(top);

    // Only pixels outside the current [left, right] span need checking.
    let mut left = w; // exclusive sentinel: "nothing found yet"
    let mut right: Option<u32> = None;
    for y in top..=bottom {
        if let Some(x) = (0..left).find(|&x| !is_bg(x, y)) {
            left = x;
        }
        let lo = right.map_or(0, |r| r + 1);
        if let Some(x) = (lo..w).rev().find(|&x| !is_bg(x, y)) {
            right = Some(x);
        }
    }
    let right = right?;
    if left >= w {
        return None;
    }
    Some(Rect {
        x: left,
        y: top,
        width: right - left + 1,
        height: bottom - top + 1,
    })
}

/// Grow `r` by `margin` pixels on every side, clamped to the image bounds.
pub fn expand(r: Rect, margin: u32, w: u32, h: u32) -> Rect {
    let x0 = r.x.saturating_sub(margin);
    let y0 = r.y.saturating_sub(margin);
    let x1 = (r.x + r.width).saturating_add(margin).min(w);
    let y1 = (r.y + r.height).saturating_add(margin).min(h);
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn canvas(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]))
    }

    #[test]
    fn finds_box() {
        let mut img = canvas(100, 80);
        for y in 10..20 {
            for x in 30..50 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let r = content_bbox(&img, [255, 255, 255], 5.0).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 30,
                y: 10,
                width: 20,
                height: 10
            }
        );
    }

    #[test]
    fn single_pixel_at_edges() {
        for (px, py) in [(0, 0), (99, 0), (0, 79), (99, 79), (50, 40)] {
            let mut img = canvas(100, 80);
            img.put_pixel(px, py, Rgba([0, 0, 0, 255]));
            let r = content_bbox(&img, [255, 255, 255], 5.0).unwrap();
            assert_eq!(
                r,
                Rect {
                    x: px,
                    y: py,
                    width: 1,
                    height: 1
                },
                "pixel {px},{py}"
            );
        }
    }

    #[test]
    fn fuzz_ignores_near_white() {
        let mut img = canvas(50, 50);
        img.put_pixel(5, 5, Rgba([250, 250, 250, 255])); // ~2% off
        img.put_pixel(20, 20, Rgba([0, 0, 0, 255]));
        let r = content_bbox(&img, [255, 255, 255], 5.0).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 20,
                y: 20,
                width: 1,
                height: 1
            }
        );
        let r0 = content_bbox(&img, [255, 255, 255], 0.0).unwrap();
        assert_eq!(
            r0,
            Rect {
                x: 5,
                y: 5,
                width: 16,
                height: 16
            }
        );
    }

    #[test]
    fn blank_and_transparent() {
        assert!(content_bbox(&canvas(10, 10), [255, 255, 255], 5.0).is_none());
        let clear = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 0]));
        assert!(content_bbox(&clear, [255, 255, 255], 5.0).is_none());
    }

    #[test]
    fn parse_colors() {
        assert_eq!(
            parse_background("White").unwrap(),
            Background::Color([255, 255, 255])
        );
        assert_eq!(
            parse_background("#f00").unwrap(),
            Background::Color([255, 0, 0])
        );
        assert_eq!(
            parse_background("00ff80").unwrap(),
            Background::Color([0, 255, 128])
        );
        assert_eq!(parse_background("auto").unwrap(), Background::Auto);
        assert!(parse_background("#12").is_err());
    }

    #[test]
    fn auto_background() {
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        assert_eq!(resolve_background(&img, Background::Auto), [0, 0, 0]);
    }

    #[test]
    fn expand_clamps() {
        let r = expand(
            Rect {
                x: 2,
                y: 3,
                width: 5,
                height: 5,
            },
            10,
            20,
            9,
        );
        assert_eq!(
            r,
            Rect {
                x: 0,
                y: 0,
                width: 17,
                height: 9
            }
        );
    }
}
