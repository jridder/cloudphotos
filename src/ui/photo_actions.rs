//! What you can do to one photo or video: shared by the right-click menu
//! (grid and viewer) and the viewer's own buttons, so they behave the same
//! everywhere.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use serde::{Deserialize, Serialize};

use crate::{
    asset_object::AssetObject,
    library::ORIGINAL,
    model::model,
    render::{Format, Output},
    store,
    ui::editor,
};

/// The photo an action is for, and where it's shown.
#[derive(Clone)]
pub struct Target {
    pub nav: adw::NavigationView,
    pub toasts: adw::ToastOverlay,
    /// The collection it was picked from (an album's list drops it on delete).
    pub items: gio::ListModel,
    pub object: AssetObject,
}

fn toast(toasts: &adw::ToastOverlay, message: &str) {
    toasts.add_toast(adw::Toast::new(&glib::markup_escape_text(message)));
}

pub fn set_favorite(object: &AssetObject, value: bool) {
    if object.favorite() == value {
        return;
    }
    let object = object.clone();
    glib::spawn_future_local(async move { model().set_favorite(&object, value).await });
}

pub fn edit(target: &Target) {
    editor::open(&target.nav, &target.toasts, &target.object);
}

/// Sizes offered for export: label and longest side (`None` = full size).
const SIZES: [(&str, Option<u32>); 4] =
    [("Full Size", None), ("Large (2048 px)", Some(2048)), ("Medium (1280 px)", Some(1280)), ("Small (640 px)", Some(640))];
/// JPEG qualities offered.
const QUALITIES: [(&str, u8); 4] = [("Maximum", 100), ("High", 92), ("Medium", 80), ("Low", 65)];
const FORMATS: [(&str, &str, Format); 3] = [
    ("JPEG", "Smaller files, adjustable quality", Format::Jpeg),
    ("PNG", "Lossless, larger files", Format::Png),
    ("TIFF", "Lossless and uncompressed, without camera details", Format::Tiff),
];

/// What the export dialog remembers between exports.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
struct Choices {
    /// Save the original file as stored in iCloud (when not exporting an edit).
    original_file: bool,
    output: Output,
}

/// Ask how to export (version, format, size, quality, location), then
/// where. Videos are saved as they are.
pub fn export(parent: &gtk::Widget, toasts: &adw::ToastOverlay, object: &AssetObject) {
    let (filename, is_video, edited) = object.with(|a| (a.filename.clone(), a.is_video(), a.edit.is_some()));
    if is_video {
        return choose_file(parent, toasts, object.clone(), "Export Video", filename, Job::OriginalFile);
    }
    let choices: Choices = model().setting("export").and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(Choices {
        original_file: true,
        output: Output::default(),
    });
    let extension = std::path::Path::new(&filename).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();

    let version = adw::ComboRow::builder().title("Version").model(&gtk::StringList::new(&["Edited", "Original"])).build();
    version.set_visible(edited);
    let format = adw::ComboRow::builder().title("Format").build();
    let size = adw::ComboRow::builder().title("Size").model(&gtk::StringList::new(&SIZES.map(|(l, _)| l))).build();
    let quality = adw::ComboRow::builder().title("Quality").model(&gtk::StringList::new(&QUALITIES.map(|(l, _)| l))).build();
    let location = adw::SwitchRow::builder().title("Include Location").active(choices.output.location).build();
    size.set_selected(SIZES.iter().position(|(_, e)| *e == choices.output.long_edge).unwrap_or(0) as u32);
    quality.set_selected(QUALITIES.iter().position(|(_, q)| *q == choices.output.quality).unwrap_or(1) as u32);

    // The format list: the original file is a choice only for the original.
    let formats_for = move |original: bool| -> Vec<Option<Format>> {
        let mut list: Vec<Option<Format>> = if original { vec![None] } else { Vec::new() };
        list.extend(FORMATS.iter().map(|(_, _, f)| Some(*f)));
        list
    };
    let listed: Rc<std::cell::RefCell<Vec<Option<Format>>>> = Rc::default();
    let original_label = format!("Original File ({extension})");
    let fill_formats = {
        let (format, listed) = (format.clone(), listed.clone());
        move |original: bool, wanted: Option<Format>| {
            let list = formats_for(original);
            let labels: Vec<&str> = list
                .iter()
                .map(|f| match f {
                    None => original_label.as_str(),
                    Some(f) => FORMATS.iter().find(|(_, _, x)| x == f).map(|(l, _, _)| *l).unwrap_or(""),
                })
                .collect();
            format.set_model(Some(&gtk::StringList::new(&labels)));
            format.set_selected(list.iter().position(|f| *f == wanted).unwrap_or(0) as u32);
            listed.replace(list);
        }
    };
    let chosen_format = {
        let (format, listed) = (format.clone(), listed.clone());
        move || listed.borrow().get(format.selected() as usize).copied().flatten()
    };
    let editing = move |version: &adw::ComboRow| edited && version.selected() == 0;
    fill_formats(!editing(&version), if choices.original_file && !editing(&version) { None } else { Some(choices.output.format) });

    // Rows that don't apply to the chosen format are dimmed.
    let update = {
        let (format, size, quality, location, chosen_format) = (format.clone(), size.clone(), quality.clone(), location.clone(), chosen_format.clone());
        Rc::new(move || {
            let picked = chosen_format();
            format.set_subtitle(match picked {
                None => "Exactly as stored in iCloud",
                Some(f) => FORMATS.iter().find(|(_, _, x)| *x == f).map(|(_, d, _)| *d).unwrap_or(""),
            });
            size.set_sensitive(picked.is_some());
            quality.set_sensitive(picked == Some(Format::Jpeg));
            location.set_sensitive(picked.is_some_and(|f| f != Format::Tiff));
            location.set_subtitle(match picked {
                None => "Kept as in the original file",
                Some(Format::Tiff) => "TIFF exports carry no camera details",
                Some(_) => "The GPS position in the photo's details",
            });
        })
    };
    update();
    let u = update.clone();
    format.connect_selected_notify(move |_| u());
    let (u, fill) = (update.clone(), Rc::new(fill_formats));
    let chosen = chosen_format.clone();
    version.connect_selected_notify(move |version| {
        let keep = chosen().or(Some(Format::Jpeg));
        fill(!editing(version), keep);
        u();
    });

    let group = adw::PreferencesGroup::new();
    group.add(&version);
    group.add(&format);
    let output_group = adw::PreferencesGroup::new();
    output_group.add(&size);
    output_group.add(&quality);
    output_group.add(&location);
    let page = adw::PreferencesPage::new();
    page.add(&group);
    page.add(&output_group);

    let cancel = gtk::Button::with_label("Cancel");
    let go = gtk::Button::with_label("Export…");
    go.add_css_class("suggested-action");
    let header = adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
    header.pack_start(&cancel);
    header.pack_end(&go);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder().title("Export Photo").content_width(440).child(&view).build();

    let d = dialog.clone();
    cancel.connect_clicked(move |_| {
        d.close();
    });
    let (d, parent2, toasts, object) = (dialog.clone(), parent.clone(), toasts.clone(), object.clone());
    go.connect_clicked(move |_| {
        let picked = chosen_format();
        let edit_version = editing(&version);
        let mut choices = choices;
        choices.original_file = picked.is_none();
        if let Some(f) = picked {
            choices.output.format = f;
        }
        choices.output.long_edge = SIZES[size.selected() as usize % SIZES.len()].1;
        choices.output.quality = QUALITIES[quality.selected() as usize % QUALITIES.len()].1;
        choices.output.location = location.is_active();
        if let Ok(value) = serde_json::to_string(&choices) {
            model().set_setting("export", &value);
        }
        d.close();
        let stem = std::path::Path::new(&filename).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match picked {
            None => choose_file(&parent2, &toasts, object.clone(), "Export Original", filename.clone(), Job::OriginalFile),
            Some(f) => {
                let name = format!("{stem}{}.{}", if edit_version { " (Edited)" } else { "" }, f.extension());
                let job = Job::Render { edited: edit_version, output: choices.output };
                choose_file(&parent2, &toasts, object.clone(), if edit_version { "Export Edited Photo" } else { "Export Photo" }, name, job);
            }
        }
    });
    dialog.present(Some(parent));
}

/// What an export writes.
#[derive(Clone, Copy)]
enum Job {
    /// The original file, byte for byte.
    OriginalFile,
    /// Rendered from the original, edited or not.
    Render { edited: bool, output: Output },
}

/// Ask where to save, then write it. Test runs (CLOUDPHOTOS_EXPORT_DIR)
/// skip the file chooser and write into that folder.
fn choose_file(parent: &gtk::Widget, toasts: &adw::ToastOverlay, object: AssetObject, title: &str, name: String, job: Job) {
    let toasts = toasts.clone();
    let write = move |path: std::path::PathBuf| {
        glib::spawn_future_local(async move {
            match job {
                Job::Render { edited, output } => {
                    toast(&toasts, "Exporting…");
                    match model().render(&object, edited, output, path).await {
                        Ok(_) => toast(&toasts, "Exported."),
                        Err(e) => toast(&toasts, &format!("Export failed: {e}")),
                    }
                }
                Job::OriginalFile => match model().store.copy_out(&object.id(), ORIGINAL, path).await {
                    Ok(_) => toast(&toasts, "Exported."),
                    Err(store::Error::Missing) => {
                        toast(&toasts, "The original hasn't been downloaded yet; try again once the sync reaches it.")
                    }
                    Err(e) => toast(&toasts, &format!("Export failed: {e:?}")),
                },
            }
        });
    };
    if let Some(dir) = std::env::var_os("CLOUDPHOTOS_EXPORT_DIR") {
        return write(std::path::PathBuf::from(dir).join(name));
    }
    let dialog = gtk::FileDialog::builder().title(title).initial_name(name).accept_label("Export").build();
    let root = parent.root().and_downcast::<gtk::Window>();
    dialog.save(root.as_ref(), gio::Cancellable::NONE, move |result| {
        if let Some(path) = result.ok().and_then(|file| file.path()) {
            write(path);
        }
    });
}

pub fn save_copy(toasts: &adw::ToastOverlay, object: &AssetObject) {
    let (toasts, object) = (toasts.clone(), object.clone());
    toast(&toasts, "Saving a copy to iCloud…");
    glib::spawn_future_local(async move {
        match model().save_copy_to_icloud(&object).await {
            Ok(()) => toast(&toasts, "Saved to iCloud as a new photo."),
            Err(e) => toast(&toasts, &format!("Couldn't save to iCloud: {e}")),
        }
    });
}

pub fn undo_edit(toasts: &adw::ToastOverlay, object: &AssetObject) {
    let (toasts, object) = (toasts.clone(), object.clone());
    glib::spawn_future_local(async move {
        match model().undo_edit(&object).await {
            Ok(true) => toast(&toasts, if object.with(|a| a.edit.is_none()) { "Back to the original." } else { "Undid the last edit." }),
            Ok(false) => {}
            Err(e) => toast(&toasts, &format!("Couldn't undo: {e}")),
        }
    });
}

pub fn revert(toasts: &adw::ToastOverlay, object: &AssetObject) {
    let (toasts, object) = (toasts.clone(), object.clone());
    glib::spawn_future_local(async move {
        match model().save_edit(&object, crate::edit::Recipe::default()).await {
            Ok(()) => toast(&toasts, "Reverted to the original."),
            Err(e) => toast(&toasts, &format!("Couldn't revert: {e}")),
        }
    });
}

/// Ask, then move it to Recently Deleted; `on_deleted` runs once iCloud
/// has taken it (the viewer moves on to a neighbour).
pub fn confirm_delete(parent: &gtk::Widget, target: &Target, on_deleted: impl Fn() + 'static) {
    let what = if target.object.with(|a| a.is_video()) { "Video" } else { "Photo" };
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Delete This {what}?"))
        .body("It moves to Recently Deleted in iCloud Photos on all your devices, where you can recover it for 30 days.")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    let target = target.clone();
    let on_deleted = Rc::new(on_deleted);
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let (target, on_deleted) = (target.clone(), on_deleted.clone());
        glib::spawn_future_local(async move {
            if !model().delete(&target.object).await {
                return;
            }
            // Album views hold their own list; drop it there too.
            if let Some(store) = target.items.downcast_ref::<gio::ListStore>() {
                if let Some(pos) = store.find(&target.object) {
                    store.remove(pos);
                }
            }
            on_deleted();
            toast(&target.toasts, "Moved to Recently Deleted.");
        });
    });
    dialog.present(Some(parent));
}

/// The right-click menu for `target`, at (x, y) in `anchor`. `open` adds
/// an Open item (the grid); `on_deleted` runs after a delete.
pub fn popup(anchor: &gtk::Widget, x: f64, y: f64, target: Target, open: Option<Rc<dyn Fn()>>, on_deleted: Rc<dyn Fn()>) {
    let asset = target.object.asset();
    let edited = asset.edit.is_some();

    let menu = gio::Menu::new();
    let first = gio::Menu::new();
    if open.is_some() {
        first.append(Some("Open"), Some("photo.open"));
    }
    first.append(Some(if target.object.favorite() { "Remove from Favorites" } else { "Add to Favorites" }), Some("photo.favorite"));
    if !asset.is_video() {
        first.append(Some("Edit"), Some("photo.edit"));
    }
    first.append(Some("Export…"), Some("photo.export"));
    menu.append_section(None, &first);
    if edited || model().can_undo_edit(&target.object) {
        let edits = gio::Menu::new();
        edits.append(Some("Undo Last Edit"), Some("photo.undo-edit"));
        edits.append(Some("Revert to Original"), Some("photo.revert"));
        edits.append(Some("Save Copy to iCloud"), Some("photo.save-copy"));
        menu.append_section(None, &edits);
    }
    let last = gio::Menu::new();
    last.append(Some("Delete…"), Some("photo.delete"));
    menu.append_section(None, &last);

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(anchor);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

    let actions = gio::SimpleActionGroup::new();
    let add = |name: &str, enabled: bool, f: Box<dyn Fn(&Target)>| {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(enabled);
        let target = target.clone();
        action.connect_activate(move |_, _| f(&target));
        actions.add_action(&action);
    };
    if let Some(open) = open {
        add("open", true, Box::new(move |_| open()));
    }
    add("favorite", true, Box::new(|t| set_favorite(&t.object, !t.object.favorite())));
    add("edit", true, Box::new(edit));
    let a = anchor.clone();
    add("export", true, Box::new(move |t| export(&a, &t.toasts, &t.object)));
    add("undo-edit", model().can_undo_edit(&target.object), Box::new(|t| undo_edit(&t.toasts, &t.object)));
    add("revert", edited, Box::new(|t| revert(&t.toasts, &t.object)));
    add("save-copy", edited, Box::new(|t| save_copy(&t.toasts, &t.object)));
    let a = anchor.clone();
    add("delete", true, Box::new(move |t| {
        let on_deleted = on_deleted.clone();
        confirm_delete(&a, t, move || on_deleted());
    }));
    popover.insert_action_group("photo", Some(&actions));

    // Unparent once closed, after the chosen item's action has run.
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

/// Right-click (or long-press) on `widget` calls `show(x, y)`.
pub fn on_context_click(widget: &impl IsA<gtk::Widget>, show: impl Fn(f64, f64) + 'static) {
    let show = Rc::new(show);
    let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
    let s = show.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        s(x, y);
    });
    widget.add_controller(click);
    let long = gtk::GestureLongPress::builder().touch_only(true).build();
    long.connect_pressed(move |_, x, y| show(x, y));
    widget.add_controller(long);
}
