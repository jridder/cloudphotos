//! The viewer's Info panel: what we know about one photo or video, from
//! iCloud's records, the original's EXIF, and the local library.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use serde_json::Value;

use crate::{
    asset_object::AssetObject,
    library::{LIVE, ORIGINAL},
    model::model,
    ui::grid::{format_date, local_time},
};

/// Smart albums that say what kind of photo this is.
const TYPE_ALBUMS: [(&str, &str); 5] = [
    ("Panoramas", "Panorama"),
    ("Slo-mo", "Slow Motion"),
    ("Time-lapse", "Time-Lapse"),
    ("Bursts", "Burst"),
    ("Screenshots", "Screenshot"),
];

/// Rebuild `container` for `object`. `still_current` is checked after the
/// slow part (EXIF) so a stale result never lands on the next photo.
pub fn fill(container: &gtk::Box, object: &AssetObject, still_current: Rc<dyn Fn() -> bool>) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let asset = object.asset();
    let details = &asset.details;
    let albums = model().albums();
    let in_album = |name: &str| albums.iter().any(|a| a.kind == "smart" && a.name == name && a.assets.contains(&asset.id));

    // Caption and description, when set on the phone.
    let caption = details.get("caption").and_then(Value::as_str);
    let description = details.get("description").and_then(Value::as_str);
    if caption.is_some() || description.is_some() {
        let group = adw::PreferencesGroup::new();
        if let Some(caption) = caption {
            let label = gtk::Label::builder().label(caption).wrap(true).xalign(0.0).selectable(true).build();
            label.add_css_class("title-3");
            group.add(&label);
        }
        if let Some(description) = description {
            group.add(&gtk::Label::builder().label(description).wrap(true).xalign(0.0).selectable(true).build());
        }
        container.append(&group);
    }

    // When
    let when = adw::PreferencesGroup::builder().title("Date").build();
    when.add(&row("Taken", &format_date(asset.taken)));
    if let Some(offset) = details.get("timezone_offset").and_then(Value::as_i64) {
        if let Some(there) = taken_there(asset.taken, offset) {
            when.add(&row("Local Time Where Taken", &there));
        }
    }
    if asset.added - asset.taken > 3600.0 {
        when.add(&row("Added to iCloud", &format_date(asset.added)));
    }
    container.append(&when);

    // What
    let mut kinds: Vec<&str> = vec![match (asset.is_video(), asset.live) {
        (true, _) => "Video",
        (false, true) => "Live Photo",
        _ => "Photo",
    }];
    kinds.extend(TYPE_ALBUMS.iter().filter(|(album, _)| in_album(album)).map(|(_, label)| *label));
    if details.get("hdr").is_some_and(|v| v.as_i64().unwrap_or(0) > 0) {
        kinds.push("HDR");
    }
    if details.get("edited").is_some() {
        kinds.push("Edited");
    }
    let what = adw::PreferencesGroup::builder().title("Item").build();
    what.add(&row("Type", &kinds.join(" · ")));
    what.add(&row("File", &asset.filename));
    let megapixels = asset.width as f64 * asset.height as f64 / 1e6;
    let dims = if asset.is_video() {
        format!("{} × {}", asset.width, asset.height)
    } else {
        format!("{} × {} · {:.1} MP", asset.width, asset.height, megapixels)
    };
    what.add(&row("Dimensions", &dims));
    let format = asset.resources.get(ORIGINAL).and_then(|r| r.kind.as_deref()).map(file_format).unwrap_or("");
    what.add(&row("Original", &format!("{format} · {}", glib::format_size(asset.size)).trim_start_matches(" · ").to_string()));
    if let Some(seconds) = details.get("duration").and_then(Value::as_f64).filter(|d| *d > 0.0) {
        what.add(&row("Length", &format_duration(seconds)));
    }
    if let Some(live) = asset.resources.get(LIVE) {
        let size = live.size.map(glib::format_size).map(|s| s.to_string()).unwrap_or_default();
        what.add(&row("Live Photo Video", &size));
    }
    container.append(&what);

    // Camera: filled in once the EXIF has been read.
    let camera = adw::PreferencesGroup::builder().title("Camera").visible(false).build();
    container.append(&camera);

    // Where
    let lat = details.get("latitude").and_then(Value::as_f64);
    let lon = details.get("longitude").and_then(Value::as_f64);
    let place = adw::PreferencesGroup::builder().title("Location").visible(false).build();
    container.append(&place);
    if let (Some(lat), Some(lon)) = (lat, lon) {
        fill_location(&place, lat, lon, details.get("altitude").and_then(Value::as_f64));
    }

    // Keywords and albums
    if let Some(keywords) = details.get("keywords").and_then(Value::as_array).filter(|k| !k.is_empty()) {
        let words: Vec<&str> = keywords.iter().filter_map(Value::as_str).collect();
        let group = adw::PreferencesGroup::builder().title("Keywords").build();
        group.add(&row("", &words.join(", ")));
        container.append(&group);
    }
    let user_albums: Vec<String> =
        albums.iter().filter(|a| a.kind == "user" && a.assets.contains(&asset.id)).map(|a| a.name.replace('/', " › ")).collect();
    if !user_albums.is_empty() {
        let group = adw::PreferencesGroup::builder().title("Albums").build();
        for name in user_albums {
            group.add(&adw::ActionRow::builder().title(glib::markup_escape_text(&name)).build());
        }
        container.append(&group);
    }

    // On this computer
    let storage = adw::PreferencesGroup::builder().title("On This Computer").build();
    let stored = row("Original", "Checking…");
    storage.add(&stored);
    container.append(&storage);

    let object = object.clone();
    glib::spawn_future_local(async move {
        let model = model();
        let has_original = model.has_original(&object).await;
        if !still_current() {
            return;
        }
        stored.set_subtitle(if has_original { "Stored in the library" } else { "Not downloaded yet" });
        if !has_original {
            return;
        }
        let exif = model.exif(&object).await;
        if !still_current() || exif.is_null() {
            return;
        }
        fill_camera(&camera, &exif);
        if !place.is_visible() {
            if let (Some(lat), Some(lon)) = (exif.get("latitude").and_then(Value::as_f64), exif.get("longitude").and_then(Value::as_f64)) {
                fill_location(&place, lat, lon, exif.get("altitude").and_then(Value::as_f64));
            }
        }
    });
}

fn fill_camera(group: &adw::PreferencesGroup, exif: &Value) {
    let s = |k: &str| exif.get(k).and_then(Value::as_str).map(str::to_owned);
    let f = |k: &str| exif.get(k).and_then(Value::as_f64);
    let make = s("make").unwrap_or_default();
    let model_name = s("model").unwrap_or_default();
    let camera = if model_name.to_lowercase().starts_with(&make.to_lowercase()) || make.is_empty() {
        model_name
    } else {
        format!("{make} {model_name}")
    };
    if !camera.trim().is_empty() {
        let r = row("Camera", camera.trim());
        if let Some(lens) = s("lens") {
            r.set_tooltip_text(Some(&lens));
        }
        group.add(&r);
    }
    if let Some(lens) = s("lens") {
        group.add(&row("Lens", &lens));
    }
    let mut exposure = Vec::new();
    if let Some(n) = f("f_number") {
        exposure.push(format!("ƒ/{}", trim(n, 1)));
    }
    if let Some(t) = f("exposure_time") {
        exposure.push(if t >= 1.0 { format!("{} s", trim(t, 1)) } else { format!("1/{} s", (1.0 / t).round()) });
    }
    if let Some(iso) = exif.get("iso").and_then(Value::as_u64) {
        exposure.push(format!("ISO {iso}"));
    }
    if let Some(bias) = f("exposure_bias") {
        exposure.push(format!("{:+} EV", trim(bias, 1)));
    }
    if !exposure.is_empty() {
        group.add(&row("Exposure", &exposure.join(" · ")));
    }
    if let Some(mm) = f("focal_length") {
        let equivalent = exif.get("focal_length_35mm").and_then(Value::as_u64).map(|e| format!(" ({e} mm equivalent)")).unwrap_or_default();
        group.add(&row("Focal Length", &format!("{} mm{equivalent}", trim(mm, 1))));
    }
    if let Some(fired) = exif.get("flash_fired").and_then(Value::as_bool) {
        group.add(&row("Flash", if fired { "Fired" } else { "Off" }));
    }
    if let Some(software) = s("software") {
        // iPhones and iPads record just the iOS version ("27.2").
        let ios = make == "Apple" && software.chars().next().is_some_and(|c| c.is_ascii_digit());
        group.add(&row("Software", &if ios { format!("iOS {software}") } else { software }));
    }
    group.set_visible(group.first_child().is_some_and(|_| !camera.trim().is_empty() || !exposure.is_empty()));
}

fn fill_location(group: &adw::PreferencesGroup, lat: f64, lon: f64, altitude: Option<f64>) {
    let mut text = format!(
        "{:.5}° {}, {:.5}° {}",
        lat.abs(),
        if lat >= 0.0 { "N" } else { "S" },
        lon.abs(),
        if lon >= 0.0 { "E" } else { "W" }
    );
    if let Some(alt) = altitude.filter(|a| a.abs() > 0.5) {
        text.push_str(&format!(" · {:.0} m", alt));
    }
    let r = row("Coordinates", &text);
    let open = gtk::Button::builder().icon_name("mark-location-symbolic").tooltip_text("Open in Maps").valign(gtk::Align::Center).build();
    open.add_css_class("flat");
    open.connect_clicked(move |button| {
        let uri = format!("geo:{lat:.6},{lon:.6}");
        let window = button.root().and_downcast::<gtk::Window>();
        gtk::UriLauncher::new(&uri).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
    });
    r.add_suffix(&open);
    group.add(&r);
    group.set_visible(true);
}

fn row(title: &str, value: &str) -> adw::ActionRow {
    let r = adw::ActionRow::builder()
        .title(glib::markup_escape_text(title))
        .subtitle(glib::markup_escape_text(value))
        .subtitle_selectable(true)
        .subtitle_lines(0)
        .build();
    r.add_css_class("property");
    r
}

fn trim(value: f64, decimals: usize) -> String {
    let s = format!("{value:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

fn format_duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn file_format(uti: &str) -> &'static str {
    match uti {
        "public.heic" | "public.heif" => "HEIC",
        "public.jpeg" => "JPEG",
        "public.png" => "PNG",
        "com.apple.quicktime-movie" => "QuickTime movie",
        "public.mpeg-4" => "MPEG-4 video",
        u if u.contains("raw-image") => "RAW",
        _ => "",
    }
}

/// The clock time where the photo was taken, when that differs from here.
fn taken_there(taken: f64, offset_seconds: i64) -> Option<String> {
    let here = local_time(taken)?;
    if here.utc_offset().as_seconds() == offset_seconds {
        return None;
    }
    let zone = glib::TimeZone::from_offset(offset_seconds as i32);
    let there = here.to_timezone(&zone).ok()?;
    let hours = offset_seconds as f64 / 3600.0;
    let label = if hours.fract() == 0.0 { format!("UTC{:+}", hours as i64) } else { format!("UTC{:+.1}", hours) };
    Some(format!("{} ({label})", there.format("%-l:%M %p, %b %-d").ok()?))
}
