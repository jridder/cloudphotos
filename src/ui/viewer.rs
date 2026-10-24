//! Full-window viewer for one photo or video, with next/previous.

use std::{cell::{Cell, RefCell}, rc::Rc};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{
    asset_object::AssetObject,
    library::{LIVE, ORIGINAL, PREVIEW, THUMB},
    model::model,
    store,
    ui::{grid::format_date, info},
};

struct Viewer {
    page: adw::NavigationPage,
    toasts: adw::ToastOverlay,
    items: gio::ListModel,
    position: Cell<u32>,
    object: RefCell<Option<AssetObject>>,
    title: adw::WindowTitle,
    scroller: gtk::ScrolledWindow,
    picture: gtk::Picture,
    video: gtk::Video,
    stack: gtk::Stack,
    zoom: Cell<f64>,
    favorite: gtk::ToggleButton,
    live: gtk::Button,
    info_box: gtk::Box,
    prev: gtk::Button,
    next: gtk::Button,
    favorite_handler: RefCell<Option<glib::SignalHandlerId>>,
    loading: Cell<u64>,
    /// The Live Photo clip already loaded for the current item, replayed on
    /// later clicks rather than fetched again.
    live_media: RefCell<Option<(String, gtk::MediaFile)>>,
    live_pending: Cell<bool>,
}

pub fn open(nav: &adw::NavigationView, toasts: &adw::ToastOverlay, items: &gio::ListModel, position: u32) {
    let title = adw::WindowTitle::new("", "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.add_css_class("flat");

    let favorite = gtk::ToggleButton::builder().icon_name("cloudphotos-heart-symbolic").tooltip_text("Favorite").build();
    favorite.connect_active_notify(|b| {
        b.set_icon_name(if b.is_active() { "cloudphotos-heart-filled-symbolic" } else { "cloudphotos-heart-symbolic" });
    });
    let info_button = gtk::ToggleButton::builder().icon_name("dialog-information-symbolic").tooltip_text("Info").build();
    let export = gtk::Button::builder().icon_name("document-save-symbolic").tooltip_text("Export Original…").build();
    let delete = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Delete").build();
    let live = gtk::Button::builder().label("LIVE").tooltip_text("Play Live Photo").visible(false).build();
    live.add_css_class("live-button");
    header.pack_end(&delete);
    header.pack_end(&info_button);
    header.pack_end(&export);
    header.pack_end(&favorite);
    header.pack_start(&live);

    let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).can_shrink(true).build();
    let scroller = gtk::ScrolledWindow::builder().child(&picture).hexpand(true).vexpand(true).build();
    let video = gtk::Video::builder().autoplay(true).hexpand(true).vexpand(true).build();
    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    stack.add_named(&scroller, Some("picture"));
    stack.add_named(&video, Some("video"));

    let prev = nav_button("go-previous-symbolic", gtk::Align::Start);
    let next = nav_button("go-next-symbolic", gtk::Align::End);
    let overlay = gtk::Overlay::builder().child(&stack).build();
    overlay.add_overlay(&prev);
    overlay.add_overlay(&next);
    overlay.add_css_class("viewer");

    let info_inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    // A fixed-width sidebar: long values (lens names) wrap rather than widen it.
    let info_box = gtk::ScrolledWindow::builder()
        .child(&info_inner)
        .width_request(340)
        .hexpand(false)
        .propagate_natural_width(false)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let info = gtk::Revealer::builder()
        .child(&info_box)
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .build();
    info_button.bind_property("active", &info, "reveal-child").build();

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&overlay);
    body.append(&info);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&body));
    let page = adw::NavigationPage::builder().title("Photo").tag("viewer").child(&view).build();

    let viewer = Rc::new(Viewer {
        page: page.clone(),
        toasts: toasts.clone(),
        items: items.clone(),
        position: Cell::new(position),
        object: RefCell::default(),
        title,
        scroller,
        picture,
        video,
        stack,
        zoom: Cell::new(0.0),
        favorite,
        live,
        info_box: info_inner.clone(),
        prev,
        next,
        favorite_handler: RefCell::default(),
        loading: Cell::new(0),
        live_media: RefCell::default(),
        live_pending: Cell::new(false),
    });

    let v = viewer.clone();
    viewer.prev.connect_clicked(move |_| v.step(-1));
    let v = viewer.clone();
    viewer.next.connect_clicked(move |_| v.step(1));
    let v = viewer.clone();
    viewer.favorite.connect_toggled(move |button| v.toggle_favorite(button.is_active()));
    let v = viewer.clone();
    viewer.live.connect_clicked(move |_| v.play_live());
    let v = viewer.clone();
    export.connect_clicked(move |_| v.export());
    let v = viewer.clone();
    delete.connect_clicked(move |_| v.confirm_delete());

    // Keyboard: arrows step, Escape closes.
    let keys = gtk::EventControllerKey::new();
    let v = viewer.clone();
    keys.connect_key_pressed(move |_, key, _, _| match key {
        gdk::Key::Left => {
            v.step(-1);
            glib::Propagation::Stop
        }
        gdk::Key::Right => {
            v.step(1);
            glib::Propagation::Stop
        }
        gdk::Key::Escape => {
            v.close();
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    page.add_controller(keys);

    // Double-click toggles fit / 100 %; Ctrl+scroll zooms.
    let clicks = gtk::GestureClick::new();
    let v = viewer.clone();
    clicks.connect_pressed(move |_, n, _, _| {
        if n == 2 {
            v.set_zoom(if v.zoom.get() == 0.0 { 1.0 } else { 0.0 });
        }
    });
    viewer.scroller.add_controller(clicks);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let v = viewer.clone();
    scroll.connect_scroll(move |controller, _, dy| {
        if !controller.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        let current = if v.zoom.get() == 0.0 { v.fit_scale() } else { v.zoom.get() };
        let target = (current * if dy < 0.0 { 1.25 } else { 0.8 }).clamp(0.05, 8.0);
        v.set_zoom(if target <= v.fit_scale() { 0.0 } else { target });
        glib::Propagation::Stop
    });
    viewer.scroller.add_controller(scroll);

    let v = viewer.clone();
    page.connect_hidden(move |_| {
        v.video.set_media_stream(None::<&gtk::MediaStream>);
    });

    viewer.show(position);
    nav.push(&page);
    page.grab_focus();
}

fn nav_button(icon: &str, halign: gtk::Align) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .halign(halign)
        .valign(gtk::Align::Center)
        .margin_start(12)
        .margin_end(12)
        .build();
    button.add_css_class("osd");
    button.add_css_class("circular");
    button
}

impl Viewer {
    fn close(&self) {
        if let Some(nav) = self.page.parent().and_downcast::<adw::NavigationView>() {
            nav.pop();
        }
    }

    fn step(self: &Rc<Self>, delta: i32) {
        let target = self.position.get() as i64 + delta as i64;
        if target >= 0 && (target as u32) < self.items.n_items() {
            self.show(target as u32);
        }
    }

    fn show(self: &Rc<Self>, position: u32) {
        let Some(object) = self.items.item(position).and_downcast::<AssetObject>() else {
            self.close();
            return;
        };
        self.position.set(position);
        self.prev.set_visible(position > 0);
        self.next.set_visible(position + 1 < self.items.n_items());

        if let (Some(old), Some(handler)) = (self.object.borrow().as_ref(), self.favorite_handler.take()) {
            old.disconnect(handler);
        }
        let favorite = self.favorite.clone();
        let handler = object.connect_favorite_notify(move |o| {
            if favorite.is_active() != o.favorite() {
                favorite.set_active(o.favorite());
            }
        });
        self.favorite_handler.replace(Some(handler));
        self.object.replace(Some(object.clone()));
        self.favorite.set_active(object.favorite());

        let asset = object.asset();
        self.title.set_title(&asset.filename);
        self.title.set_subtitle(&format_date(asset.taken));
        self.page.set_title(&asset.filename);
        self.live.set_visible(asset.live && asset.resources.contains_key(LIVE));
        self.fill_info(&object);
        self.set_zoom(0.0);
        self.video.set_media_stream(None::<&gtk::MediaStream>);
        self.live_media.replace(None);
        self.stack.set_visible_child_name("picture");

        // Thumbnail at once, then the sharper preview.
        let generation = self.loading.get() + 1;
        self.loading.set(generation);
        let model = model();
        self.picture.set_paintable(model.store.cached(&object.id(), THUMB).as_ref());
        let this = self.clone();
        glib::spawn_future_local(async move {
            if this.picture.paintable().is_none() {
                if let Some(texture) = model.texture(&object, THUMB).await {
                    if this.loading.get() == generation {
                        this.picture.set_paintable(Some(&texture));
                    }
                }
            }
            if let Some(texture) = model.texture(&object, PREVIEW).await {
                if this.loading.get() == generation {
                    this.picture.set_paintable(Some(&texture));
                }
            }
            if object.with(|a| a.is_video()) && this.loading.get() == generation {
                this.play(&object, ORIGINAL, false).await;
            }
        });
    }

    async fn play(&self, object: &AssetObject, rendition: &str, live: bool) {
        // Playback reads a temporary copy pulled out of the library.
        let file = match model().store.playable(&object.id(), rendition).await {
            Ok(path) => gio::File::for_path(path),
            Err(store::Error::Missing) => {
                self.toast("This video hasn't been downloaded yet — it will play once the sync reaches it.");
                return;
            }
            Err(e) => {
                self.toast(&format!("Couldn't play it: {e:?}"));
                return;
            }
        };
        // The viewer may have moved on meanwhile.
        if self.object.borrow().as_ref() != Some(object) {
            return;
        }
        let media = gtk::MediaFile::for_file(&file);
        if live {
            let stack = self.stack.clone();
            media.connect_ended_notify(move |m| {
                if m.is_ended() {
                    stack.set_visible_child_name("picture");
                }
            });
            self.live_media.replace(Some((object.id(), media.clone())));
        }
        self.video.set_media_stream(Some(&media));
        self.stack.set_visible_child_name("video");
    }

    fn play_live(self: &Rc<Self>) {
        let Some(object) = self.object.borrow().clone() else { return };
        let loaded = self.live_media.borrow().as_ref().filter(|(id, _)| *id == object.id()).map(|(_, m)| m.clone());
        if let Some(media) = loaded {
            media.seek(0);
            media.play();
            self.stack.set_visible_child_name("video");
            return;
        }
        if self.live_pending.replace(true) {
            return; // already fetching it
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.play(&object, LIVE, true).await;
            this.live_pending.set(false);
        });
    }

    fn fit_scale(&self) -> f64 {
        let Some(paintable) = self.picture.paintable() else { return 1.0 };
        let (w, h) = (paintable.intrinsic_width().max(1) as f64, paintable.intrinsic_height().max(1) as f64);
        (self.scroller.width() as f64 / w).min(self.scroller.height() as f64 / h).min(1.0)
    }

    /// 0 = fit the window; otherwise a scale of the image's own size.
    fn set_zoom(&self, zoom: f64) {
        self.zoom.set(zoom);
        if zoom == 0.0 {
            self.picture.set_can_shrink(true);
            self.picture.set_size_request(-1, -1);
            self.scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        } else if let Some(paintable) = self.picture.paintable() {
            self.picture.set_can_shrink(false);
            let (w, h) = (paintable.intrinsic_width() as f64 * zoom, paintable.intrinsic_height() as f64 * zoom);
            self.picture.set_size_request(w as i32, h as i32);
            self.scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        }
    }

    fn fill_info(self: &Rc<Self>, object: &AssetObject) {
        let weak = Rc::downgrade(self);
        let id = object.id();
        let still_current = Rc::new(move || {
            weak.upgrade().is_some_and(|v| v.object.borrow().as_ref().is_some_and(|o| o.id() == id))
        });
        info::fill(&self.info_box, object, still_current);
    }

    fn toggle_favorite(self: &Rc<Self>, value: bool) {
        let Some(object) = self.object.borrow().clone() else { return };
        if object.favorite() == value {
            return;
        }
        glib::spawn_future_local(async move { model().set_favorite(&object, value).await });
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(&glib::markup_escape_text(message)));
    }

    fn export(self: &Rc<Self>) {
        let Some(object) = self.object.borrow().clone() else { return };
        let dialog = gtk::FileDialog::builder()
            .title("Export Original")
            .initial_name(object.with(|a| a.filename.clone()))
            .accept_label("Export")
            .build();
        let root = self.page.root().and_downcast::<gtk::Window>();
        let this = self.clone();
        dialog.save(root.as_ref(), gio::Cancellable::NONE, move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            glib::spawn_future_local(async move {
                match model().store.copy_out(&object.id(), ORIGINAL, path).await {
                    Ok(_) => this.toast("Exported."),
                    Err(store::Error::Missing) => {
                        this.toast("The original hasn't been downloaded yet; try again once the sync reaches it.")
                    }
                    Err(e) => this.toast(&format!("Export failed: {e:?}")),
                }
            });
        });
    }

    fn confirm_delete(self: &Rc<Self>) {
        let Some(object) = self.object.borrow().clone() else { return };
        let what = if object.with(|a| a.is_video()) { "Video" } else { "Photo" };
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Delete This {what}?"))
            .body("It moves to Recently Deleted in iCloud Photos on all your devices, where you can recover it for 30 days.")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            let this = this.clone();
            let object = object.clone();
            glib::spawn_future_local(async move {
                if !model().delete(&object).await {
                    return;
                }
                // Album views hold their own list; drop it there too.
                if let Some(store) = this.items.downcast_ref::<gio::ListStore>() {
                    if let Some(pos) = store.find(&object) {
                        store.remove(pos);
                    }
                }
                let position = this.position.get().min(this.items.n_items().saturating_sub(1));
                if this.items.n_items() == 0 {
                    this.close();
                } else {
                    this.show(position);
                }
                this.toast("Moved to Recently Deleted.");
            });
        });
        dialog.present(Some(&self.page));
    }
}
