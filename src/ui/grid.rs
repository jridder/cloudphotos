//! The thumbnail grid used by the library and every album.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::{gdk, gio, glib, prelude::*, subclass::prelude::ObjectSubclassIsExt};

use crate::{
    asset_object::AssetObject,
    library::{PREVIEW, THUMB},
    model::model,
    ui::photo_actions,
};

/// Cell sizes (edge, px) the grid zooms through.
const SIZES: [i32; 7] = [96, 128, 168, 224, 300, 400, 540];
const DEFAULT_ZOOM: usize = 2;
/// Cells bigger than this show the preview; the thumbnail would look soft.
const PREVIEW_ABOVE: i32 = 256;

pub struct PhotoGrid {
    pub root: gtk::Overlay,
    /// Zoom Out / Zoom In buttons, for the header.
    pub zoom_controls: gtk::Box,
    grid: gtk::GridView,
    scroller: gtk::ScrolledWindow,
    month: gtk::Label,
}

/// A grid cell: at least `edge` wide, and as tall as it's wide, so the
/// columns fill the grid's width (the grid sizes rows from the column width).
mod square {
    use std::cell::Cell;

    use gtk::{glib, prelude::*, subclass::prelude::*};

    #[derive(Default)]
    pub struct Square {
        pub edge: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Square {
        const NAME: &'static str = "CloudPhotosSquare";
        type Type = super::Square;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Square {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Square {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let edge = self.edge.get();
            let size = if orientation == gtk::Orientation::Vertical && for_size > 0 { for_size } else { edge };
            (size, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct Square(ObjectSubclass<square::Square>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Square {
    fn new(child: &impl IsA<gtk::Widget>, edge: i32) -> Self {
        let square: Self = glib::Object::new();
        square.imp().edge.set(edge);
        child.set_parent(&square);
        square
    }

    fn set_edge(&self, edge: i32) {
        if self.imp().edge.replace(edge) != edge {
            self.queue_resize();
        }
    }
}

/// How big the cells are, and every cell made, to resize them in place.
struct Zoom {
    level: Cell<usize>,
    cells: RefCell<Vec<(glib::WeakRef<gtk::ListItem>, glib::WeakRef<gtk::Picture>)>>,
    zoom_out: gtk::Button,
    zoom_in: gtk::Button,
}

impl Zoom {
    fn size(&self) -> i32 {
        SIZES[self.level.get()]
    }

    fn large(&self) -> bool {
        self.size() > PREVIEW_ABOVE
    }

    fn step(&self, delta: i32) {
        self.set((self.level.get() as i32 + delta).clamp(0, SIZES.len() as i32 - 1) as usize);
    }

    fn set(&self, level: usize) {
        let level = level.min(SIZES.len() - 1);
        self.zoom_out.set_sensitive(level > 0);
        self.zoom_in.set_sensitive(level + 1 < SIZES.len());
        if level == self.level.get() {
            return;
        }
        // The grid keeps the photos in view where they were as the cells
        // resize, so the scroll position needs no help.
        let was_large = self.large();
        self.level.set(level);
        model().set_setting("grid_zoom", &level.to_string());
        let (size, large) = (self.size(), self.large());
        self.cells.borrow_mut().retain(|(item, picture)| {
            let (Some(item), Some(picture)) = (item.upgrade(), picture.upgrade()) else { return false };
            if let Some(square) = picture.parent().and_then(|cell| cell.parent()).and_downcast::<Square>() {
                square.set_edge(size);
            }
            if large != was_large {
                if let Some(object) = item.item().and_downcast::<AssetObject>() {
                    load_thumb(&item, &picture, object, large);
                }
            }
            true
        });
    }
}

impl PhotoGrid {
    /// `on_menu(object, widget, x, y)` shows the right-click menu for an item.
    pub fn new(
        on_activate: impl Fn(&gio::ListModel, u32) + 'static,
        on_menu: impl Fn(&AssetObject, &gtk::Widget, f64, f64) + 'static,
    ) -> Self {
        let on_menu: Rc<dyn Fn(&AssetObject, &gtk::Widget, f64, f64)> = Rc::new(on_menu);
        let zoom_out = gtk::Button::builder().icon_name("zoom-out-symbolic").tooltip_text("Zoom Out (Ctrl+−)").build();
        let zoom_in = gtk::Button::builder().icon_name("zoom-in-symbolic").tooltip_text("Zoom In (Ctrl++)").build();
        let zoom_controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        zoom_controls.add_css_class("linked");
        zoom_controls.append(&zoom_out);
        zoom_controls.append(&zoom_in);
        let level = model().setting("grid_zoom").and_then(|v| v.parse().ok()).filter(|&l| l < SIZES.len()).unwrap_or(DEFAULT_ZOOM);
        let zoom = Rc::new(Zoom {
            level: Cell::new(level),
            cells: RefCell::default(),
            zoom_out,
            zoom_in,
        });

        let factory = gtk::SignalListItemFactory::new();
        let z = zoom.clone();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).build();
            let cell = gtk::Overlay::builder().child(&picture).build();
            z.cells.borrow_mut().push((item.downgrade(), picture.downgrade()));
            let square = Square::new(&cell, z.size());
            cell.add_css_class("photo-cell");
            let video = badge("media-playback-start-symbolic", gtk::Align::End, gtk::Align::Start);
            let heart = badge("cloudphotos-heart-filled-symbolic", gtk::Align::Start, gtk::Align::End);
            cell.add_overlay(&video);
            cell.add_overlay(&heart);
            // Recently Deleted: days until iCloud removes it.
            let days = gtk::Label::builder()
                .halign(gtk::Align::End)
                .valign(gtk::Align::End)
                .margin_end(6)
                .margin_bottom(6)
                .visible(false)
                .build();
            days.add_css_class("days-left");
            days.add_css_class("caption");
            cell.add_overlay(&days);
            // The heart follows the item's favorite property, through rebinds.
            item.property_expression("item")
                .chain_property::<AssetObject>("favorite")
                .bind(&heart, "visible", gtk::Widget::NONE);
            // An edit changes the thumbnail; GTK keeps the cell bound to the
            // same object, so reload on the item's revision instead.
            let (weak_item, weak_picture, z) = (item.downgrade(), picture.downgrade(), z.clone());
            // GTK holds the watch for as long as it's installed.
            item.property_expression("item").chain_property::<AssetObject>("revision").watch(
                gtk::Widget::NONE,
                move || {
                    let (Some(item), Some(picture)) = (weak_item.upgrade(), weak_picture.upgrade()) else { return };
                    let Some(object) = item.item().and_downcast::<AssetObject>() else { return };
                    if let Some(cell) = picture.parent().and_downcast::<gtk::Overlay>() {
                        show_days_left(&cell, &object);
                    }
                    if object.revision() == 0 {
                        return; // never edited; bind loads it
                    }
                    load_thumb(&item, &picture, object, z.large());
                },
            );
            // Right-click or long-press, and the cell.menu action (keyboard
            // Menu / Shift+F10 from the grid) open the photo's menu.
            let (weak_item, menu) = (item.downgrade(), on_menu.clone());
            photo_actions::on_context_click(&square, move |x, y| {
                let Some(item) = weak_item.upgrade() else { return };
                if let (Some(object), Some(cell)) = (item.item().and_downcast::<AssetObject>(), item.child()) {
                    menu(&object, &cell, x, y);
                }
            });
            let (weak_item, menu) = (item.downgrade(), on_menu.clone());
            let action = gio::SimpleAction::new("menu", None);
            action.connect_activate(move |_, _| {
                let Some(item) = weak_item.upgrade() else { return };
                if let (Some(object), Some(cell)) = (item.item().and_downcast::<AssetObject>(), item.child()) {
                    menu(&object, &cell, cell.width() as f64 / 2.0, cell.height() as f64 / 2.0);
                }
            });
            let actions = gio::SimpleActionGroup::new();
            actions.add_action(&action);
            square.insert_action_group("cell", Some(&actions));
            item.set_child(Some(&square));
        });
        let z = zoom.clone();
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let Some(object) = item.item().and_downcast::<AssetObject>() else { return };
            let square = item.child().and_downcast::<Square>().expect("cell");
            let cell = square.first_child().and_downcast::<gtk::Overlay>().expect("cell");
            let picture = cell.child().and_downcast::<gtk::Picture>().expect("picture");
            let video = cell.first_child().and_then(|w| w.next_sibling()).expect("video badge");
            video.set_visible(object.with(|a| a.is_video()));
            let (filename, taken) = object.with(|a| (a.filename.clone(), a.taken));
            cell.set_tooltip_text(Some(&format!("{filename}\n{}", format_date(taken))));
            show_days_left(&cell, &object);
            // A recycled cell may have been made at another zoom.
            square.set_edge(z.size());

            load_thumb(item, &picture, object, z.large());
        });

        let grid = gtk::GridView::builder()
            .factory(&factory)
            .single_click_activate(true)
            .max_columns(20)
            .min_columns(2)
            .build();
        grid.add_css_class("photo-grid");
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Menu|<Shift>F10"),
            Some(gtk::CallbackAction::new(|grid, _| {
                // The focused cell's content carries the cell.menu action.
                if let Some(cell) = grid.focus_child().and_then(|c| c.first_child()) {
                    let _ = cell.activate_action("cell.menu", None);
                }
                glib::Propagation::Stop
            })),
        ));
        grid.add_controller(shortcuts);
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

        // Zoom: the header buttons, Ctrl+scroll, pinch, and Ctrl+plus/minus/0.
        zoom.set(level);
        let z = zoom.clone();
        zoom.zoom_out.connect_clicked(move |_| z.step(-1));
        let z = zoom.clone();
        zoom.zoom_in.connect_clicked(move |_| z.step(1));
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        // Touchpads scroll in small steps; zoom once they add up to a notch.
        let (z, pending) = (zoom.clone(), Cell::new(0.0));
        wheel.connect_scroll(move |wheel, _, dy| {
            if !wheel.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            pending.set(pending.get() + dy);
            if pending.get().abs() >= 1.0 {
                z.step(if pending.get() < 0.0 { 1 } else { -1 });
                pending.set(0.0);
            }
            glib::Propagation::Stop
        });
        scroller.add_controller(wheel);
        let pinch = gtk::GestureZoom::new();
        let (z, base) = (zoom.clone(), Rc::new(Cell::new(1.0)));
        let b = base.clone();
        pinch.connect_begin(move |_, _| b.set(1.0));
        pinch.connect_scale_changed(move |_, scale| {
            if scale / base.get() > 1.25 {
                z.step(1);
                base.set(scale);
            } else if scale / base.get() < 0.8 {
                z.step(-1);
                base.set(scale);
            }
        });
        scroller.add_controller(pinch);
        let keys = gtk::ShortcutController::new();
        for (trigger, delta) in [("<Control>plus|<Control>equal|<Control>KP_Add", 1), ("<Control>minus|<Control>KP_Subtract", -1)] {
            let z = zoom.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(gtk::CallbackAction::new(move |_, _| {
                    z.step(delta);
                    glib::Propagation::Stop
                })),
            ));
        }
        let z = zoom.clone();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>0|<Control>KP_0"),
            Some(gtk::CallbackAction::new(move |_, _| {
                z.set(DEFAULT_ZOOM);
                glib::Propagation::Stop
            })),
        ));
        root.add_controller(keys);

        let this = Self { root, zoom_controls, grid, scroller, month };
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

/// Show the item's thumbnail (edited if it has an edit) in `picture`; with
/// `large` cells, its preview, the thumbnail standing in until it's loaded.
fn load_thumb(item: &gtk::ListItem, picture: &gtk::Picture, object: AssetObject, large: bool) {
    let model = model();
    let cached = |kind| model.store.cached(&object.id(), crate::model::Model::display_kind(&object, kind));
    let wanted = if large { PREVIEW } else { THUMB };
    if let Some(texture) = cached(wanted) {
        picture.set_paintable(Some(&texture));
        return;
    }
    let stand_in = if large { cached(THUMB) } else { None };
    picture.set_paintable(stand_in.as_ref());
    let item = item.downgrade();
    let picture = picture.downgrade();
    glib::spawn_future_local(async move {
        // The cell may have been recycled for another item meanwhile.
        let show = |texture: Option<gdk::Texture>| {
            let showing = item.upgrade().is_some_and(|item| item.item().as_ref() == Some(object.upcast_ref()));
            if let (true, Some(picture)) = (showing, picture.upgrade()) {
                picture.set_paintable(texture.as_ref());
            }
        };
        if large && stand_in.is_none() {
            if let Some(texture) = model.texture(&object, THUMB).await {
                show(Some(texture));
            }
        }
        let texture = model.texture(&object, wanted).await;
        // A missing preview leaves the thumbnail showing.
        if texture.is_some() || !large {
            show(texture);
        }
    });
}

/// "29 days" on items in Recently Deleted; hidden otherwise.
fn show_days_left(cell: &gtk::Overlay, object: &AssetObject) {
    let mut child = cell.first_child();
    while let Some(widget) = child {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            let text = object.with(|a| a.deleted.then(|| days_left(a.expires)).flatten());
            label.set_visible(text.is_some());
            label.set_label(text.as_deref().unwrap_or(""));
            return;
        }
        child = widget.next_sibling();
    }
}

/// How long until `expires` (Unix seconds), as iPhone's Recently Deleted puts it.
pub fn days_left(expires: Option<f64>) -> Option<String> {
    let now = glib::real_time() as f64 / 1e6;
    let days = ((expires? - now) / 86_400.0).ceil().max(0.0) as i64;
    Some(match days {
        0 => "Today".into(),
        1 => "1 day".into(),
        n => format!("{n} days"),
    })
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
    let first = first_visible(grid, adj).filter(|_| adj.value() >= 1.0);
    let Some(object) = first.and_then(|i| grid.model()?.item(i)).and_downcast::<AssetObject>() else {
        label.set_visible(false);
        return;
    };
    label.set_label(&format_month(object.with(|a| a.taken)));
    label.set_visible(true);
}

/// The item at the start of the first visible row.
fn first_visible(grid: &gtk::GridView, adj: &gtk::Adjustment) -> Option<u32> {
    let model = grid.model()?;
    let cell = grid.first_child()?;
    let (width, height) = (cell.width().max(1), cell.height().max(1));
    let columns = (grid.width() / width).max(1) as u32;
    let row = (adj.value() / height as f64) as u32;
    (model.n_items() > 0).then(|| (row * columns).min(model.n_items() - 1))
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
