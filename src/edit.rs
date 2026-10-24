//! Non-destructive edits: a small recipe (rotate, flip, crop) stored in the
//! library and applied to the original's pixels when rendering.
//!
//! Order: the decoded original (EXIF orientation already applied by the
//! decoder) → rotate clockwise → flip → crop → color. The crop rectangle
//! is in 0..1 coordinates of the rotated and flipped image.
//!
//! Color adjustments work where each looks natural: white balance and
//! exposure in linear light; contrast, highlights and shadows on Oklab
//! lightness; saturation and vibrance on Oklab chroma. Pixels keep the
//! original's encoding (iPhone photos: Display P3, converted for the maths),
//! and the original's ICC profile goes into the output.

use gtk::prelude::*;
use image::{ImageBuffer, RgbaImage, imageops};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const FULL: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

    pub fn is_full(&self) -> bool {
        self.x <= 1e-4 && self.y <= 1e-4 && self.w >= 1.0 - 1e-4 && self.h >= 1.0 - 1e-4
    }
}

/// Color corrections. Exposure is in stops (-3..3); the rest are -1..1,
/// 0 meaning unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustments {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub warmth: f32,
    pub tint: f32,
}

impl Adjustments {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

/// The primaries the pixel values are encoded in (both use the sRGB curve).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gamut {
    Srgb,
    DisplayP3,
}

impl Gamut {
    /// Display P3 when the ICC profile says so (iPhone photos).
    pub fn from_icc(icc: Option<&[u8]>) -> Self {
        let Some(icc) = icc else { return Gamut::Srgb };
        let ascii = icc.windows(10).any(|w| w == b"Display P3");
        let utf16: Vec<u8> = "Display P3".bytes().flat_map(|b| [0, b]).collect();
        if ascii || icc.windows(utf16.len()).any(|w| w == utf16.as_slice()) { Gamut::DisplayP3 } else { Gamut::Srgb }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Clockwise quarter turns, 0..=3.
    #[serde(default)]
    pub quarter_turns: u8,
    #[serde(default)]
    pub flip_horizontal: bool,
    #[serde(default)]
    pub flip_vertical: bool,
    #[serde(default = "full")]
    pub crop: Rect,
    #[serde(default)]
    pub color: Adjustments,
}

fn full() -> Rect {
    Rect::FULL
}

impl Default for Recipe {
    fn default() -> Self {
        Self { quarter_turns: 0, flip_horizontal: false, flip_vertical: false, crop: Rect::FULL, color: Adjustments::default() }
    }
}

impl Recipe {
    pub fn is_identity(&self) -> bool {
        self.quarter_turns % 4 == 0 && !self.flip_horizontal && !self.flip_vertical && self.crop.is_full() && self.color.is_identity()
    }

    /// Size after rotating (before cropping) for an image of `w`×`h`.
    pub fn oriented_size(&self, w: u32, h: u32) -> (u32, u32) {
        if self.quarter_turns % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// Rotating changes which way is "horizontal": keep the crop on the
    /// same part of the picture.
    pub fn rotate_clockwise(&mut self) {
        self.quarter_turns = (self.quarter_turns + 1) % 4;
        let c = self.crop;
        self.crop = Rect { x: 1.0 - c.y - c.h, y: c.x, w: c.h, h: c.w };
        std::mem::swap(&mut self.flip_horizontal, &mut self.flip_vertical);
    }

    pub fn rotate_counterclockwise(&mut self) {
        for _ in 0..3 {
            self.rotate_clockwise();
        }
    }

    pub fn flip_horizontally(&mut self) {
        self.flip_horizontal = !self.flip_horizontal;
        self.crop.x = 1.0 - self.crop.x - self.crop.w;
    }

    pub fn flip_vertically(&mut self) {
        self.flip_vertical = !self.flip_vertical;
        self.crop.y = 1.0 - self.crop.y - self.crop.h;
    }

    /// Apply to decoded pixels (geometry, then color).
    pub fn apply(&self, image: RgbaImage, gamut: Gamut) -> RgbaImage {
        let mut image = self.apply_geometry(image);
        apply_color(&mut image, &self.color, gamut);
        image
    }

    fn apply_geometry(&self, image: RgbaImage) -> RgbaImage {
        let mut image = match self.quarter_turns % 4 {
            1 => imageops::rotate90(&image),
            2 => imageops::rotate180(&image),
            3 => imageops::rotate270(&image),
            _ => image,
        };
        if self.flip_horizontal {
            imageops::flip_horizontal_in_place(&mut image);
        }
        if self.flip_vertical {
            imageops::flip_vertical_in_place(&mut image);
        }
        if self.crop.is_full() {
            return image;
        }
        let (w, h) = (image.width() as f64, image.height() as f64);
        let x = (self.crop.x * w).round().clamp(0.0, w - 1.0) as u32;
        let y = (self.crop.y * h).round().clamp(0.0, h - 1.0) as u32;
        let cw = ((self.crop.w * w).round() as u32).clamp(1, image.width() - x);
        let ch = ((self.crop.h * h).round() as u32).clamp(1, image.height() - y);
        imageops::crop_imm(&image, x, y, cw, ch).to_image()
    }
}

// ── color ───────────────────────────────────────────────────────────────────

/// sRGB-curve decoding for 8-bit values.
fn decode_lut() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut lut = [0.0; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        lut
    })
}

const ENCODE_STEPS: usize = 4096;

/// sRGB-curve encoding, linear 0..1 → 8-bit, as a table.
fn encode_lut() -> &'static [u8; ENCODE_STEPS + 1] {
    static LUT: std::sync::OnceLock<[u8; ENCODE_STEPS + 1]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut lut = [0u8; ENCODE_STEPS + 1];
        for (i, v) in lut.iter_mut().enumerate() {
            let l = i as f32 / ENCODE_STEPS as f32;
            let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
            *v = (c * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        lut
    })
}

fn encode(l: f32) -> u8 {
    encode_lut()[(l.clamp(0.0, 1.0) * ENCODE_STEPS as f32 + 0.5) as usize]
}

fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// Linear Display P3 <-> linear sRGB (D65 both).
const P3_TO_SRGB: [[f32; 3]; 3] = [[1.224_940, -0.224_940, 0.0], [-0.042_057, 1.042_057, 0.0], [-0.019_638, -0.078_636, 1.098_274]];
const SRGB_TO_P3: [[f32; 3]; 3] = [[0.822_462, 0.177_538, 0.0], [0.033_194, 0.966_806, 0.0], [0.017_083, 0.072_397, 0.910_520]];

// Oklab (Björn Ottosson), from/to linear sRGB.
fn to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}

/// Apply `adjust` in place. Runs on all cores.
pub fn apply_color(image: &mut RgbaImage, adjust: &Adjustments, gamut: Gamut) {
    use rayon::prelude::*;
    if adjust.is_identity() {
        return;
    }
    let lut = decode_lut();
    let a = *adjust;
    // White balance gains, normalized so overall brightness stays put.
    let (wr, wg, wb) = (2f32.powf(0.35 * a.warmth), 2f32.powf(-0.25 * a.tint), 2f32.powf(-0.35 * a.warmth));
    let norm = 0.2126 * wr + 0.7152 * wg + 0.0722 * wb;
    let gain = 2f32.powf(a.exposure);
    let (wr, wg, wb) = (wr / norm * gain, wg / norm * gain, wb / norm * gain);
    let tone = a.contrast != 0.0 || a.highlights != 0.0 || a.shadows != 0.0 || a.saturation != 0.0 || a.vibrance != 0.0;

    image.par_chunks_mut(4).for_each(|px| {
        let mut rgb = [lut[px[0] as usize] * wr, lut[px[1] as usize] * wg, lut[px[2] as usize] * wb];
        if tone {
            if gamut == Gamut::DisplayP3 {
                rgb = mul(&P3_TO_SRGB, rgb);
            }
            let [mut l, mut ca, mut cb] = to_oklab(rgb);
            // Shadows act on the darker tones, highlights on the brighter.
            l += a.shadows * 0.45 * l * (1.0 - l) * (1.0 - l);
            l += a.highlights * 0.6 * l * l * (1.0 - l);
            if a.contrast != 0.0 {
                // S-curve about the mid tone: steeper (or flatter) in the middle.
                let x = l.clamp(0.0, 1.0) - 0.5;
                let k = 1.0 + a.contrast * 0.9;
                l = 0.5 + x * k / (1.0 + (k - 1.0).max(0.0) * (2.0 * x).powi(2) * 0.5);
            }
            let chroma = (ca * ca + cb * cb).sqrt();
            let mut scale = 1.0 + a.saturation;
            if a.vibrance != 0.0 {
                // Muted colors move more than already-vivid ones.
                scale *= 1.0 + a.vibrance * (1.0 - (chroma / 0.2).min(1.0));
            }
            ca *= scale.max(0.0);
            cb *= scale.max(0.0);
            rgb = from_oklab([l, ca, cb]);
            if gamut == Gamut::DisplayP3 {
                rgb = mul(&SRGB_TO_P3, rgb);
            }
        }
        px[0] = encode(rgb[0]);
        px[1] = encode(rgb[1]);
        px[2] = encode(rgb[2]);
    });
}

/// RGBA pixels from a GDK texture (what the decoder hands back).
pub fn texture_pixels(texture: &gtk::gdk::Texture) -> RgbaImage {
    let mut downloader = gtk::gdk::TextureDownloader::new(texture);
    downloader.set_format(gtk::gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let (w, h) = (texture.width() as u32, texture.height() as u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for row in bytes.chunks(stride).take(h as usize) {
        data.extend_from_slice(&row[..(w * 4) as usize]);
    }
    ImageBuffer::from_raw(w, h, data).expect("downloaded texture size")
}

/// Scale so the long edge is at most `long_edge`.
pub fn fit(image: &RgbaImage, long_edge: u32) -> RgbaImage {
    let (w, h) = image.dimensions();
    if w.max(h) <= long_edge {
        return image.clone();
    }
    let scale = long_edge as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64) * scale).round().max(1.0) as u32, ((h as f64) * scale).round().max(1.0) as u32);
    imageops::resize(image, nw, nh, imageops::FilterType::CatmullRom)
}

/// JPEG bytes, carrying the original's EXIF (raw TIFF) and ICC color
/// profile when given.
pub fn encode_jpeg(image: &RgbaImage, quality: u8, exif: Option<&[u8]>, icc: Option<&[u8]>) -> Vec<u8> {
    let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder.encode_image(&rgb).expect("encode JPEG into memory");
    if let Some(icc) = icc {
        out = with_icc(out, icc);
    }
    match exif {
        Some(tiff) => with_exif(out, tiff),
        None => out,
    }
}

/// Insert the ICC profile as APP2 "ICC_PROFILE" segments after SOI, so
/// viewers show Display P3 colors as such.
pub fn with_icc(jpeg: Vec<u8>, icc: &[u8]) -> Vec<u8> {
    const CHUNK: usize = 65_519 - 14;
    if jpeg.len() < 2 || jpeg[0..2] != [0xFF, 0xD8] || icc.is_empty() {
        return jpeg;
    }
    let chunks: Vec<&[u8]> = icc.chunks(CHUNK).collect();
    if chunks.len() > 255 {
        return jpeg;
    }
    let mut out = Vec::with_capacity(jpeg.len() + icc.len() + chunks.len() * 18);
    out.extend_from_slice(&[0xFF, 0xD8]);
    for (i, chunk) in chunks.iter().enumerate() {
        out.extend_from_slice(&[0xFF, 0xE2]);
        out.extend_from_slice(&((2 + 14 + chunk.len()) as u16).to_be_bytes());
        out.extend_from_slice(b"ICC_PROFILE\0");
        out.push(i as u8 + 1);
        out.push(chunks.len() as u8);
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Insert an APP1 Exif segment after the JPEG's SOI marker. The pixels are
/// already upright, so the Orientation tag is reset to 1.
pub fn with_exif(jpeg: Vec<u8>, tiff: &[u8]) -> Vec<u8> {
    let mut tiff = tiff.to_vec();
    set_orientation_upright(&mut tiff);
    let length = 2 + 6 + tiff.len();
    if jpeg.len() < 2 || jpeg[0..2] != [0xFF, 0xD8] || length > 0xFFFF {
        return jpeg;
    }
    let mut out = Vec::with_capacity(jpeg.len() + length + 2);
    out.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE1]);
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    // The encoder may have written its own APP0/APP1; keep the rest as is.
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// Set IFD0's Orientation (0x0112) to 1 in a raw TIFF/EXIF block, if present.
fn set_orientation_upright(tiff: &mut [u8]) {
    if tiff.len() < 8 {
        return;
    }
    let little = &tiff[0..2] == b"II";
    let u16_at = |b: &[u8], i: usize| if little { u16::from_le_bytes([b[i], b[i + 1]]) } else { u16::from_be_bytes([b[i], b[i + 1]]) };
    let u32_at = |b: &[u8], i: usize| {
        let a = [b[i], b[i + 1], b[i + 2], b[i + 3]];
        if little { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) }
    };
    let ifd = u32_at(tiff, 4) as usize;
    if ifd + 2 > tiff.len() {
        return;
    }
    let count = u16_at(tiff, ifd) as usize;
    for i in 0..count {
        let entry = ifd + 2 + i * 12;
        if entry + 12 > tiff.len() {
            return;
        }
        if u16_at(tiff, entry) == 0x0112 {
            let one = if little { 1u16.to_le_bytes() } else { 1u16.to_be_bytes() };
            tiff[entry + 8..entry + 10].copy_from_slice(&one);
            return;
        }
    }
}

/// The original's raw EXIF (TIFF) block, for carrying into edited JPEGs.
pub fn original_exif(db: &rusqlite::Connection, id: &str) -> Option<Vec<u8>> {
    let blob = crate::library::open_blob(db, id, crate::library::ORIGINAL).ok()?;
    let exif = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(blob)).ok()?;
    Some(exif.buf().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// 4×2 image, each pixel's red channel = its index, to track where pixels go.
    fn sample() -> RgbaImage {
        ImageBuffer::from_fn(4, 2, |x, y| Rgba([(y * 4 + x) as u8, 0, 0, 255]))
    }

    fn reds(image: &RgbaImage) -> Vec<u8> {
        image.pixels().map(|p| p[0]).collect()
    }

    #[test]
    fn identity_leaves_pixels_alone() {
        assert!(Recipe::default().is_identity());
        assert_eq!(reds(&Recipe::default().apply(sample(), Gamut::Srgb)), reds(&sample()));
    }

    #[test]
    fn rotate_clockwise_then_flip() {
        let mut r = Recipe::default();
        r.rotate_clockwise();
        let out = r.apply(sample(), Gamut::Srgb);
        assert_eq!(out.dimensions(), (2, 4));
        // Clockwise: the bottom-left pixel (4) ends up top-left.
        assert_eq!(reds(&out), [4, 0, 5, 1, 6, 2, 7, 3]);
        r.flip_horizontally();
        assert_eq!(reds(&r.apply(sample(), Gamut::Srgb)), [0, 4, 1, 5, 2, 6, 3, 7]);
    }

    #[test]
    fn four_rotations_are_the_identity() {
        let mut r = Recipe { crop: Rect { x: 0.25, y: 0.0, w: 0.5, h: 1.0 }, ..Default::default() };
        let before = reds(&r.apply(sample(), Gamut::Srgb));
        for _ in 0..4 {
            r.rotate_clockwise();
        }
        assert_eq!(r.quarter_turns, 0);
        assert_eq!(reds(&r.apply(sample(), Gamut::Srgb)), before);
    }

    #[test]
    fn crop_follows_the_picture_when_rotating() {
        // Keep the right half (pixels 2,3,6,7), then rotate: still those pixels.
        let mut r = Recipe { crop: Rect { x: 0.5, y: 0.0, w: 0.5, h: 1.0 }, ..Default::default() };
        let mut kept = reds(&r.apply(sample(), Gamut::Srgb));
        kept.sort();
        r.rotate_clockwise();
        let mut after = reds(&r.apply(sample(), Gamut::Srgb));
        after.sort();
        assert_eq!(kept, after);
        r.flip_vertically();
        let mut flipped = reds(&r.apply(sample(), Gamut::Srgb));
        flipped.sort();
        assert_eq!(kept, flipped);
    }

    #[test]
    fn exif_is_carried_with_orientation_reset() {
        // Minimal little-endian TIFF: one IFD entry, Orientation = 6.
        let mut tiff = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&6u16.to_le_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let jpeg = encode_jpeg(&sample(), 90, Some(&tiff), Some(b"fake icc Display P3 profile"));
        let exif = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(&jpeg)).unwrap();
        let orientation = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).unwrap();
        assert_eq!(orientation.value.get_uint(0), Some(1));
        assert!(image::load_from_memory(&jpeg).is_ok());
        let marker = b"ICC_PROFILE\0\x01\x01fake icc";
        assert!(jpeg.windows(marker.len()).any(|w| w == marker), "ICC profile embedded");
    }

    fn gray(v: u8) -> RgbaImage {
        ImageBuffer::from_pixel(2, 2, Rgba([v, v, v, 255]))
    }

    #[test]
    fn zero_adjustments_change_nothing() {
        let mut image = sample();
        apply_color(&mut image, &Adjustments::default(), Gamut::Srgb);
        assert_eq!(reds(&image), reds(&sample()));
    }

    #[test]
    fn exposure_is_in_stops() {
        let mut image = gray(100); // ~0.127 linear
        apply_color(&mut image, &Adjustments { exposure: 1.0, ..Default::default() }, Gamut::Srgb);
        // One stop doubles linear light: 0.127 -> 0.254 ≈ 137 in sRGB.
        assert!((136..=138).contains(&image.get_pixel(0, 0)[0]), "{:?}", image.get_pixel(0, 0));
    }

    #[test]
    fn grays_stay_gray_under_saturation_and_tone() {
        let mut image = gray(120);
        let a = Adjustments { saturation: 1.0, vibrance: 1.0, contrast: 0.5, shadows: 0.5, highlights: -0.5, ..Default::default() };
        apply_color(&mut image, &a, Gamut::DisplayP3);
        let p = image.get_pixel(0, 0);
        assert!(p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1, "{p:?}");
    }

    #[test]
    fn warmth_moves_toward_orange_and_saturation_toward_vivid() {
        let mut warm = gray(128);
        apply_color(&mut warm, &Adjustments { warmth: 0.6, ..Default::default() }, Gamut::Srgb);
        let p = warm.get_pixel(0, 0);
        assert!(p[0] > p[1] && p[1] > p[2], "{p:?}");

        let mut color = ImageBuffer::from_pixel(1, 1, Rgba([160, 110, 90, 255]));
        apply_color(&mut color, &Adjustments { saturation: 0.5, ..Default::default() }, Gamut::Srgb);
        let q = color.get_pixel(0, 0);
        assert!(q[0] > 160 && q[2] < 90, "{q:?}");
    }

    #[test]
    fn shadows_lift_darks_more_than_brights() {
        let (mut dark, mut bright) = (gray(40), gray(220));
        let a = Adjustments { shadows: 1.0, ..Default::default() };
        apply_color(&mut dark, &a, Gamut::Srgb);
        apply_color(&mut bright, &a, Gamut::Srgb);
        assert!(dark.get_pixel(0, 0)[0] - 40 > bright.get_pixel(0, 0)[0].saturating_sub(220));
    }

    #[test]
    fn display_p3_is_recognized() {
        assert_eq!(Gamut::from_icc(Some(b"....desc....Display P3....")), Gamut::DisplayP3);
        assert_eq!(Gamut::from_icc(Some(b"sRGB IEC61966-2.1")), Gamut::Srgb);
        assert_eq!(Gamut::from_icc(None), Gamut::Srgb);
    }
}
