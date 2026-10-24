//! The thumbnail grid used by the library and every album.

use gtk::{gdk, gio, glib, prelude::*};

use crate::{asset_object::AssetObject, model::model};

pub const CELL: i32 = 168;

pub struct PhotoGrid {
    pub root: gtk::Overlay,
    grid: gtk::GridView,
    scroller: gtk::ScrolledWindow,
    month: gtk::Label,
}

impl PhotoGrid {
    pub fn new(on_activate: impl Fn(&gio::ListModel, u32) + 'static) -> Self {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).build();
            let cell = gtk::Overlay::builder().child(&picture).width_request(CELL).height_request(CELL).build();
            let square = gtk::AspectFrame::builder().ratio(1.0).obey_child(false).child(&cell).build();
            cell.add_css_class("photo-cell");
            let video = badge("media-playback-start-symbolic", gtk::Align::End, gtk::Align::Start);
            let heart = badge("cloudphotos-heart-filled-symbolic", gtk::Align::Start, gtk::Align::End);
            cell.add_overlay(&video);
            cell.add_overlay(&heart);
            // The heart follows the item's favorite property, through rebinds.
            item.property_expression("item")
                .chain_property::<AssetObject>("favorite")
                .bind(&heart, "visible", gtk::Widget::NONE);
            // An edit changes the thumbnail; GTK keeps the cell bound to the
            // same object, so reload on the item's revision instead.
            let (weak_item, weak_picture) = (item.downgrade(), picture.downgrade());
            // GTK holds the watch for as long as it's installed.
            item.property_expression("item").chain_property::<AssetObject>("revision").watch(
                gtk::Widget::NONE,
                move || {
                    let (Some(item), Some(picture)) = (weak_item.upgrade(), weak_picture.upgrade()) else { return };
                    let Some(object) = item.item().and_downcast::<AssetObject>() else { return };
                    if object.revision() == 0 {
                        return; // never edited; bind loads it
                    }
                    load_thumb(&item, &picture, object);
                },
            );
            item.set_child(Some(&square));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let Some(object) = item.item().and_downcast::<AssetObject>() else { return };
            let square = item.child().and_downcast::<gtk::AspectFrame>().expect("cell");
            let cell = square.child().and_downcast::<gtk::Overlay>().expect("cell");
            let picture = cell.child().and_downcast::<gtk::Picture>().expect("picture");
            let video = cell.first_child().and_then(|w| w.next_sibling()).expect("video badge");
            video.set_visible(object.with(|a| a.is_video()));
            let (filename, taken) = object.with(|a| (a.filename.clone(), a.taken));
            cell.set_tooltip_text(Some(&format!("{filename}\n{}", format_date(taken))));

            load_thumb(item, &picture, object);
        });

        let grid = gtk::GridView::builder()
            .factory(&factory)
            .single_click_activate(true)
            .max_columns(20)
            .min_columns(2)
            .build();
        grid.add_css_class("photo-grid");
        grid.connect_activate(move |grid, position| {
            if let Some(model) = grid.model() {
                on_activate(&model.upcast(), position);
            }
        });

        let scroller = gtk::ScrolledWindow::builder().child(&grid).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let month = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_start(18)
            .margin_top(12)
            .visible(false)
            .build();
        month.add_css_class("month-label");
        month.add_css_class("osd");
        let root = gtk::Overlay::builder().child(&scroller).build();
        root.add_overlay(&month);

        let this = Self { root, grid, scroller, month };
        let (grid, month) = (this.grid.clone(), this.month.clone());
        this.scroller.vadjustment().connect_value_changed(move |adj| update_month(&grid, &month, adj));
        this
    }

    pub fn set_model(&self, items: &impl IsA<gio::ListModel>) {
        self.grid.set_model(Some(&gtk::NoSelection::new(Some(items.clone()))));
        self.scroller.vadjustment().set_value(0.0);
        update_month(&self.grid, &self.month, &self.scroller.vadjustment());
    }
}

/// Show the item's thumbnail (edited if it has an edit) in `picture`.
fn load_thumb(item: &gtk::ListItem, picture: &gtk::Picture, object: AssetObject) {
    let model = model();
    match model.store.cached(&object.id(), crate::model::Model::display_kind(&object, crate::library::THUMB)) {
        Some(texture) => picture.set_paintable(Some(&texture)),
        None => {
            picture.set_paintable(None::<&gdk::Paintable>);
            let item = item.downgrade();
            let picture = picture.downgrade();
            glib::spawn_future_local(async move {
                let texture = model.texture(&object, crate::library::THUMB).await;
                // The cell may have been recycled for another item meanwhile.
                let (Some(item), Some(picture)) = (item.upgrade(), picture.upgrade()) else { return };
                if item.item().as_ref() == Some(object.upcast_ref()) {
                    picture.set_paintable(texture.as_ref());
                }
            });
        }
    }
}

fn badge(icon: &str, halign: gtk::Align, valign: gtk::Align) -> gtk::Image {
    let image = gtk::Image::builder()
        .icon_name(icon)
        .halign(halign)
        .valign(valign)
        .margin_start(6)
        .margin_end(6)
        .margin_top(6)
        .margin_bottom(6)
        .visible(false)
        .build();
    image.add_css_class("badge");
    image
}

/// Show the month of the first visible row while scrolled down.
fn update_month(grid: &gtk::GridView, label: &gtk::Label, adj: &gtk::Adjustment) {
    let Some(model) = grid.model() else { return };
    if adj.value() < 1.0 || model.n_items() == 0 {
        label.set_visible(false);
        return;
    }
    let cell = grid.first_child().map(|c| (c.width().max(1), c.height().max(1))).unwrap_or((CELL, CELL));
    let columns = (grid.width() / cell.0).max(1) as u32;
    let row = (adj.value() / cell.1 as f64) as u32;
    let index = (row * columns).min(model.n_items() - 1);
    let Some(object) = model.item(index).and_downcast::<AssetObject>() else { return };
    label.set_label(&format_month(object.with(|a| a.taken)));
    label.set_visible(true);
}

pub fn local_time(timestamp: f64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(timestamp as i64).ok()
}

pub fn format_month(timestamp: f64) -> String {
    local_time(timestamp).and_then(|t| t.format("%B %Y").ok()).map(Into::into).unwrap_or_default()
}

pub fn format_date(timestamp: f64) -> String {
    local_time(timestamp).and_then(|t| t.format("%A, %B %-d, %Y · %-l:%M %p").ok()).map(Into::into).unwrap_or_default()
}
