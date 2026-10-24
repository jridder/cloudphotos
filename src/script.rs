//! Test hook: CLOUDPHOTOS_SCRIPT="wait 2; shot /tmp/a.png; open 3; key Right; sidebar 1; quit"
//! drives the window without input devices (headless screenshots).

use adw::prelude::*;
use gtk::{gdk, glib, graphene};

pub fn run(window: adw::ApplicationWindow) {
    let Ok(script) = std::env::var("CLOUDPHOTOS_SCRIPT") else { return };
    let steps: Vec<String> = script.split(';').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
    glib::spawn_future_local(async move {
        for step in steps {
            let (cmd, arg) = step.split_once(' ').unwrap_or((&step, ""));
            match cmd {
                "wait" => glib::timeout_future(std::time::Duration::from_secs_f64(arg.parse().unwrap_or(1.0))).await,
                "shot" => shot(&window, arg),
                "open" => {
                    if let Some(grid) = find::<gtk::GridView>(window.upcast_ref()) {
                        grid.emit_by_name::<()>("activate", &[&arg.parse::<u32>().unwrap_or(0)]);
                    }
                }
                "sidebar" => {
                    if let Some(list) = find::<gtk::ListBox>(window.upcast_ref()) {
                        if let Some(row) = list.row_at_index(arg.parse().unwrap_or(0)) {
                            list.select_row(Some(&row));
                        }
                    }
                }
                "key" => {
                    let key = gdk::Key::from_name(arg).unwrap_or(gdk::Key::Escape);
                    if let Some(page) = find_page(window.upcast_ref()) {
                        for controller in page.observe_controllers().into_iter().flatten() {
                            if let Ok(keys) = controller.downcast::<gtk::EventControllerKey>() {
                                keys.emit_by_name::<bool>("key-pressed", &[&key, &0u32, &gdk::ModifierType::empty()]);
                            }
                        }
                    }
                }
                "respond" => {
                    // respond ID: answer the open alert dialog (e.g. "respond delete").
                    if let Some(dialog) = find::<adw::AlertDialog>(window.upcast_ref()) {
                        dialog.emit_by_name::<()>("response", &[&arg]);
                        dialog.close();
                    }
                }
                "focus" => {
                    // focus TOOLTIP: focus that button, scrolling it into view.
                    if let Some(button) = find_button(window.upcast_ref(), arg) {
                        button.grab_focus();
                    }
                }
                "click" => {
                    // click TOOLTIP: press the visible button with that tooltip.
                    if let Some(button) = find_button(window.upcast_ref(), arg) {
                        button.emit_clicked();
                    }
                }
                "render" => {
                    // render N PATH: item N's edit at full resolution (tests export).
                    let (n, path) = arg.split_once(' ').unwrap_or((arg, "/tmp/cloudphotos-render.jpg"));
                    let model = crate::model::model();
                    if let Some(object) = model.all.item(n.parse().unwrap_or(0)).and_downcast::<crate::asset_object::AssetObject>() {
                        match model.render_edited(&object, path.into()).await {
                            Ok(p) => eprintln!("CLOUDPHOTOS_SCRIPT: rendered {}", p.display()),
                            Err(e) => eprintln!("CLOUDPHOTOS_SCRIPT: render failed: {e}"),
                        }
                    }
                }
                "action" => {
                    // action viewer.undo-edit: activate an action on the open viewer.
                    if let Some(page) = find_page(window.upcast_ref()) {
                        let _ = page.activate_action(arg, None);
                    }
                }
                "dump" => {
                    // dump N: item N's current edit, to stderr.
                    let model = crate::model::model();
                    if let Some(o) = model.all.item(arg.parse().unwrap_or(0)).and_downcast::<crate::asset_object::AssetObject>() {
                        eprintln!("CLOUDPHOTOS_SCRIPT: edit = {:?}", o.with(|a| a.edit));
                    }
                }
                "slider" => {
                    // slider NAME VALUE: set the editor slider under that label.
                    let (name, value) = arg.rsplit_once(' ').unwrap_or((arg, "0"));
                    if let Some(scale) = find_slider(window.upcast_ref(), name) {
                        scale.set_value(value.parse().unwrap_or(0.0));
                    }
                }
                "quit" => {
                    if let Some(app) = window.application() {
                        app.quit();
                    }
                }
                _ => eprintln!("CLOUDPHOTOS_SCRIPT: unknown step {step:?}"),
            }
        }
    });
}

fn shot(window: &adw::ApplicationWindow, path: &str) {
    let (w, h) = (window.width(), window.height());
    let snapshot = gtk::Snapshot::new();
    gtk::WidgetPaintable::new(Some(window)).snapshot(&snapshot, w as f64, h as f64);
    let Some(node) = snapshot.to_node() else { return };
    let Some(renderer) = window.renderer() else { return };
    let texture = renderer.render_texture(&node, Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32)));
    if let Err(e) = texture.save_to_png(path) {
        eprintln!("CLOUDPHOTOS_SCRIPT: {e}");
    }
}

fn walk(root: &gtk::Widget, f: &mut dyn FnMut(&gtk::Widget) -> bool) -> bool {
    if f(root) {
        return true;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if walk(&c, f) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

fn find<T: IsA<gtk::Widget>>(root: &gtk::Widget) -> Option<T> {
    let mut found = None;
    walk(root, &mut |w| {
        if w.is_mapped() {
            if let Some(t) = w.downcast_ref::<T>() {
                found = Some(t.clone());
                return true;
            }
        }
        false
    });
    found
}

fn find_page(root: &gtk::Widget) -> Option<adw::NavigationPage> {
    let mut found = None;
    walk(root, &mut |w| {
        if let Some(page) = w.downcast_ref::<adw::NavigationPage>() {
            if page.is_mapped() && page.tag().as_deref() == Some("viewer") {
                found = Some(page.clone());
                return true;
            }
        }
        false
    });
    found
}

fn find_button(root: &gtk::Widget, tooltip: &str) -> Option<gtk::Button> {
    let mut found = None;
    walk(root, &mut |w| {
        if let Some(button) = w.downcast_ref::<gtk::Button>() {
            if button.is_mapped() && (button.tooltip_text().as_deref() == Some(tooltip) || button.label().as_deref() == Some(tooltip)) {
                found = Some(button.clone());
                return true;
            }
        }
        false
    });
    found
}

/// The gtk::Scale that follows the label `name` (editor sliders).
fn find_slider(root: &gtk::Widget, name: &str) -> Option<gtk::Scale> {
    let mut found = None;
    walk(root, &mut |w| {
        if let Some(label) = w.downcast_ref::<gtk::Label>() {
            if label.is_mapped() && label.label() == name {
                if let Some(scale) = label.parent().and_then(|row| row.next_sibling()).and_downcast::<gtk::Scale>() {
                    found = Some(scale);
                    return true;
                }
            }
        }
        false
    });
    found
}
