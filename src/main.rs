//! Cloud Photos: browse your iCloud Photos library on GNOME.
//!
//! The interface, library index, caching and downloads are Rust; talking to
//! Apple goes through a small Python helper (helper/photos_helper.py) that
//! reuses icloud-linux's sign-in.

mod asset_object;
mod helper;
mod images;
mod library;
mod model;
mod script;
mod ui;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "com.jridder.CloudPhotos";

thread_local! {
    // The window's state lives as long as the app.
    static WINDOW: std::cell::RefCell<Option<std::rc::Rc<ui::window::Window>>> = const { std::cell::RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    glib::set_application_name("Cloud Photos");
    let mut flags = gio::ApplicationFlags::empty();
    // Fake-library runs (tests, screenshots) are their own instance.
    if std::env::var_os("CLOUDPHOTOS_FAKE").is_some() {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_startup(|_| {
        let display = gdk::Display::default().expect("a display");
        // Running from the source tree; the Flatpak installs these into hicolor.
        let icons = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/icons");
        if icons.is_dir() {
            gtk::IconTheme::for_display(&display).add_search_path(icons);
        }
        let css = gtk::CssProvider::new();
        css.load_from_string(include_str!("style.css"));
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        model::model().start();
    });
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let win = ui::window::build(app);
        win.window.present();
        script::run(win.window.clone());
        WINDOW.with(|w| w.replace(Some(win)));
    });
    app.run()
}
