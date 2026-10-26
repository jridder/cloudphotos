//! Signing in with an Apple ID, where there's no iCloud Drive app to share a
//! sign-in with (Windows): Apple ID and password, then the two-factor code.

use std::{cell::Cell, rc::Rc};

use adw::prelude::*;
use gtk::glib;

use crate::model::model;

pub fn present(parent: &impl IsA<gtk::Widget>) {
    let username = adw::EntryRow::builder().title("Apple ID").input_purpose(gtk::InputPurpose::Email).build();
    let password = adw::PasswordEntryRow::builder().title("Password").build();
    let code = adw::EntryRow::builder().title("Verification Code").input_purpose(gtk::InputPurpose::Digits).visible(false).build();
    let group = adw::PreferencesGroup::builder()
        .description("Your password goes only to Apple. Cloud Photos keeps the session it gets back, not the password.")
        .build();
    group.add(&username);
    group.add(&password);
    group.add(&code);

    let error = gtk::Label::builder().wrap(true).visible(false).build();
    error.add_css_class("error");
    let button = gtk::Button::builder().label("Sign In").halign(gtk::Align::Center).build();
    button.add_css_class("suggested-action");
    button.add_css_class("pill");
    let spinner = adw::Spinner::builder().visible(false).build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_start(24)
        .margin_end(24)
        .margin_top(12)
        .margin_bottom(24)
        .build();
    content.append(&group);
    content.append(&error);
    content.append(&button);
    content.append(&spinner);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&content));
    let dialog = adw::Dialog::builder().title("Sign In to iCloud").content_width(420).child(&view).build();

    // false: Apple ID and password; true: waiting for the code.
    let asking_code = Rc::new(Cell::new(false));
    button.connect_clicked(glib::clone!(
        #[weak] dialog, #[weak] username, #[weak] password, #[weak] code, #[weak] group, #[weak] error, #[weak] spinner,
        #[strong] asking_code,
        move |button| {
            let (user, pass, digits) = (username.text().trim().to_owned(), password.text().to_string(), code.text().trim().to_owned());
            if (!asking_code.get() && (user.is_empty() || pass.is_empty())) || (asking_code.get() && digits.is_empty()) {
                return;
            }
            button.set_sensitive(false);
            spinner.set_visible(true);
            group.set_sensitive(false);
            error.set_visible(false);
            let button = button.clone();
            let asking_code = asking_code.clone();
            glib::spawn_future_local(async move {
                let model = model();
                let result = if asking_code.get() {
                    model.verify(&digits).await.map(|()| false)
                } else {
                    model.sign_in(&user, &pass).await
                };
                button.set_sensitive(true);
                spinner.set_visible(false);
                group.set_sensitive(true);
                match result {
                    Ok(false) => {
                        dialog.close();
                    }
                    Ok(true) => {
                        asking_code.set(true);
                        username.set_visible(false);
                        password.set_visible(false);
                        password.set_text("");
                        code.set_visible(true);
                        group.set_description(Some("Enter the code Apple sent to your other devices."));
                        button.set_label("Verify");
                        code.grab_focus();
                    }
                    Err(message) => {
                        error.set_label(&message);
                        error.set_visible(true);
                    }
                }
            });
        }
    ));
    for row in [username.upcast_ref::<adw::EntryRow>(), password.upcast_ref(), &code] {
        row.connect_entry_activated(glib::clone!(#[weak] button, move |_| button.emit_clicked()));
    }
    dialog.present(Some(parent));
}
