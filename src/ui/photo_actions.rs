//! What you can do to one photo or video: shared by the right-click menu
//! (grid and viewer) and the viewer's own buttons, so they behave the same
//! everywhere.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{asset_object::AssetObject, library::ORIGINAL, model::model, store, ui::editor};

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

/// Export the original, or for an edited photo ask which version first.
pub fn export(parent: &gtk::Widget, toasts: &adw::ToastOverlay, object: &AssetObject) {
    if object.with(|a| a.edit.is_none()) {
        return export_as(parent, toasts, object.clone(), false);
    }
    let dialog = adw::AlertDialog::builder()
        .heading("Export Which Version?")
        .body("The edited photo is saved as a full-resolution JPEG with the original's date, camera and location.")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("original", "Original"), ("edited", "Edited")]);
    dialog.set_response_appearance("edited", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("edited"));
    let (parent2, toasts, object) = (parent.clone(), toasts.clone(), object.clone());
    dialog.connect_response(None, move |_, response| match response {
        "edited" => export_as(&parent2, &toasts, object.clone(), true),
        "original" => export_as(&parent2, &toasts, object.clone(), false),
        _ => {}
    });
    dialog.present(Some(parent));
}

fn export_as(parent: &gtk::Widget, toasts: &adw::ToastOverlay, object: AssetObject, edited: bool) {
    let filename = object.with(|a| a.filename.clone());
    let name = if edited {
        let stem = std::path::Path::new(&filename).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{stem} (Edited).jpg")
    } else {
        filename
    };
    let dialog = gtk::FileDialog::builder()
        .title(if edited { "Export Edited Photo" } else { "Export Original" })
        .initial_name(name)
        .accept_label("Export")
        .build();
    let root = parent.root().and_downcast::<gtk::Window>();
    let toasts = toasts.clone();
    dialog.save(root.as_ref(), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else { return };
        let Some(path) = file.path() else { return };
        glib::spawn_future_local(async move {
            if edited {
                toast(&toasts, "Exporting…");
                match model().render_edited(&object, path).await {
                    Ok(_) => toast(&toasts, "Exported."),
                    Err(e) => toast(&toasts, &format!("Export failed: {e}")),
                }
                return;
            }
            match model().store.copy_out(&object.id(), ORIGINAL, path).await {
                Ok(_) => toast(&toasts, "Exported."),
                Err(store::Error::Missing) => {
                    toast(&toasts, "The original hasn't been downloaded yet; try again once the sync reaches it.")
                }
                Err(e) => toast(&toasts, &format!("Export failed: {e:?}")),
            }
        });
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
