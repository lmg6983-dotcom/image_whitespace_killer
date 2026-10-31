//! Decoding / encoding with resolution (DPI) preservation.
//!
//! Patent offices care about drawing resolution, so the DPI stored in the
//! source file is carried over to the output for PNG (pHYs), JPEG (JFIF),
//! TIFF (X/YResolution) and BMP (pixels-per-metre). GIF/WebP have no DPI field.

use std::io::Cursor;

use anyhow::{Context, Result, anyhow, bail};
use image::codecs::jpeg::{JpegEncoder, PixelDensity, PixelDensityUnit};
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

/// File extensions handled (lower-case, without dot).
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif", "webp"];

pub fn is_supported_ext(ext: &str) -> bool {
    EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

pub struct Decoded {
    pub image: DynamicImage,
    pub format: ImageFormat,
    /// (x, y) dots per inch, if the file declares an absolute resolution.
    pub dpi: Option<(f64, f64)>,
}

pub fn decode(bytes: &[u8]) -> Result<Decoded> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("cannot read image")?;
    let format = reader
        .format()
        .ok_or_else(|| anyhow!("unrecognised image format"))?;
    let mut decoder = reader.into_decoder().context("cannot decode image")?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).context("cannot decode image")?;
    image.apply_orientation(orientation);

    let mut dpi = read_dpi(bytes, format);
    if let Some((x, y)) = dpi
        && matches!(
            orientation,
            Orientation::Rotate90
                | Orientation::Rotate270
                | Orientation::Rotate90FlipH
                | Orientation::Rotate270FlipH
        )
    {
        dpi = Some((y, x));
    }
    Ok(Decoded { image, format, dpi })
}

/// Map an output file extension to an encoder format.
pub fn format_for_ext(ext: &str) -> Option<ImageFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        "bmp" => Some(ImageFormat::Bmp),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

pub fn encode(
    img: &DynamicImage,
    format: ImageFormat,
    dpi: Option<(f64, f64)>,
    jpeg_quality: u8,
) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => {
            let flat = flatten_alpha(img);
            let mut enc = JpegEncoder::new_with_quality(&mut buf, jpeg_quality);
            if let Some((x, y)) = dpi {
                enc.set_pixel_density(PixelDensity {
                    density: (to_u16(x), to_u16(y)),
                    unit: PixelDensityUnit::Inches,
                });
            }
            if flat.color().has_color() {
                enc.encode_image(&flat.to_rgb8())?;
            } else {
                enc.encode_image(&flat.to_luma8())?;
            }
        }
        ImageFormat::Png => {
            to_8_or_16bit(img).write_to(&mut buf, ImageFormat::Png)?;
            if let Some((x, y)) = dpi {
                let out = png_insert_phys(buf.into_inner(), x, y)?;
                return Ok(out);
            }
        }
        ImageFormat::Tiff => return encode_tiff(img, dpi),
        ImageFormat::Bmp => {
            to_8bit(img).write_to(&mut buf, ImageFormat::Bmp)?;
            let mut out = buf.into_inner();
            if let Some((x, y)) = dpi {
                bmp_set_ppm(&mut out, x, y);
            }
            return Ok(out);
        }
        ImageFormat::Gif => {
            DynamicImage::ImageRgba8(img.to_rgba8()).write_to(&mut buf, ImageFormat::Gif)?;
        }
        ImageFormat::WebP => {
            to_8bit(img).write_to(&mut buf, ImageFormat::WebP)?;
        }
        other => bail!("unsupported output format {other:?}"),
    }
    Ok(buf.into_inner())
}

fn to_u16(v: f64) -> u16 {
    v.round().clamp(1.0, u16::MAX as f64) as u16
}

/// Down-convert anything exotic (float, 16-bit) to an 8-bit equivalent.
fn to_8bit(img: &DynamicImage) -> DynamicImage {
    use DynamicImage::*;
    match img {
        ImageLuma8(_) | ImageLumaA8(_) | ImageRgb8(_) | ImageRgba8(_) => img.clone(),
        ImageLuma16(_) => ImageLuma8(img.to_luma8()),
        ImageLumaA16(_) => ImageLumaA8(img.to_luma_alpha8()),
        ImageRgb16(_) | ImageRgb32F(_) => ImageRgb8(img.to_rgb8()),
        _ => ImageRgba8(img.to_rgba8()),
    }
}

/// Keep 16-bit precision where the format supports it, convert floats.
fn to_8_or_16bit(img: &DynamicImage) -> DynamicImage {
    use DynamicImage::*;
    match img {
        ImageRgb32F(_) => ImageRgb16(img.to_rgb16()),
        ImageRgba32F(_) => ImageRgba16(img.to_rgba16()),
        _ => img.clone(),
    }
}

/// Composite transparent pixels over white (JPEG has no alpha).
fn flatten_alpha(img: &DynamicImage) -> DynamicImage {
    if !img.color().has_alpha() {
        return to_8bit(img);
    }
    let mut rgba = img.to_rgba8();
    for p in rgba.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        }
        p[3] = 255;
    }
    let rgb = DynamicImage::ImageRgba8(rgba).to_rgb8();
    if img.color().has_color() {
        DynamicImage::ImageRgb8(rgb)
    } else {
        DynamicImage::ImageLuma8(DynamicImage::ImageRgb8(rgb).to_luma8())
    }
}

fn encode_tiff(img: &DynamicImage, dpi: Option<(f64, f64)>) -> Result<Vec<u8>> {
    use tiff::encoder::{Compression, Rational, TiffEncoder, colortype};
    use tiff::tags::ResolutionUnit;

    let mut buf = Cursor::new(Vec::new());
    let mut enc = TiffEncoder::new(&mut buf)?.with_compression(Compression::Lzw);
    let (w, h) = (img.width(), img.height());

    macro_rules! write {
        ($ct:ty, $data:expr) => {{
            let mut im = enc.new_image::<$ct>(w, h)?;
            if let Some((x, y)) = dpi {
                im.resolution_unit(ResolutionUnit::Inch);
                im.x_resolution(rational(x));
                im.y_resolution(rational(y));
            }
            im.write_data($data)?;
        }};
    }
    fn rational(v: f64) -> Rational {
        Rational {
            n: (v * 100.0).round().clamp(1.0, u32::MAX as f64) as u32,
            d: 100,
        }
    }

    use DynamicImage::*;
    match img {
        ImageLuma8(i) => write!(colortype::Gray8, i.as_raw()),
        ImageLuma16(i) => write!(colortype::Gray16, i.as_raw()),
        ImageRgb8(i) => write!(colortype::RGB8, i.as_raw()),
        ImageRgb16(i) => write!(colortype::RGB16, i.as_raw()),
        ImageRgba16(i) => write!(colortype::RGBA16, i.as_raw()),
        ImageRgb32F(_) => write!(colortype::RGB8, img.to_rgb8().as_raw()),
        _ => write!(colortype::RGBA8, img.to_rgba8().as_raw()),
    }
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// DPI reading
// ---------------------------------------------------------------------------

fn read_dpi(bytes: &[u8], format: ImageFormat) -> Option<(f64, f64)> {
    let dpi = match format {
        ImageFormat::Png => png_read_phys(bytes),
        ImageFormat::Jpeg => jpeg_read_jfif(bytes),
        ImageFormat::Tiff => tiff_read_res(bytes),
        ImageFormat::Bmp => bmp_read_ppm(bytes),
        _ => None,
    }?;
    (dpi.0 > 0.0 && dpi.1 > 0.0 && dpi.0.is_finite() && dpi.1.is_finite()).then_some(dpi)
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn png_read_phys(b: &[u8]) -> Option<(f64, f64)> {
    let mut i = 8;
    while i + 8 <= b.len() {
        let len = be32(b, i)? as usize;
        let kind = b.get(i + 4..i + 8)?;
        if kind == b"pHYs" {
            let d = b.get(i + 8..i + 8 + 9)?;
            if d[8] != 1 {
                return None; // aspect ratio only
            }
            let x = be32(d, 0)? as f64 * 0.0254;
            let y = be32(d, 4)? as f64 * 0.0254;
            return Some((round2(x), round2(y)));
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        i += 12 + len;
    }
    None
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Insert a pHYs chunk right after IHDR (the encoder never writes one).
fn png_insert_phys(png: Vec<u8>, x_dpi: f64, y_dpi: f64) -> Result<Vec<u8>> {
    const IHDR_END: usize = 8 + 8 + 13 + 4;
    if png.len() < IHDR_END || &png[12..16] != b"IHDR" {
        bail!("unexpected PNG layout");
    }
    let ppm = |d: f64| (d / 0.0254).round().clamp(1.0, u32::MAX as f64) as u32;
    let mut data = Vec::with_capacity(13);
    data.extend_from_slice(b"pHYs");
    data.extend_from_slice(&ppm(x_dpi).to_be_bytes());
    data.extend_from_slice(&ppm(y_dpi).to_be_bytes());
    data.push(1); // unit: metre
    let crc = crc32fast::hash(&data);

    let mut out = Vec::with_capacity(png.len() + 21);
    out.extend_from_slice(&png[..IHDR_END]);
    out.extend_from_slice(&9u32.to_be_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&png[IHDR_END..]);
    Ok(out)
}

fn jpeg_read_jfif(b: &[u8]) -> Option<(f64, f64)> {
    if b.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 || marker == 0xFF {
            i += if marker == 0xFF { 1 } else { 2 };
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None; // start of scan / end: no JFIF found
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let seg = b.get(i + 4..i + 2 + len)?;
        if marker == 0xE0 && seg.len() >= 12 && &seg[0..5] == b"JFIF\0" {
            let unit = seg[7];
            let x = u16::from_be_bytes([seg[8], seg[9]]) as f64;
            let y = u16::from_be_bytes([seg[10], seg[11]]) as f64;
            return match unit {
                1 => Some((x, y)),
                2 => Some((round2(x * 2.54), round2(y * 2.54))),
                _ => None,
            };
        }
        i += 2 + len;
    }
    None
}

fn tiff_read_res(b: &[u8]) -> Option<(f64, f64)> {
    use tiff::tags::Tag;
    let mut dec = tiff::decoder::Decoder::new(Cursor::new(b)).ok()?;
    fn num(v: tiff::decoder::ifd::Value) -> Option<f64> {
        use tiff::decoder::ifd::Value::*;
        match v {
            Rational(n, d) if d != 0 => Some(n as f64 / d as f64),
            Float(f) => Some(f as f64),
            Double(f) => Some(f),
            Short(v) => Some(v as f64),
            Unsigned(v) => Some(v as f64),
            List(mut l) if !l.is_empty() => num(l.swap_remove(0)),
            _ => None,
        }
    }
    let x = num(dec.get_tag(Tag::XResolution).ok()?)?;
    let y = dec
        .get_tag(Tag::YResolution)
        .ok()
        .and_then(num)
        .unwrap_or(x);
    let unit: u16 = dec
        .find_tag_unsigned(Tag::ResolutionUnit)
        .ok()
        .flatten()
        .unwrap_or(2);
    match unit {
        2 => Some((round2(x), round2(y))),
        3 => Some((round2(x * 2.54), round2(y * 2.54))),
        _ => None,
    }
}

fn bmp_read_ppm(b: &[u8]) -> Option<(f64, f64)> {
    if b.get(0..2)? != b"BM" || u32::from_le_bytes(b.get(14..18)?.try_into().ok()?) < 40 {
        return None;
    }
    let x = i32::from_le_bytes(b.get(38..42)?.try_into().ok()?);
    let y = i32::from_le_bytes(b.get(42..46)?.try_into().ok()?);
    (x > 0 && y > 0).then(|| (round2(x as f64 * 0.0254), round2(y as f64 * 0.0254)))
}

fn bmp_set_ppm(b: &mut [u8], x_dpi: f64, y_dpi: f64) {
    if b.len() < 46 || &b[0..2] != b"BM" {
        return;
    }
    let ppm = |d: f64| ((d / 0.0254).round().clamp(1.0, i32::MAX as f64) as i32).to_le_bytes();
    b[38..42].copy_from_slice(&ppm(x_dpi));
    b[42..46].copy_from_slice(&ppm(y_dpi));
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma, RgbImage};

    fn sample() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(7, 5, Luma([200])))
    }

    #[test]
    fn dpi_roundtrip_all_formats() {
        for fmt in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Tiff,
            ImageFormat::Bmp,
        ] {
            let bytes = encode(&sample(), fmt, Some((300.0, 300.0)), 95).unwrap();
            let d = decode(&bytes).unwrap();
            assert_eq!(d.format, fmt);
            assert_eq!((d.image.width(), d.image.height()), (7, 5));
            let (x, y) = d.dpi.unwrap_or_else(|| panic!("{fmt:?} lost dpi"));
            assert!(
                (x - 300.0).abs() < 0.01 && (y - 300.0).abs() < 0.01,
                "{fmt:?}: {x},{y}"
            );
        }
    }

    #[test]
    fn encodes_without_dpi() {
        let rgb = DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])));
        for fmt in [
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Png,
            ImageFormat::Tiff,
        ] {
            let bytes = encode(&rgb, fmt, None, 95).unwrap();
            let d = decode(&bytes).unwrap();
            assert_eq!(d.format, fmt);
            assert_eq!((d.image.width(), d.image.height()), (4, 4));
        }
    }
}
