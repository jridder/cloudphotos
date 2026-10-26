//! Turning an edit recipe into pixels, from the full-resolution original.
//!
//! Decoding goes through glycin (GNOME's sandboxed image loaders, which
//! handle HEIC, RAW, PNG… and apply EXIF orientation); on Windows, where
//! there's no glycin, through libheif and the image crate. The transform
//! and JPEG encoding run on a worker thread.

use std::path::{Path, PathBuf};

use gtk::glib;
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::{
    edit::{self, Recipe},
    library::{self, EDITED_PREVIEW, EDITED_THUMB, ORIGINAL},
    store::Store,
};

const PREVIEW_EDGE: u32 = 2048;
const THUMB_EDGE: u32 = 512;

/// The original, decoded, upright, with its ICC profile (the pixels keep
/// its encoding — Display P3 for iPhone photos).
pub async fn decode_original(store: &Store, temp: &Path, id: &str) -> Result<(RgbaImage, Option<Vec<u8>>), String> {
    let source = temp.join(format!("edit-source-{}", glib::compute_checksum_for_string(glib::ChecksumType::Sha1, id).unwrap_or_default()));
    store.copy_out(id, ORIGINAL, source.clone()).await.map_err(|e| format!("The original isn't available ({e:?})"))?;
    let result = decode_file(&source).await;
    let _ = std::fs::remove_file(&source);
    result
}

#[cfg(not(windows))]
async fn decode_file(source: &Path) -> Result<(RgbaImage, Option<Vec<u8>>), String> {
    let mut image = glycin::Loader::new(gtk::gio::File::for_path(source)).load().await.map_err(|e| format!("Couldn't open the original: {e}"))?;
    let frame = image.next_frame().await.map_err(|e| format!("Couldn't decode the original: {e}"))?;
    let icc = frame.details().color_icc_profile().map(|b| b.to_vec());
    Ok((edit::texture_pixels(&frame.texture()), icc))
}

#[cfg(windows)]
async fn decode_file(source: &Path) -> Result<(RgbaImage, Option<Vec<u8>>), String> {
    let bytes = std::fs::read(source).map_err(|e| format!("Couldn't open the original: {e}"))?;
    off_thread(move || {
        // HEIC, HEIF and AVIF are ISO media files: an "ftyp" box comes first.
        if bytes.get(4..8) == Some(b"ftyp".as_slice()) { decode_heif(&bytes) } else { decode_image(&bytes) }
    })
    .await
}

/// HEIC through libheif, which applies the file's rotation and mirroring.
#[cfg(windows)]
fn decode_heif(bytes: &[u8]) -> Result<(RgbaImage, Option<Vec<u8>>), String> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

    let context = HeifContext::read_from_bytes(bytes).map_err(|e| format!("Couldn't open the original: {e}"))?;
    let handle = context.primary_image_handle().map_err(|e| format!("Couldn't open the original: {e}"))?;
    let icc = handle.color_profile_raw().map(|profile| profile.data);
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(|e| format!("Couldn't decode the original: {e}"))?;
    let plane = image.planes().interleaved.ok_or("Couldn't decode the original: no RGBA plane")?;
    let (w, h) = (plane.width, plane.height);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for row in plane.data.chunks(plane.stride).take(h as usize) {
        data.extend_from_slice(&row[..(w * 4) as usize]);
    }
    let pixels = RgbaImage::from_raw(w, h, data).ok_or("Couldn't decode the original: bad image size")?;
    Ok((pixels, icc))
}

/// JPEG, PNG and TIFF through the image crate, turned upright by the EXIF
/// orientation.
#[cfg(windows)]
fn decode_image(bytes: &[u8]) -> Result<(RgbaImage, Option<Vec<u8>>), String> {
    use image::ImageDecoder;

    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("Couldn't open the original: {e}"))?
        .into_decoder()
        .map_err(|e| format!("Couldn't open the original: {e}"))?;
    let icc = decoder.icc_profile().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(|e| format!("Couldn't decode the original: {e}"))?;
    image.apply_orientation(orientation);
    Ok((image.into_rgba8(), icc))
}

/// Run `f` on a worker thread and await its result.
async fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx.recv().await.expect("worker thread finished")
}

/// Render and store the edited thumbnail and preview shown in the app.
pub async fn store_display_images(store: &Store, temp: &Path, library_path: &Path, id: &str, recipe: Recipe) -> Result<(), String> {
    let (pixels, icc) = decode_original(store, temp, id).await?;
    let (library_path, id) = (library_path.to_owned(), id.to_owned());
    off_thread(move || -> Result<(), String> {
        let edited = recipe.apply(pixels, edit::Gamut::from_icc(icc.as_deref()));
        let preview = edit::encode_jpeg(&edit::fit(&edited, PREVIEW_EDGE), 88, None, icc.as_deref());
        let thumb = edit::encode_jpeg(&edit::fit(&edited, THUMB_EDGE), 85, None, icc.as_deref());
        let db = library::connect(&library_path).map_err(|e| e.to_string())?;
        library::write_blob_bytes(&db, &id, EDITED_PREVIEW, &preview).map_err(|e| e.to_string())?;
        library::write_blob_bytes(&db, &id, EDITED_THUMB, &thumb).map_err(|e| e.to_string())
    })
    .await
}

/// File formats a photo can be exported in (besides the original file).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Format {
    Jpeg,
    Png,
    Tiff,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Tiff => "tif",
        }
    }
}

/// How to write a rendered photo.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub format: Format,
    /// Longest side in pixels; `None` = full size.
    pub long_edge: Option<u32>,
    /// JPEG quality, 1–100.
    pub quality: u8,
    /// Keep the GPS position in the EXIF.
    pub location: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self { format: Format::Jpeg, long_edge: None, quality: 92, location: true }
    }
}

/// The photo rendered from its full-resolution original — with `recipe`
/// applied when given — written to `target` as `output` says, carrying the
/// original's EXIF (date, camera, and location unless left out) and ICC
/// color profile.
pub async fn render_full(
    store: &Store,
    temp: &Path,
    library_path: &Path,
    id: &str,
    recipe: Option<Recipe>,
    output: Output,
    target: PathBuf,
) -> Result<PathBuf, String> {
    let (pixels, icc) = decode_original(store, temp, id).await?;
    let (library_path, id) = (library_path.to_owned(), id.to_owned());
    off_thread(move || -> Result<PathBuf, String> {
        let exif = library::connect(&library_path)
            .ok()
            .and_then(|db| edit::original_exif(&db, &id))
            .map(|tiff| if output.location { tiff } else { edit::without_location(tiff) });
        let gamut = edit::Gamut::from_icc(icc.as_deref());
        let image = match recipe {
            Some(recipe) => recipe.apply(pixels, gamut),
            None => pixels,
        };
        let image = match output.long_edge {
            Some(edge) => edit::fit(&image, edge),
            None => image,
        };
        let bytes = match output.format {
            Format::Jpeg => edit::encode_jpeg(&image, output.quality.clamp(1, 100), exif.as_deref(), icc.as_deref()),
            Format::Png => edit::encode_png(&image, exif.as_deref(), icc.as_deref()),
            Format::Tiff => edit::encode_tiff(&image, icc.as_deref()),
        };
        let part = target.with_extension("part");
        std::fs::write(&part, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
        Ok(target)
    })
    .await
}
