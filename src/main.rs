//! Cloud Photos: browse your iCloud Photos library on GNOME.
//!
//! The interface, library index, caching and downloads are Rust; talking to
//! Apple goes through a small Python helper (helper/photos_helper.py) that
//! reuses icloud-linux's sign-in (on Windows, signs in itself).

// Release builds on Windows are a GUI app: no console window behind it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod asset_object;
mod edit;
mod exif;
mod helper;
mod library;
mod model;
mod render;
mod script;
mod store;
mod ui;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "com.jridder.CloudPhotos";

thread_local! {
    // The window's state lives as long as the app.
    static WINDOW: std::cell::RefCell<Option<std::rc::Rc<ui::window::Window>>> = const { std::cell::RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    // Video frames from GStreamer are GL textures. GTK's Vulkan renderer has
    // to download them through a GL context, and on NVIDIA that context can
    // fail to become current, which aborts the app (epoxy "Couldn't find
    // current GLX or EGL context"). The GL renderer draws them directly.
    if cfg!(not(windows)) && std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: first thing in main, before GTK or any other thread starts.
        unsafe { std::env::set_var("GSK_RENDERER", "gl") };
    }
    glib::set_application_name("Cloud Photos");
    let mut flags = gio::ApplicationFlags::empty();
    // Test runs (fake or other library, offline, scripted) are their own
    // instance, never a message to the app you have open.
    let test_run = ["CLOUDPHOTOS_FAKE", "CLOUDPHOTOS_LIBRARY", "CLOUDPHOTOS_OFFLINE", "CLOUDPHOTOS_SCRIPT"]
        .iter()
        .any(|v| std::env::var_os(v).is_some());
    if test_run {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_startup(|_| {
        let display = gdk::Display::default().expect("a display");
        // Running from the source tree; the Flatpak and the Windows bundle
        // install these into hicolor.
        let icons = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/icons/hicolor/symbolic/apps");
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
