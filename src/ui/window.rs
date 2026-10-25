//! Main window: sidebar of collections, the photo grid, and the viewer.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{
    asset_object::AssetObject,
    model::{Event, State, model},
    ui::{grid::PhotoGrid, photo_actions, viewer},
};

/// What the sidebar can show.
#[derive(Clone, PartialEq)]
enum Collection {
    Library,
    Favorites,
    Videos,
    RecentlyDeleted,
    Album(String, String), // id, name
}

impl Collection {
    fn title(&self) -> String {
        match self {
            Collection::Library => "Library".into(),
            Collection::Favorites => "Favorites".into(),
            Collection::Videos => "Videos".into(),
            Collection::RecentlyDeleted => "Recently Deleted".into(),
            Collection::Album(_, name) => name.rsplit('/').next().unwrap_or(name).into(),
        }
    }

    fn items(&self) -> gio::ListModel {
        let model = model();
        match self {
            Collection::Library => model.all.clone().upcast(),
            // Filters over the whole library, so favoriting updates them live.
            Collection::Favorites => {
                let filter = gtk::BoolFilter::new(Some(gtk::PropertyExpression::new(
                    AssetObject::static_type(),
                    gtk::Expression::NONE,
                    "favorite",
                )));
                gtk::FilterListModel::new(Some(model.all.clone()), Some(filter)).upcast()
            }
            Collection::Videos => {
                let filter = gtk::CustomFilter::new(|o| o.downcast_ref::<AssetObject>().is_some_and(|a| a.with(|x| x.is_video())));
                gtk::FilterListModel::new(Some(model.all.clone()), Some(filter)).upcast()
            }
            Collection::RecentlyDeleted => model.trash.clone().upcast(),
            Collection::Album(id, _) => model.album_items(id).upcast(),
        }
    }
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    nav: adw::NavigationView,
    sidebar: gtk::ListBox,
    rows: RefCell<Vec<(gtk::ListBoxRow, Collection)>>,
    grid: PhotoGrid,
    stack: gtk::Stack,
    status: adw::StatusPage,
    title: adw::WindowTitle,
    spinner: adw::Spinner,
    refresh: gtk::Button,
    sync_box: gtk::Box,
    sync_label: gtk::Label,
    sync_bar: gtk::ProgressBar,
    current: RefCell<Collection>,
    items: RefCell<Option<gio::ListModel>>,
}

pub fn build(app: &adw::Application) -> Rc<Window> {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Cloud Photos")
        .default_width(1180)
        .default_height(800)
        .build();

    // Sidebar
    let sidebar = gtk::ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::new());
    sidebar_view.set_content(Some(&gtk::ScrolledWindow::builder().child(&sidebar).vexpand(true).build()));
    // Offline download progress, at the foot of the sidebar.
    let sync_label = gtk::Label::builder().xalign(0.0).wrap(true).build();
    sync_label.add_css_class("caption");
    sync_label.add_css_class("dimmed");
    let sync_bar = gtk::ProgressBar::new();
    let sync_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(12)
        .visible(false)
        .build();
    sync_box.append(&sync_label);
    sync_box.append(&sync_bar);
    sidebar_view.add_bottom_bar(&sync_box);
    let sidebar_page = adw::NavigationPage::builder().title("Cloud Photos").child(&sidebar_view).build();

    // Content: grid or a status page, under a header with the collection name.
    let title = adw::WindowTitle::new("Library", "");
    let spinner = adw::Spinner::builder().visible(false).build();
    let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Check for New Photos").build();
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.pack_end(&refresh);
    header.pack_end(&spinner);

    let window_ref: Rc<RefCell<Option<std::rc::Weak<Window>>>> = Rc::default();
    let grid = {
        let (open_ref, menu_ref) = (window_ref.clone(), window_ref.clone());
        PhotoGrid::new(
            move |items, position| {
                if let Some(win) = open_ref.borrow().as_ref().and_then(|w| w.upgrade()) {
                    viewer::open(&win.nav, &win.toasts, items, position);
                }
            },
            move |object, cell, x, y| {
                if let Some(win) = menu_ref.borrow().as_ref().and_then(|w| w.upgrade()) {
                    win.photo_menu(object, cell, x, y);
                }
            },
        )
    };
    let status = adw::StatusPage::new();
    let stack = gtk::Stack::new();
    stack.add_named(&grid.root, Some("grid"));
    stack.add_named(&status, Some("status"));

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&header);
    content_view.set_content(Some(&stack));
    let content_page = adw::NavigationPage::builder().title("Library").tag("grid").child(&content_view).build();
    let nav = adw::NavigationView::new();
    nav.add(&content_page);

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&adw::NavigationPage::builder().title("Photos").child(&nav).build())
        .min_sidebar_width(200.0)
        .build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));
    window.set_content(Some(&toasts));

    let win = Rc::new(Window {
        window,
        toasts,
        nav,
        sidebar,
        rows: RefCell::default(),
        grid,
        stack,
        status,
        title,
        spinner,
        refresh,
        sync_box,
        sync_label,
        sync_bar,
        current: RefCell::new(Collection::Library),
        items: RefCell::default(),
    });
    window_ref.replace(Some(Rc::downgrade(&win)));

    let weak = Rc::downgrade(&win);
    win.sidebar.connect_row_selected(move |_, row| {
        let (Some(win), Some(row)) = (weak.upgrade(), row) else { return };
        let collection = win.rows.borrow().iter().find(|(r, _)| r == row).map(|(_, c)| c.clone());
        if let Some(collection) = collection {
            win.show(collection);
        }
    });
    win.refresh.connect_clicked(|_| model().refresh());

    let weak = Rc::downgrade(&win);
    model().subscribe(move |event| {
        let Some(win) = weak.upgrade() else { return };
        match event {
            Event::State => win.update_state(),
            Event::Scanning(count) => win.update_scanning(*count),
            Event::Albums => win.rebuild_sidebar(),
            Event::Syncing(progress) => win.update_sync(*progress),
            Event::Toast(message) => win.toast(message),
            Event::Edited(_) => {}
        }
    });
    win.rebuild_sidebar();
    win.update_sync(None);
    win.show(Collection::Library);
    win.update_state();
    win
}

impl Window {
    /// The right-click menu for a photo in the grid.
    fn photo_menu(&self, object: &AssetObject, cell: &gtk::Widget, x: f64, y: f64) {
        let Some(items) = self.items.borrow().clone() else { return };
        let target = photo_actions::Target { nav: self.nav.clone(), toasts: self.toasts.clone(), items: items.clone(), object: object.clone() };
        let (nav, toasts, o) = (self.nav.clone(), self.toasts.clone(), object.clone());
        let open = Rc::new(move || {
            if let Some(position) = (0..items.n_items()).find(|&i| items.item(i).as_ref() == Some(o.upcast_ref())) {
                viewer::open(&nav, &toasts, &items, position);
            }
        });
        photo_actions::popup(cell, x, y, target, Some(open), Rc::new(|| {}));
    }

    pub fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::builder().title(glib::markup_escape_text(message)).timeout(6).build());
    }

    fn rebuild_sidebar(&self) {
        let current = self.current.borrow().clone();
        while let Some(child) = self.sidebar.first_child() {
            self.sidebar.remove(&child);
        }
        let mut rows = Vec::new();
        let mut add = |label: &str, icon: &str, collection: Collection| {
            let row = sidebar_row(label, icon);
            self.sidebar.append(&row);
            rows.push((row, collection));
        };
        add("Library", "image-x-generic-symbolic", Collection::Library);
        add("Favorites", "cloudphotos-heart-filled-symbolic", Collection::Favorites);
        add("Videos", "camera-video-symbolic", Collection::Videos);
        let albums = model().albums();
        for (name, icon) in [("Live", "camera-photo-symbolic"), ("Screenshots", "applets-screenshooter-symbolic")] {
            if let Some(album) = albums.iter().find(|a| a.kind == "smart" && a.name == name) {
                let label = if name == "Live" { "Live Photos" } else { name };
                add(label, icon, Collection::Album(album.id.clone(), label.into()));
            }
        }
        add("Recently Deleted", "user-trash-symbolic", Collection::RecentlyDeleted);
        let user: Vec<_> = albums.iter().filter(|a| a.kind == "user").collect();
        if !user.is_empty() {
            let heading = gtk::Label::builder().label("Albums").xalign(0.0).margin_top(12).margin_start(12).build();
            heading.add_css_class("heading");
            heading.add_css_class("dimmed");
            let row = gtk::ListBoxRow::builder().child(&heading).selectable(false).activatable(false).build();
            self.sidebar.append(&row);
            for album in user {
                // Albums inside folders show as "Folder › Album".
                let label = album.name.replace('/', " › ");
                add(&label, "folder-pictures-symbolic", Collection::Album(album.id.clone(), album.name.clone()));
            }
        }
        let selected = rows.iter().find(|(_, c)| *c == current).or(rows.first()).map(|(r, _)| r.clone());
        self.rows.replace(rows);
        if let Some(row) = selected {
            self.sidebar.select_row(Some(&row));
        }
    }

    fn show(&self, collection: Collection) {
        // Leave the viewer (or editor) for the newly chosen collection's grid.
        if self.current.borrow().clone() != collection {
            self.nav.pop_to_tag("grid");
        }
        let items = collection.items();
        self.grid.set_model(&items);
        self.title.set_title(&collection.title());
        let count = items.n_items();
        self.title.set_subtitle(&match count {
            0 => String::new(),
            1 => "1 item".into(),
            n => format!("{n} items"),
        });
        let weak_title = self.title.downgrade();
        items.connect_items_changed(move |items, _, _, _| {
            if let Some(title) = weak_title.upgrade() {
                let n = items.n_items();
                title.set_subtitle(&if n == 1 { "1 item".into() } else { format!("{n} items") });
            }
        });
        self.items.replace(Some(items));
        self.current.replace(collection);
        self.update_state();
    }

    fn update_state(&self) {
        let state = model().state();
        let empty = self.items.borrow().as_ref().is_none_or(|i| i.n_items() == 0);
        self.refresh.set_sensitive(state == State::Ready && !model().is_scanning());
        let page = match (&state, empty) {
            (State::NeedsSetup(message), _) => Some(("Set Up iCloud Drive First", "dialog-information-symbolic", message.clone())),
            (State::NeedsSignin(_), true) => Some((
                "Sign In Again",
                "dialog-password-symbolic",
                "Open the iCloud Drive app and choose Sign In Again. Cloud Photos uses the same sign-in.".into(),
            )),
            (State::Offline(message), true) => Some(("Can't Reach iCloud", "network-offline-symbolic", message.clone())),
            (State::Starting, true) => Some(("Connecting to iCloud…", "content-loading-symbolic", String::new())),
            (State::Ready, true) if model().is_scanning() => None,
            (State::Ready, true) if *self.current.borrow() == Collection::RecentlyDeleted => Some((
                "No Recently Deleted Items",
                "user-trash-symbolic",
                "Deleted photos and videos stay here for 30 days before iCloud removes them for good.".into(),
            )),
            (State::Ready, true) => Some(("No Photos", "image-x-generic-symbolic", String::new())),
            _ => None,
        };
        match page {
            Some((title, icon, description)) => {
                self.status.set_title(title);
                self.status.set_icon_name(Some(icon));
                self.status.set_description(Some(&glib::markup_escape_text(&description)));
                self.stack.set_visible_child_name("status");
            }
            None => self.stack.set_visible_child_name("grid"),
        }
        if let State::NeedsSignin(_) = state {
            if !empty {
                self.toast("iCloud needs you to sign in again — use Sign In Again in the iCloud Drive app.");
            }
        }
    }

    fn update_scanning(&self, count: Option<usize>) {
        self.spinner.set_visible(count.is_some());
        self.refresh.set_visible(count.is_none());
        if let Some(n) = count {
            if self.items.borrow().as_ref().is_none_or(|i| i.n_items() == 0) {
                self.stack.set_visible_child_name("status");
                self.status.set_icon_name(Some("content-loading-symbolic"));
                self.status.set_title("Loading Your Library…");
                self.status.set_description(Some(&format!("{n} items so far")));
            }
        } else {
            self.update_state();
        }
    }
}

impl Window {
    fn update_sync(&self, progress: Option<(usize, usize, u64)>) {
        self.sync_box.set_visible(true);
        self.sync_bar.set_visible(progress.is_some());
        if progress.is_none() {
            // Idle: what the library holds, and where.
            let model = model();
            let ((files, bytes), (wanted, _)) = model.totals();
            let path = model.library_path();
            let folder = path.parent().map(|p| p.to_string_lossy().replace(&*glib::home_dir().to_string_lossy(), "~")).unwrap_or_default();
            let complete = if files >= wanted { "All downloaded" } else { "Partly downloaded" };
            self.sync_label.set_label(&format!("{complete} · {} on this computer\n{folder}", glib::format_size(bytes)));
            self.sync_box.set_tooltip_text(Some(&path.to_string_lossy()));
        }
        if let Some((done, total, bytes)) = progress {
            self.sync_label.set_label(&format!(
                "Downloading your library · {done} of {total} files · {}",
                glib::format_size(bytes)
            ));
            self.sync_bar.set_fraction(done as f64 / total.max(1) as f64);
        }
    }
}

fn sidebar_row(label: &str, icon: &str) -> gtk::ListBoxRow {
    let row_box = gtk::Box::builder().spacing(12).margin_start(6).margin_end(6).margin_top(6).margin_bottom(6).build();
    row_box.append(&gtk::Image::from_icon_name(icon));
    row_box.append(&gtk::Label::builder().label(label).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build());
    gtk::ListBoxRow::builder().child(&row_box).build()
}
