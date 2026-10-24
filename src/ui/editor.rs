//! The edit page: rotate, flip and crop one photo.

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

pub fn open(nav: &adw::NavigationView, toasts: &adw::ToastOverlay, object: &AssetObject) {
    let canvas = CropCanvas::default();
    canvas.set_recipe(object.with(|a| a.edit).unwrap_or_default());

    let cancel = gtk::Button::with_label("Cancel");
    let done = gtk::Button::with_label("Done");
    done.add_css_class("suggested-action");
    let spinner = adw::Spinner::builder().visible(false).build();
    let header = adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
    header.pack_start(&cancel);
    header.pack_end(&done);
    header.pack_end(&spinner);
    header.set_title_widget(Some(&adw::WindowTitle::new("Edit", &object.with(|a| a.filename.clone()))));

    let tool = |icon: &str, tip: &str| {
        let b = gtk::Button::builder().icon_name(icon).tooltip_text(tip).build();
        b.add_css_class("flat");
        b
    };
    let rotate_left = tool("object-rotate-left-symbolic", "Rotate Left");
    let rotate_right = tool("object-rotate-right-symbolic", "Rotate Right");
    let flip_h = tool("object-flip-horizontal-symbolic", "Flip Horizontally");
    let flip_v = tool("object-flip-vertical-symbolic", "Flip Vertically");
    let aspect = gtk::DropDown::from_strings(&ASPECTS.map(|(label, _)| label));
    aspect.set_tooltip_text(Some("Crop Shape"));
    let reset = gtk::Button::with_label("Reset");
    reset.add_css_class("flat");

    let tools = gtk::Box::builder().spacing(6).halign(gtk::Align::Center).margin_top(6).margin_bottom(6).build();
    for w in [rotate_left.upcast_ref::<gtk::Widget>(), rotate_right.upcast_ref(), flip_h.upcast_ref(), flip_v.upcast_ref()] {
        tools.append(w);
    }
    tools.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    tools.append(&gtk::Image::from_icon_name("cloudphotos-crop-symbolic"));
    tools.append(&aspect);
    tools.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    tools.append(&reset);
    let bottom = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bottom.append(&tools);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.add_bottom_bar(&bottom);
    view.set_content(Some(&canvas));
    view.add_css_class("viewer");
    let page = adw::NavigationPage::builder().title("Edit").tag("editor").child(&view).build();

    // The unedited preview; the recipe is drawn on top of it live.
    let id = object.id();
    let c = canvas.clone();
    glib::spawn_future_local(async move {
        if let Ok(texture) = model().store.texture(&id, PREVIEW).await {
            c.set_texture(Some(&texture));
        }
    });

    let c = canvas.clone();
    rotate_left.connect_clicked(move |_| c.modify(Recipe::rotate_counterclockwise));
    let c = canvas.clone();
    rotate_right.connect_clicked(move |_| c.modify(Recipe::rotate_clockwise));
    let c = canvas.clone();
    flip_h.connect_clicked(move |_| c.modify(Recipe::flip_horizontally));
    let c = canvas.clone();
    flip_v.connect_clicked(move |_| c.modify(Recipe::flip_vertically));
    let c = canvas.clone();
    aspect.connect_selected_notify(move |dd| {
        let (_, ratio) = ASPECTS[dd.selected() as usize];
        let (ow, oh) = c.oriented_size();
        let ratio = ratio.map(|r| match r {
            0.0 => ow / oh,
            r if oh > ow => 1.0 / r, // portrait picture: portrait crop
            r => r,
        });
        c.set_aspect(ratio);
    });
    let (c, a) = (canvas.clone(), aspect.clone());
    reset.connect_clicked(move |_| {
        a.set_selected(0);
        c.set_aspect(None);
        c.set_recipe(Recipe::default());
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
