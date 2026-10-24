//! Non-destructive edits: a small recipe (rotate, flip, crop) stored in the
//! library and applied to the original's pixels when rendering.
//!
//! Order: the decoded original (EXIF orientation already applied by the
//! decoder) → rotate clockwise → flip → crop. The crop rectangle is in
//! 0..1 coordinates of the rotated and flipped image.

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
}

fn full() -> Rect {
    Rect::FULL
}

impl Default for Recipe {
    fn default() -> Self {
        Self { quarter_turns: 0, flip_horizontal: false, flip_vertical: false, crop: Rect::FULL }
    }
}

impl Recipe {
    pub fn is_identity(&self) -> bool {
        self.quarter_turns % 4 == 0 && !self.flip_horizontal && !self.flip_vertical && self.crop.is_full()
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

    /// Apply to decoded pixels.
    pub fn apply(&self, image: RgbaImage) -> RgbaImage {
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

/// JPEG bytes, with `exif` (raw TIFF from the original) carried over when given.
pub fn encode_jpeg(image: &RgbaImage, quality: u8, exif: Option<&[u8]>) -> Vec<u8> {
    let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder.encode_image(&rgb).expect("encode JPEG into memory");
    match exif {
        Some(tiff) => with_exif(out, tiff),
        None => out,
    }
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
        assert_eq!(reds(&Recipe::default().apply(sample())), reds(&sample()));
    }

    #[test]
    fn rotate_clockwise_then_flip() {
        let mut r = Recipe::default();
        r.rotate_clockwise();
        let out = r.apply(sample());
        assert_eq!(out.dimensions(), (2, 4));
        // Clockwise: the bottom-left pixel (4) ends up top-left.
        assert_eq!(reds(&out), [4, 0, 5, 1, 6, 2, 7, 3]);
        r.flip_horizontally();
        assert_eq!(reds(&r.apply(sample())), [0, 4, 1, 5, 2, 6, 3, 7]);
    }

    #[test]
    fn four_rotations_are_the_identity() {
        let mut r = Recipe { crop: Rect { x: 0.25, y: 0.0, w: 0.5, h: 1.0 }, ..Default::default() };
        let before = reds(&r.apply(sample()));
        for _ in 0..4 {
            r.rotate_clockwise();
        }
        assert_eq!(r.quarter_turns, 0);
        assert_eq!(reds(&r.apply(sample())), before);
    }

    #[test]
    fn crop_follows_the_picture_when_rotating() {
        // Keep the right half (pixels 2,3,6,7), then rotate: still those pixels.
        let mut r = Recipe { crop: Rect { x: 0.5, y: 0.0, w: 0.5, h: 1.0 }, ..Default::default() };
        let mut kept = reds(&r.apply(sample()));
        kept.sort();
        r.rotate_clockwise();
        let mut after = reds(&r.apply(sample()));
        after.sort();
        assert_eq!(kept, after);
        r.flip_vertically();
        let mut flipped = reds(&r.apply(sample()));
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
        let jpeg = encode_jpeg(&sample(), 90, Some(&tiff));
        let exif = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(&jpeg)).unwrap();
        let orientation = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY).unwrap();
        assert_eq!(orientation.value.get_uint(0), Some(1));
        assert!(image::load_from_memory(&jpeg).is_ok());
    }
}
