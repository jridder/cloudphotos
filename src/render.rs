//! Turning an edit recipe into pixels, from the full-resolution original.
//!
//! Decoding goes through glycin (GNOME's sandboxed image loaders, which
//! handle HEIC, RAW, PNG… and apply EXIF orientation); the transform and
//! JPEG encoding run on a worker thread.

use std::path::{Path, PathBuf};

use gtk::{gio, glib};
use image::RgbaImage;

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
    let result = async {
        let mut image = glycin::Loader::new(gio::File::for_path(&source)).load().await.map_err(|e| format!("Couldn't open the original: {e}"))?;
        let frame = image.next_frame().await.map_err(|e| format!("Couldn't decode the original: {e}"))?;
        let icc = frame.details().color_icc_profile().map(|b| b.to_vec());
        Ok((edit::texture_pixels(&frame.texture()), icc))
    }
    .await;
    let _ = std::fs::remove_file(&source);
    result
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

/// A full-resolution JPEG of the edited photo, carrying the original's
/// EXIF (date, camera, location), written to `target`.
pub async fn render_full(store: &Store, temp: &Path, library_path: &Path, id: &str, recipe: Recipe, target: PathBuf) -> Result<PathBuf, String> {
    let (pixels, icc) = decode_original(store, temp, id).await?;
    let (library_path, id) = (library_path.to_owned(), id.to_owned());
    off_thread(move || -> Result<PathBuf, String> {
        let exif = library::connect(&library_path).ok().and_then(|db| edit::original_exif(&db, &id));
        let gamut = edit::Gamut::from_icc(icc.as_deref());
        let jpeg = edit::encode_jpeg(&recipe.apply(pixels, gamut), 92, exif.as_deref(), icc.as_deref());
        let part = target.with_extension("part");
        std::fs::write(&part, jpeg).map_err(|e| e.to_string())?;
        std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
        Ok(target)
    })
    .await
}
