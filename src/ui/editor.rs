//! The edit page: rotate, flip and crop one photo.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::glib;

use crate::{
    asset_object::AssetObject,
    edit::Recipe,
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

    // The unedited preview; the recipe is drawn on top of it live.
    let id = object.id();
    let c = canvas.clone();
    glib::spawn_future_local(async move {
        if let Ok(texture) = model().store.texture(&id, PREVIEW).await {
            c.set_texture(Some(&texture));
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
