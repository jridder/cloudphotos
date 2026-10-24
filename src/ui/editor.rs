//! The edit page: rotate, flip, crop and color-correct one photo.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use adw::prelude::*;
use gtk::glib;

use crate::{
    asset_object::AssetObject,
    edit::{self, Adjustments, Recipe},
    library::PREVIEW,
    model::model,
    ui::crop::CropCanvas,
};

/// Aspect choices: label and width/height (landscape form; flipped for
/// portrait pictures). `None` = free, `Some(0.0)` = the picture's own.
const ASPECTS: [(&str, Option<f64>); 6] = [
    ("Free", None),
    ("Original", Some(0.0)),
    ("Square", Some(1.0)),
    ("4:3", Some(4.0 / 3.0)),
    ("3:2", Some(1.5)),
    ("16:9", Some(16.0 / 9.0)),
];

/// Undo/redo for the edit in progress: each step is the recipe before it.
struct History {
    canvas: CropCanvas,
    undo: RefCell<Vec<Recipe>>,
    redo: RefCell<Vec<Recipe>>,
    undo_button: gtk::Button,
    redo_button: gtk::Button,
}

impl History {
    fn record(&self, before: Recipe) {
        if before == self.canvas.recipe() {
            return;
        }
        self.undo.borrow_mut().push(before);
        self.redo.borrow_mut().clear();
        self.update();
    }

    /// Change the recipe with `f` as one undoable step.
    fn step(&self, f: impl FnOnce(&CropCanvas)) {
        let before = self.canvas.recipe();
        f(&self.canvas);
        self.record(before);
    }

    fn undo(&self) {
        let Some(previous) = self.undo.borrow_mut().pop() else { return };
        self.redo.borrow_mut().push(self.canvas.recipe());
        self.canvas.set_recipe(previous);
        self.update();
    }

    fn redo(&self) {
        let Some(next) = self.redo.borrow_mut().pop() else { return };
        self.undo.borrow_mut().push(self.canvas.recipe());
        self.canvas.set_recipe(next);
        self.update();
    }

    fn update(&self) {
        self.undo_button.set_sensitive(!self.undo.borrow().is_empty());
        self.redo_button.set_sensitive(!self.redo.borrow().is_empty());
    }
}

/// Sliders: label, range, step, and the field they set. Exposure is in
/// stops; the rest show -100..100.
type Field = fn(&mut Adjustments) -> &mut f32;
const LIGHT: [(&str, f64, f64, Field); 4] = [
    ("Exposure", -3.0, 3.0, |a| &mut a.exposure),
    ("Contrast", -1.0, 1.0, |a| &mut a.contrast),
    ("Highlights", -1.0, 1.0, |a| &mut a.highlights),
    ("Shadows", -1.0, 1.0, |a| &mut a.shadows),
];
const COLOR: [(&str, f64, f64, Field); 5] = [
    ("Saturation", -1.0, 1.0, |a| &mut a.saturation),
    ("Vibrance", -1.0, 1.0, |a| &mut a.vibrance),
    ("Warmth", -1.0, 1.0, |a| &mut a.warmth),
    ("Tint", -1.0, 1.0, |a| &mut a.tint),
    ("Sepia", 0.0, 1.0, |a| &mut a.sepia),
];

/// Edge of the square previews on the Looks buttons, in pixels.
const LOOK_EDGE: u32 = 160;

/// A centered square of `image`, `LOOK_EDGE` on a side.
fn look_square(image: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = image.dimensions();
    let side = w.min(h);
    let square = image::imageops::crop_imm(image, (w - side) / 2, (h - side) / 2, side, side).to_image();
    image::imageops::resize(&square, LOOK_EDGE, LOOK_EDGE, image::imageops::FilterType::Triangle)
}

fn memory_texture(image: image::RgbaImage) -> gtk::gdk::MemoryTexture {
    let (w, h) = image.dimensions();
    let bytes = glib::Bytes::from_owned(image.into_raw());
    gtk::gdk::MemoryTexture::new(w as i32, h as i32, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, w as usize * 4)
}

fn slider_value_label(name: &str, v: f64) -> String {
    if name == "Exposure" { format!("{v:+.2}") } else { format!("{:+.0}", v * 100.0) }
}

/// Live color preview: the editor's picture with the current adjustments,
/// rendered off the main thread from a downscaled copy (latest wins).
struct ColorPreview {
    canvas: CropCanvas,
    base: RefCell<Option<Arc<image::RgbaImage>>>,
    shown: Cell<Option<Adjustments>>,
    busy: Cell<bool>,
    pending: Cell<Option<Adjustments>>,
}

impl ColorPreview {
    fn request(self: &Rc<Self>, adjust: Adjustments) {
        if self.shown.get() == Some(adjust) && !self.busy.get() {
            return;
        }
        if self.busy.get() {
            self.pending.set(Some(adjust));
            return;
        }
        let Some(base) = self.base.borrow().clone() else { return };
        self.busy.set(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let mut image = (*base).clone();
                edit::apply_color(&mut image, &adjust, edit::Gamut::Srgb);
                let _ = tx.send_blocking(image);
            });
            if let Ok(image) = rx.recv().await {
                this.canvas.set_texture(Some(memory_texture(image).upcast_ref()));
                this.shown.set(Some(adjust));
            }
            this.busy.set(false);
            if let Some(next) = this.pending.take() {
                this.request(next);
            }
        });
    }
}

pub fn open(nav: &adw::NavigationView, toasts: &adw::ToastOverlay, object: &AssetObject) {
    let canvas = CropCanvas::default();
    canvas.set_recipe(object.with(|a| a.edit).unwrap_or_default());

    let cancel = gtk::Button::with_label("Cancel");
    let done = gtk::Button::with_label("Done");
    done.add_css_class("suggested-action");
    let spinner = adw::Spinner::builder().visible(false).build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .show_back_button(false) // Cancel does that
        .build();
    header.pack_start(&cancel);
    let undo = gtk::Button::builder().icon_name("edit-undo-symbolic").tooltip_text("Undo").sensitive(false).build();
    let redo = gtk::Button::builder().icon_name("edit-redo-symbolic").tooltip_text("Redo").sensitive(false).build();
    header.pack_start(&undo);
    header.pack_start(&redo);
    header.pack_end(&done);
    header.pack_end(&spinner);
    header.set_title_widget(Some(&adw::WindowTitle::new("Edit", &object.with(|a| a.filename.clone()))));

    // Controls in a panel to the right of the photo, like the Info pane.
    let tool = |icon: &str, label: &str, tip: &str| {
        let content = adw::ButtonContent::builder().icon_name(icon).label(label).build();
        let b = gtk::Button::builder().child(&content).hexpand(true).tooltip_text(tip).build();
        b.add_css_class("flat");
        b
    };
    let pair = |a: &gtk::Button, b: &gtk::Button| {
        let row = gtk::Box::builder().spacing(6).homogeneous(true).build();
        row.add_css_class("card");
        row.append(a);
        row.append(b);
        row
    };
    let rotate_left = tool("object-rotate-left-symbolic", "Left", "Rotate Left");
    let rotate_right = tool("object-rotate-right-symbolic", "Right", "Rotate Right");
    let flip_h = tool("object-flip-horizontal-symbolic", "Horizontal", "Flip Horizontally");
    let flip_v = tool("object-flip-vertical-symbolic", "Vertical", "Flip Vertically");

    let rotate_group = adw::PreferencesGroup::builder().title("Rotate").build();
    rotate_group.add(&pair(&rotate_left, &rotate_right));
    let flip_group = adw::PreferencesGroup::builder().title("Flip").build();
    flip_group.add(&pair(&flip_h, &flip_v));

    // Crop shapes as a list of choices; the first (Free) is selected.
    let crop_group = adw::PreferencesGroup::builder().title("Crop").description("Drag the corners or edges of the box").build();
    let mut shapes: Vec<gtk::CheckButton> = Vec::new();
    for (label, _) in ASPECTS {
        let check = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
        if let Some(first) = shapes.first() {
            check.set_group(Some(first));
        }
        let row = adw::ActionRow::builder().title(label).activatable_widget(&check).build();
        row.add_prefix(&check);
        crop_group.add(&row);
        shapes.push(check);
    }
    shapes[0].set_active(true);

    // Light and Color: one slider per adjustment, double-click to reset.
    let mut sliders: Vec<(gtk::Scale, gtk::Label, &'static str, Field)> = Vec::new();
    let mut slider_group = |title: &str, specs: &[(&'static str, f64, f64, Field)]| {
        let group = adw::PreferencesGroup::builder().title(title).build();
        let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
        list.add_css_class("card");
        for (name, min, max, field) in specs {
            let title_label = gtk::Label::builder().label(*name).xalign(0.0).hexpand(true).build();
            let value = gtk::Label::builder().label(slider_value_label(name, 0.0)).build();
            value.add_css_class("dimmed");
            value.add_css_class("numeric");
            let top = gtk::Box::builder().spacing(6).margin_start(12).margin_end(12).margin_top(8).build();
            top.append(&title_label);
            top.append(&value);
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, *min, *max, (max - min) / 200.0);
            scale.set_value(0.0);
            scale.add_mark(0.0, gtk::PositionType::Bottom, None);
            scale.set_margin_start(6);
            scale.set_margin_end(6);
            scale.set_tooltip_text(Some("Double-click to reset"));
            list.append(&top);
            list.append(&scale);
            sliders.push((scale, value, name, *field));
        }
        group.add(&list);
        group
    };
    // Looks: one-click presets, each previewed on the photo itself.
    let looks_group = adw::PreferencesGroup::builder().title("Looks").build();
    let looks_box = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(3)
        .max_children_per_line(3)
        .column_spacing(4)
        .row_spacing(4)
        .build();
    let mut looks: Vec<(gtk::Button, gtk::Picture)> = Vec::new();
    for (name, _) in edit::LOOKS {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .overflow(gtk::Overflow::Hidden)
            .width_request(72)
            .height_request(72)
            .halign(gtk::Align::Center)
            .build();
        picture.add_css_class("look-thumb");
        let label = gtk::Label::builder().label(name).ellipsize(gtk::pango::EllipsizeMode::End).build();
        label.add_css_class("caption");
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
        content.append(&picture);
        content.append(&label);
        let button = gtk::Button::builder().child(&content).tooltip_text(name).build();
        button.add_css_class("flat");
        button.add_css_class("look");
        looks_box.append(&button);
        looks.push((button, picture));
    }
    looks_group.add(&looks_box);

    let light_group = slider_group("Light", &LIGHT);
    let color_group = slider_group("Color", &COLOR);

    let reset = gtk::Button::builder().label("Reset All").halign(gtk::Align::Center).build();
    reset.add_css_class("pill");

    let panel_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    panel_box.append(&rotate_group);
    panel_box.append(&flip_group);
    panel_box.append(&crop_group);
    panel_box.append(&looks_group);
    panel_box.append(&light_group);
    panel_box.append(&color_group);
    panel_box.append(&reset);
    let panel = gtk::ScrolledWindow::builder()
        .child(&panel_box)
        .width_request(300)
        .hexpand(false)
        .propagate_natural_width(false)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    panel.add_css_class("edit-panel");

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    canvas.add_css_class("viewer");
    body.append(&canvas);
    body.append(&panel);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&body));
    let page = adw::NavigationPage::builder().title("Edit").tag("editor").child(&view).build();

    // The unedited preview; geometry is drawn on top of it live, and color
    // adjustments re-render a downscaled copy in the background.
    let preview = Rc::new(ColorPreview {
        canvas: canvas.clone(),
        base: RefCell::default(),
        shown: Cell::new(None),
        busy: Cell::new(false),
        pending: Cell::new(None),
    });
    let id = object.id();
    let (c, p) = (canvas.clone(), preview.clone());
    let pictures: Vec<gtk::Picture> = looks.iter().map(|(_, picture)| picture.clone()).collect();
    glib::spawn_future_local(async move {
        let Ok(texture) = model().store.texture(&id, PREVIEW).await else { return };
        c.set_texture(Some(&texture));
        let (tx, rx) = async_channel::bounded(1);
        let pixels = edit::texture_pixels(&texture);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(edit::fit(&pixels, 1600));
        });
        let Ok(base) = rx.recv().await else { return };
        let square = look_square(&base);
        p.base.replace(Some(Arc::new(base)));
        p.shown.set(Some(Adjustments::default()));
        p.request(c.recipe().color);

        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let rendered: Vec<image::RgbaImage> = edit::LOOKS
                .iter()
                .map(|(_, look)| {
                    let mut image = square.clone();
                    edit::apply_color(&mut image, look, edit::Gamut::Srgb);
                    image
                })
                .collect();
            let _ = tx.send_blocking(rendered);
        });
        if let Ok(rendered) = rx.recv().await {
            for (picture, image) in pictures.iter().zip(rendered) {
                picture.set_paintable(Some(&memory_texture(image)));
            }
        }
    });

    let history = Rc::new(History {
        canvas: canvas.clone(),
        undo: RefCell::default(),
        redo: RefCell::default(),
        undo_button: undo.clone(),
        redo_button: redo.clone(),
    });
    let h = history.clone();
    undo.connect_clicked(move |_| h.undo());
    let h = history.clone();
    redo.connect_clicked(move |_| h.redo());
    let h = Rc::downgrade(&history);
    canvas.connect_committed(move |before| {
        if let Some(h) = h.upgrade() {
            h.record(before);
        }
    });
    // Sliders <-> recipe. A burst of slider movement is one undo step,
    // recorded once it settles; undo/redo/reset move the sliders back.
    let syncing = Rc::new(Cell::new(false));
    let settle: Rc<RefCell<Option<(Recipe, glib::SourceId)>>> = Rc::default();
    let sliders = Rc::new(sliders);
    for (scale, value, name, field) in sliders.iter() {
        let (c, h, syncing, settle, value, name, field) =
            (canvas.clone(), history.clone(), syncing.clone(), settle.clone(), value.clone(), *name, *field);
        scale.connect_value_changed(move |scale| {
            value.set_label(&slider_value_label(name, scale.value()));
            if syncing.get() {
                return;
            }
            let before = c.recipe();
            let mut recipe = before;
            *field(&mut recipe.color) = scale.value() as f32;
            let first = match settle.borrow_mut().take() {
                Some((first, timer)) => {
                    timer.remove();
                    first
                }
                None => before,
            };
            let (h2, settle2) = (h.clone(), settle.clone());
            let timer = glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
                if let Some((first, _)) = settle2.borrow_mut().take() {
                    h2.record(first);
                }
            });
            settle.borrow_mut().replace((first, timer));
            c.set_recipe(recipe);
        });
        let reset_one = gtk::GestureClick::new();
        let s = scale.clone();
        reset_one.connect_pressed(move |_, n, _, _| {
            if n == 2 {
                s.set_value(0.0);
            }
        });
        scale.add_controller(reset_one);
    }
    // A look replaces the color adjustments (geometry stays) as one step.
    let look_buttons: Vec<gtk::Button> = looks.iter().map(|(button, _)| button.clone()).collect();
    for (button, (_, look)) in look_buttons.iter().zip(edit::LOOKS) {
        let (h, settle) = (history.clone(), settle.clone());
        button.connect_clicked(move |_| {
            // Finish any slider burst first so it stays its own undo step.
            if let Some((first, timer)) = settle.borrow_mut().take() {
                timer.remove();
                h.record(first);
            }
            h.step(|c| c.modify(|r| r.color = look));
        });
    }
    let (p, sl, syncing2) = (preview.clone(), sliders.clone(), syncing.clone());
    let weak_canvas = canvas.downgrade();
    canvas.connect_changed(move || {
        let Some(c) = weak_canvas.upgrade() else { return };
        let mut color = c.recipe().color;
        p.request(color);
        for (button, (_, look)) in look_buttons.iter().zip(edit::LOOKS) {
            if look == color { button.add_css_class("chosen") } else { button.remove_css_class("chosen") }
        }
        syncing2.set(true);
        for (scale, _, _, field) in sl.iter() {
            let v = *field(&mut color) as f64;
            if (scale.value() - v).abs() > 1e-6 {
                scale.set_value(v);
            }
        }
        syncing2.set(false);
    });

    // Show the saved edit's values on the sliders.
    canvas.set_recipe(canvas.recipe());

    let shortcuts = gtk::ShortcutController::new();
    for (trigger, redo) in [("<Control>z", false), ("<Control><Shift>z", true), ("<Control>y", true)] {
        let h = history.clone();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(trigger),
            Some(gtk::CallbackAction::new(move |_, _| {
                if redo { h.redo() } else { h.undo() }
                glib::Propagation::Stop
            })),
        ));
    }

    page.add_controller(shortcuts);

    let h = history.clone();
    rotate_left.connect_clicked(move |_| h.step(|c| c.modify(Recipe::rotate_counterclockwise)));
    let h = history.clone();
    rotate_right.connect_clicked(move |_| h.step(|c| c.modify(Recipe::rotate_clockwise)));
    let h = history.clone();
    flip_h.connect_clicked(move |_| h.step(|c| c.modify(Recipe::flip_horizontally)));
    let h = history.clone();
    flip_v.connect_clicked(move |_| h.step(|c| c.modify(Recipe::flip_vertically)));
    for (index, check) in shapes.iter().enumerate() {
        let c = canvas.clone();
        let h = history.clone();
        check.connect_toggled(move |check| {
            if !check.is_active() {
                return;
            }
            let (_, ratio) = ASPECTS[index];
            let (ow, oh) = c.oriented_size();
            let ratio = ratio.map(|r| match r {
                0.0 => ow / oh,
                r if oh > ow => 1.0 / r, // portrait picture: portrait crop
                r => r,
            });
            h.step(|c| c.set_aspect(ratio));
        });
    }
    let (h, free) = (history.clone(), shapes[0].clone());
    reset.connect_clicked(move |_| {
        h.step(|c| {
            free.set_active(true);
            c.set_aspect(None);
            c.set_recipe(Recipe::default());
        });
    });

    let p = page.clone();
    cancel.connect_clicked(move |_| {
        if let Some(nav) = p.parent().and_downcast::<adw::NavigationView>() {
            nav.pop();
        }
    });

    let (p, c, t, obj) = (page.clone(), canvas.clone(), toasts.clone(), object.clone());
    done.connect_clicked(move |done| {
        done.set_sensitive(false);
        spinner.set_visible(true);
        let (p, c, t, obj, done, spinner) = (p.clone(), c.clone(), t.clone(), obj.clone(), done.clone(), spinner.clone());
        glib::spawn_future_local(async move {
            let recipe = c.recipe();
            match model().save_edit(&obj, recipe).await {
                Ok(()) => {
                    if let Some(nav) = p.parent().and_downcast::<adw::NavigationView>() {
                        nav.pop();
                    }
                    t.add_toast(adw::Toast::new(if recipe.is_identity() { "Reverted to the original." } else { "Edit saved." }));
                }
                Err(e) => {
                    done.set_sensitive(true);
                    spinner.set_visible(false);
                    t.add_toast(adw::Toast::new(&glib::markup_escape_text(&format!("Couldn't save the edit: {e}"))));
                }
            }
        });
    });

    nav.push(&page);
}
