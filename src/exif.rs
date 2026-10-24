//! Camera details from an original's EXIF (HEIC, JPEG, PNG, TIFF), read in
//! place from the library blob.

use std::io::BufReader;

use exif::{In, Reader, Tag, Value};
use serde_json::{Map, Value as Json, json};

use crate::library;

/// Read and summarize the EXIF of a stored original. `Null` when it has none.
pub fn read(db: &rusqlite::Connection, id: &str) -> Json {
    let Ok(blob) = library::open_blob(db, id, library::ORIGINAL) else { return Json::Null };
    let Ok(exif) = Reader::new().read_from_container(&mut BufReader::new(blob)) else { return Json::Null };
    let mut out = Map::new();

    let text = |tag: Tag| {
        exif.get_field(tag, In::PRIMARY).map(|f| match &f.value {
            Value::Ascii(parts) => parts
                .iter()
                .map(|p| String::from_utf8_lossy(p).trim_matches(char::from(0)).trim().to_owned())
                .collect::<Vec<_>>()
                .join(" "),
            _ => f.display_value().to_string(),
        })
    };
    let rational = |tag: Tag| {
        exif.get_field(tag, In::PRIMARY).and_then(|f| match &f.value {
            Value::Rational(v) if !v.is_empty() && v[0].denom != 0 => Some(v[0].to_f64()),
            Value::SRational(v) if !v.is_empty() && v[0].denom != 0 => Some(v[0].to_f64()),
            _ => None,
        })
    };
    let uint = |tag: Tag| exif.get_field(tag, In::PRIMARY).and_then(|f| f.value.get_uint(0));

    for (key, tag) in [
        ("make", Tag::Make),
        ("model", Tag::Model),
        ("lens", Tag::LensModel),
        ("software", Tag::Software),
        ("date_original", Tag::DateTimeOriginal),
        ("offset_original", Tag::OffsetTimeOriginal),
    ] {
        if let Some(value) = text(tag).filter(|v| !v.is_empty()) {
            out.insert(key.into(), json!(value));
        }
    }
    if let Some(f) = rational(Tag::FNumber) {
        out.insert("f_number".into(), json!(f));
    }
    if let Some(t) = rational(Tag::ExposureTime) {
        out.insert("exposure_time".into(), json!(t));
    }
    if let Some(iso) = uint(Tag::PhotographicSensitivity) {
        out.insert("iso".into(), json!(iso));
    }
    if let Some(mm) = rational(Tag::FocalLength) {
        out.insert("focal_length".into(), json!(mm));
    }
    if let Some(mm) = uint(Tag::FocalLengthIn35mmFilm) {
        out.insert("focal_length_35mm".into(), json!(mm));
    }
    if let Some(bias) = rational(Tag::ExposureBiasValue).filter(|b| b.abs() > 0.001) {
        out.insert("exposure_bias".into(), json!(bias));
    }
    if let Some(flash) = uint(Tag::Flash) {
        out.insert("flash_fired".into(), json!(flash & 1 == 1));
    }
    if let (Some(lat), Some(lon)) = (gps(&exif, Tag::GPSLatitude, Tag::GPSLatitudeRef, 'S'), gps(&exif, Tag::GPSLongitude, Tag::GPSLongitudeRef, 'W')) {
        out.insert("latitude".into(), json!(lat));
        out.insert("longitude".into(), json!(lon));
        if let Some(alt) = rational(Tag::GPSAltitude) {
            out.insert("altitude".into(), json!(alt));
        }
    }
    if out.is_empty() { Json::Null } else { Json::Object(out) }
}

/// Degrees/minutes/seconds to signed decimal degrees.
fn gps(exif: &exif::Exif, tag: Tag, reference: Tag, negative: char) -> Option<f64> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    let Value::Rational(parts) = &field.value else { return None };
    if parts.len() < 3 || parts.iter().any(|p| p.denom == 0) {
        return None;
    }
    let degrees = parts[0].to_f64() + parts[1].to_f64() / 60.0 + parts[2].to_f64() / 3600.0;
    let sign = exif
        .get_field(reference, In::PRIMARY)
        .map(|f| f.display_value().to_string())
        .is_some_and(|r| r.starts_with(negative));
    Some(if sign { -degrees } else { degrees })
}

#[cfg(test)]
mod tests {
    /// Opt-in check against a real library (read-only):
    /// CLOUDPHOTOS_TEST_LIBRARY=path cargo test exif_on_real_library -- --ignored --nocapture
    #[test]
    #[ignore]
    fn exif_on_real_library() {
        let Some(path) = std::env::var_os("CLOUDPHOTOS_TEST_LIBRARY") else { return };
        let db = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let rows: Vec<(String, String)> = db
            .prepare("SELECT b.asset_id, r.uti FROM blobs b JOIN resources r USING (asset_id, kind) WHERE b.kind = 'original' ORDER BY b.rowid LIMIT 400")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))
            .unwrap()
            .flatten()
            .collect();
        let (mut with, mut gps) = (0, 0);
        let mut by_type = std::collections::BTreeMap::<String, (u32, u32)>::new();
        let mut shown = 0;
        for (id, uti) in &rows {
            let started = std::time::Instant::now();
            let exif = super::read(&db, id);
            let entry = by_type.entry(uti.clone()).or_default();
            entry.0 += 1;
            if !exif.is_null() {
                with += 1;
                entry.1 += 1;
                if exif.get("latitude").is_some() {
                    gps += 1;
                }
                if shown < 3 {
                    shown += 1;
                    println!("{uti} in {:?}: {exif}", started.elapsed());
                }
            }
        }
        println!("{} originals: {with} with EXIF, {gps} with GPS; by type (count, with EXIF): {by_type:?}", rows.len());
    }
}
