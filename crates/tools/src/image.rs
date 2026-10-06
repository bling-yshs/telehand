//! Image reading, following pi's `mime.ts`, `image-process.ts` and
//! `image-resize-core.ts`.

use std::io::Cursor;

use base64::{Engine, engine::general_purpose::STANDARD};
use image::{
    DynamicImage, ImageDecoder, ImageFormat, ImageReader, codecs::jpeg::JpegEncoder,
    imageops::FilterType,
};

const IMAGE_TYPE_SNIFF_BYTES: usize = 4100;
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

const MAX_WIDTH: u32 = 2000;
const MAX_HEIGHT: u32 = 2000;
/// 4.5MB of base64 payload, below Anthropic's 5MB limit.
const MAX_BYTES: usize = 4718592;
const JPEG_QUALITIES: [u8; 5] = [80, 85, 70, 55, 40];

fn read_u16_le(b: &[u8], offset: usize) -> u32 {
    let at = |i: usize| u32::from(*b.get(offset + i).unwrap_or(&0));
    at(0) | (at(1) << 8)
}

fn read_u32_le(b: &[u8], offset: usize) -> u64 {
    let at = |i: usize| u64::from(*b.get(offset + i).unwrap_or(&0));
    at(0) | (at(1) << 8) | (at(2) << 16) | (at(3) << 24)
}

fn read_u32_be(b: &[u8], offset: usize) -> u64 {
    let at = |i: usize| u64::from(*b.get(offset + i).unwrap_or(&0));
    (at(0) << 24) | (at(1) << 16) | (at(2) << 8) | at(3)
}

fn ascii_at(b: &[u8], offset: usize, text: &str) -> bool {
    b.get(offset..offset + text.len()) == Some(text.as_bytes())
}

fn is_png(b: &[u8]) -> bool {
    b.len() >= 16 && read_u32_be(b, PNG_SIGNATURE.len()) == 13 && ascii_at(b, 12, "IHDR")
}

fn is_animated_png(b: &[u8]) -> bool {
    let mut offset = PNG_SIGNATURE.len();
    while offset + 8 <= b.len() {
        let chunk_length = read_u32_be(b, offset) as usize;
        if ascii_at(b, offset + 4, "acTL") {
            return true;
        }
        if ascii_at(b, offset + 4, "IDAT") {
            return false;
        }
        let next = offset + 8 + chunk_length + 4;
        if next <= offset || next > b.len() {
            return false;
        }
        offset = next;
    }
    false
}

fn is_bmp(b: &[u8]) -> bool {
    if b.len() < 26 {
        return false;
    }
    let declared_size = read_u32_le(b, 2);
    let pixel_offset = read_u32_le(b, 10);
    let dib_header_size = read_u32_le(b, 14);
    if declared_size != 0 && declared_size < 26 {
        return false;
    }
    if pixel_offset < 14 + dib_header_size {
        return false;
    }
    if declared_size != 0 && pixel_offset >= declared_size {
        return false;
    }
    let (planes, bits) = if dib_header_size == 12 {
        (read_u16_le(b, 22), read_u16_le(b, 24))
    } else if (40..=124).contains(&dib_header_size) {
        if b.len() < 30 {
            return false;
        }
        (read_u16_le(b, 26), read_u16_le(b, 28))
    } else {
        return false;
    };
    planes == 1 && [1, 4, 8, 16, 24, 32].contains(&bits)
}

/// The MIME type of a supported image, sniffed from the file's first bytes.
pub fn detect_mime_type(bytes: &[u8]) -> Option<&'static str> {
    let b = &bytes[..bytes.len().min(IMAGE_TYPE_SNIFF_BYTES)];
    if b.starts_with(&[0xff, 0xd8, 0xff]) {
        return (b.get(3) != Some(&0xf7)).then_some("image/jpeg");
    }
    if b.starts_with(&PNG_SIGNATURE) {
        return (is_png(b) && !is_animated_png(b)).then_some("image/png");
    }
    if ascii_at(b, 0, "GIF87a") || ascii_at(b, 0, "GIF89a") {
        return Some("image/gif");
    }
    if ascii_at(b, 0, "RIFF") && ascii_at(b, 8, "WEBP") {
        return Some("image/webp");
    }
    if ascii_at(b, 0, "BM") && is_bmp(b) {
        return Some("image/bmp");
    }
    None
}

pub struct Processed {
    pub data: String,
    pub mime_type: String,
    pub hints: Vec<String>,
}

fn decode(bytes: &[u8]) -> Option<DynamicImage> {
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Some(image)
}

fn encode_png(image: &DynamicImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .ok()?;
    Some(out)
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let rgb = DynamicImage::ImageRgb8(image.to_rgb8());
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&rgb)
        .ok()?;
    Some(out)
}

struct Resized {
    data: String,
    mime_type: String,
    original: (u32, u32),
    size: (u32, u32),
    was_resized: bool,
}

/// Fit the image within the dimension and encoded-size limits, trying PNG and
/// decreasing JPEG qualities, then progressively smaller dimensions.
fn resize(bytes: &[u8], mime_type: &str) -> Option<Resized> {
    let image = decode(bytes)?;
    let (width, height) = (image.width(), image.height());
    let base64_size = bytes.len().div_ceil(3) * 4;
    if width <= MAX_WIDTH && height <= MAX_HEIGHT && base64_size < MAX_BYTES {
        return Some(Resized {
            data: STANDARD.encode(bytes),
            mime_type: mime_type.to_string(),
            original: (width, height),
            size: (width, height),
            was_resized: false,
        });
    }

    let (mut target_w, mut target_h) = (f64::from(width), f64::from(height));
    if target_w > f64::from(MAX_WIDTH) {
        target_h = (target_h * f64::from(MAX_WIDTH) / target_w).round();
        target_w = f64::from(MAX_WIDTH);
    }
    if target_h > f64::from(MAX_HEIGHT) {
        target_w = (target_w * f64::from(MAX_HEIGHT) / target_h).round();
        target_h = f64::from(MAX_HEIGHT);
    }
    let (mut w, mut h) = ((target_w as u32).max(1), (target_h as u32).max(1));

    loop {
        let resized = image.resize_exact(w, h, FilterType::Lanczos3);
        let candidates = encode_png(&resized)
            .map(|png| (png, "image/png"))
            .into_iter()
            .chain(
                JPEG_QUALITIES
                    .iter()
                    .filter_map(|&q| encode_jpeg(&resized, q).map(|jpeg| (jpeg, "image/jpeg"))),
            );
        for (encoded, mime) in candidates {
            let data = STANDARD.encode(&encoded);
            if data.len() < MAX_BYTES {
                return Some(Resized {
                    data,
                    mime_type: mime.to_string(),
                    original: (width, height),
                    size: (w, h),
                    was_resized: true,
                });
            }
        }
        if w == 1 && h == 1 {
            return None;
        }
        let next_w = if w == 1 { 1 } else { ((w as f64 * 0.75).floor() as u32).max(1) };
        let next_h = if h == 1 { 1 } else { ((h as f64 * 0.75).floor() as u32).max(1) };
        if (next_w, next_h) == (w, h) {
            return None;
        }
        (w, h) = (next_w, next_h);
    }
}

fn dimension_note(resized: &Resized) -> Option<String> {
    if !resized.was_resized {
        return None;
    }
    let (ow, oh) = resized.original;
    let (w, h) = resized.size;
    let scale = f64::from(ow) / f64::from(w);
    Some(format!(
        "[Image: original {ow}x{oh}, displayed at {w}x{h}. Multiply coordinates by {scale:.2} to map to original image.]"
    ))
}

/// Convert unsupported formats to PNG and fit the image within inline limits.
/// On failure, returns the note to show instead of the image.
pub fn process(bytes: &[u8], mime_type: &str) -> Result<Processed, String> {
    let supported = matches!(
        mime_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    );
    let (bytes, mime_type, converted_from) = if supported {
        (bytes.to_vec(), mime_type.to_string(), None)
    } else {
        let png = decode(bytes).and_then(|image| encode_png(&image)).ok_or_else(|| {
            "[Image omitted: could not be converted to a supported inline image format.]"
                .to_string()
        })?;
        (png, "image/png".to_string(), Some(mime_type.to_string()))
    };

    let resized = resize(&bytes, &mime_type).ok_or_else(|| {
        "[Image omitted: could not be resized below the inline image size limit.]".to_string()
    })?;
    let mut hints = Vec::new();
    if let Some(from) = converted_from
        && from != resized.mime_type
    {
        hints.push(format!(
            "[Image converted from {from} to {}.]",
            resized.mime_type
        ));
    }
    if let Some(note) = dimension_note(&resized) {
        hints.push(note);
    }
    Ok(Processed {
        data: resized.data,
        mime_type: resized.mime_type,
        hints,
    })
}

#[cfg(test)]
mod tests {
    use image::{Rgb, RgbImage};

    use super::*;

    fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([10, 200, 30])));
        let mut out = Vec::new();
        image.write_to(&mut Cursor::new(&mut out), format).unwrap();
        out
    }

    #[test]
    fn mime_types_are_sniffed_from_content() {
        assert_eq!(detect_mime_type(&encoded(4, 4, ImageFormat::Png)), Some("image/png"));
        assert_eq!(detect_mime_type(&encoded(4, 4, ImageFormat::Jpeg)), Some("image/jpeg"));
        assert_eq!(detect_mime_type(&encoded(4, 4, ImageFormat::Gif)), Some("image/gif"));
        assert_eq!(detect_mime_type(&encoded(4, 4, ImageFormat::Bmp)), Some("image/bmp"));
        assert_eq!(detect_mime_type(b"plain text"), None);
        assert_eq!(detect_mime_type(b""), None);
    }

    #[test]
    fn small_images_are_returned_unchanged() {
        let png = encoded(10, 10, ImageFormat::Png);
        let processed = process(&png, "image/png").unwrap();
        assert_eq!(processed.mime_type, "image/png");
        assert_eq!(processed.data, STANDARD.encode(&png));
        assert!(processed.hints.is_empty());
    }

    #[test]
    fn large_images_are_resized() {
        let png = encoded(3000, 1000, ImageFormat::Png);
        let processed = process(&png, "image/png").unwrap();
        assert_eq!(
            processed.hints,
            vec!["[Image: original 3000x1000, displayed at 2000x667. Multiply coordinates by 1.50 to map to original image.]"]
        );
        let decoded = image::load_from_memory(&STANDARD.decode(&processed.data).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2000, 667));
    }

    #[test]
    fn bmp_is_converted_to_png() {
        let bmp = encoded(5, 5, ImageFormat::Bmp);
        let processed = process(&bmp, "image/bmp").unwrap();
        assert_eq!(processed.mime_type, "image/png");
        assert_eq!(
            processed.hints,
            vec!["[Image converted from image/bmp to image/png.]"]
        );
    }
}
