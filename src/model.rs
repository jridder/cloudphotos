//! App state: the library (from SQLite, refreshed through the helper), the
//! image cache, and what the UI listens to.

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use gtk::{gdk, gio, glib, prelude::*};
use serde_json::{Value, json};

use crate::{
    asset_object::AssetObject,
    helper::{Helper, HelperError},
    images::{ImageError, Images},
    library::{Album, Asset, Library},
};

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Starting,
    /// icloud-linux isn't set up on this computer.
    NeedsSetup(String),
    /// The shared session expired; sign in again in the iCloud Drive app.
    NeedsSignin(String),
    /// Offline or the helper failed; cached items still show.
    Offline(String),
    Ready,
}

#[derive(Debug, Clone)]
pub enum Event {
    State,
    /// Scan running (items seen so far) or finished.
    Scanning(Option<usize>),
    Albums,
    Toast(String),
}

pub struct Model {
    helper: RefCell<Option<Rc<Helper>>>,
    library: RefCell<Library>,
    pub images: Rc<Images>,
    /// Every item, newest first.
    pub all: gio::ListStore,
    by_id: RefCell<HashMap<String, AssetObject>>,
    albums: RefCell<Vec<Album>>,
    state: RefCell<State>,
    scanning: Cell<bool>,
    scan_assets: RefCell<Vec<Asset>>,
    scan_albums: RefCell<Vec<Album>>,
    listeners: RefCell<Vec<Rc<dyn Fn(&Event)>>>,
}

thread_local! {
    static MODEL: OnceCell<Rc<Model>> = const { OnceCell::new() };
}

pub fn model() -> Rc<Model> {
    MODEL.with(|cell| cell.get_or_init(Model::new).clone())
}

fn data_dir() -> std::path::PathBuf {
    let dir = std::env::var_os("CLOUDPHOTOS_DATA_DIR")
        .map(Into::into)
        .unwrap_or_else(|| glib::user_data_dir().join("cloudphotos"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn cache_dir() -> std::path::PathBuf {
    std::env::var_os("CLOUDPHOTOS_CACHE_DIR")
        .map(Into::into)
        .unwrap_or_else(|| glib::user_cache_dir().join("cloudphotos"))
}

impl Model {
    fn new() -> Rc<Self> {
        let library = Library::open(&data_dir().join("library.sqlite3")).expect("library database");
        Rc::new(Self {
            helper: RefCell::default(),
            library: RefCell::new(library),
            images: Images::new(cache_dir().join("images")),
            all: gio::ListStore::new::<AssetObject>(),
            by_id: RefCell::default(),
            albums: RefCell::default(),
            state: RefCell::new(State::Starting),
            scanning: Cell::new(false),
            scan_assets: RefCell::default(),
            scan_albums: RefCell::default(),
            listeners: RefCell::default(),
        })
    }

    pub fn subscribe(&self, f: impl Fn(&Event) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self, event: Event) {
        let listeners: Vec<_> = self.listeners.borrow().clone();
        for listener in listeners {
            listener(&event);
        }
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    fn set_state(&self, state: State) {
        self.state.replace(state);
        self.notify(Event::State);
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.get()
    }

    pub fn albums(&self) -> Vec<Album> {
        self.albums.borrow().clone()
    }

    // ── startup and scanning ────────────────────────────────────────────────

    /// Show the saved library, then connect and refresh it.
    pub fn start(self: &Rc<Self>) {
        let assets = self.library.borrow().assets();
        let albums = self.library.borrow().albums();
        self.install(assets, albums);
        let this = self.clone();
        glib::spawn_future_local(async move { this.connect().await });
    }

    async fn connect(self: Rc<Self>) {
        self.set_state(State::Starting);
        let weak = Rc::downgrade(&self);
        let helper = match Helper::spawn(move |event| {
            if let Some(model) = weak.upgrade() {
                model.on_helper_event(event);
            }
        }) {
            Ok(helper) => helper,
            Err(e) => return self.set_state(State::Offline(e.message)),
        };
        self.helper.replace(Some(helper.clone()));
        let params = json!({"session_dir": data_dir().join("session")});
        match helper.call("connect", params).await {
            Ok(_) => self.set_state(State::Ready),
            Err(e) => return self.set_state(state_for(e)),
        }
        let since = self.library.borrow().sync_token();
        let changed = match helper.call("changed", json!({"since": since})).await {
            Ok(v) => v.get("changed").and_then(Value::as_bool).unwrap_or(true),
            Err(_) => true,
        };
        if changed || self.all.n_items() == 0 {
            self.scan().await;
        }
    }

    pub fn refresh(self: &Rc<Self>) {
        if self.scanning.get() || self.state() != State::Ready {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move { this.scan().await });
    }

    async fn scan(self: &Rc<Self>) {
        let Some(helper) = self.helper.borrow().clone() else { return };
        self.scanning.set(true);
        self.scan_assets.borrow_mut().clear();
        self.scan_albums.borrow_mut().clear();
        self.notify(Event::Scanning(Some(0)));
        let result = helper.call("scan", json!({})).await;
        self.scanning.set(false);
        match result {
            Ok(value) => {
                let mut assets = std::mem::take(&mut *self.scan_assets.borrow_mut());
                let albums = std::mem::take(&mut *self.scan_albums.borrow_mut());
                assets.sort_by(|a, b| b.taken.total_cmp(&a.taken).then_with(|| a.id.cmp(&b.id)));
                let token = value.get("sync_token").and_then(Value::as_str).unwrap_or_default();
                if let Err(e) = self.library.borrow_mut().replace(&assets, &albums, token) {
                    self.notify(Event::Toast(format!("Couldn't save the library: {e}")));
                }
                self.install(assets, albums);
            }
            Err(e) if matches!(e.code.as_str(), "needs_signin" | "not_connected") => self.set_state(state_for(e)),
            Err(e) => self.notify(Event::Toast(format!("Couldn't refresh the library: {}", e.message))),
        }
        self.notify(Event::Scanning(None));
    }

    fn on_helper_event(&self, event: Value) {
        match event.get("event").and_then(Value::as_str) {
            Some("assets") => {
                let items: Vec<Asset> = event
                    .get("items")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let count = {
                    let mut buffer = self.scan_assets.borrow_mut();
                    buffer.extend(items);
                    buffer.len()
                };
                self.notify(Event::Scanning(Some(count)));
            }
            Some("albums") => {
                let albums: Vec<Album> = event
                    .get("albums")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                self.scan_albums.replace(albums);
            }
            _ => {}
        }
    }

    /// Replace the visible library, keeping existing objects so open views
    /// and cells stay attached.
    fn install(&self, assets: Vec<Asset>, albums: Vec<Album>) {
        let mut old = std::mem::take(&mut *self.by_id.borrow_mut());
        let objects: Vec<AssetObject> = assets
            .into_iter()
            .map(|asset| match old.remove(&asset.id) {
                Some(object) => {
                    object.update(|a| *a = asset);
                    object
                }
                None => AssetObject::new(asset),
            })
            .collect();
        self.by_id.replace(objects.iter().map(|o| (o.id(), o.clone())).collect());
        self.all.splice(0, self.all.n_items(), &objects);
        self.albums.replace(albums);
        self.notify(Event::Albums);
    }

    /// Items of one album, newest first (ids no longer in the library are skipped).
    pub fn album_items(&self, album_id: &str) -> gio::ListStore {
        let store = gio::ListStore::new::<AssetObject>();
        let albums = self.albums.borrow();
        if let Some(album) = albums.iter().find(|a| a.id == album_id) {
            let by_id = self.by_id.borrow();
            let mut objects: Vec<AssetObject> = album.assets.iter().filter_map(|id| by_id.get(id).cloned()).collect();
            objects.sort_by(|a, b| {
                let (ta, tb) = (a.with(|x| x.taken), b.with(|x| x.taken));
                tb.total_cmp(&ta)
            });
            store.extend_from_slice(&objects);
        }
        store
    }

    // ── images ──────────────────────────────────────────────────────────────

    /// A rendition ("thumb" or "preview"), refreshing an expired URL once.
    pub async fn texture(self: &Rc<Self>, object: &AssetObject, rendition: &str) -> Option<gdk::Texture> {
        let id = object.id();
        if let Some(texture) = self.images.cached(&id, rendition) {
            return Some(texture);
        }
        let url = object.with(|a| a.resources.get(rendition).and_then(|r| r.url.clone()));
        match self.images.get(&id, rendition, url.as_deref()).await {
            Ok(texture) => Some(texture),
            Err(ImageError::Expired) => {
                let url = self.fresh_url(object, rendition).await?;
                self.images.get(&id, rendition, Some(&url)).await.ok()
            }
            Err(ImageError::Failed(_)) => None,
        }
    }

    /// Ask iCloud for a current download URL for one rendition.
    pub async fn fresh_url(self: &Rc<Self>, object: &AssetObject, rendition: &str) -> Option<String> {
        let helper = self.helper.borrow().clone()?;
        let id = object.id();
        let urls = helper.call("urls", json!({"ids": [id]})).await.ok()?;
        let urls = urls.get(&id)?.as_object()?.clone();
        object.update(|asset| {
            for (key, url) in &urls {
                if let (Some(resource), Some(url)) = (asset.resources.get_mut(key), url.as_str()) {
                    resource.url = Some(url.to_owned());
                }
            }
        });
        self.library.borrow().update(&object.asset());
        urls.get(rendition).and_then(Value::as_str).map(str::to_owned)
    }

    // ── changes ─────────────────────────────────────────────────────────────

    pub async fn set_favorite(self: &Rc<Self>, object: &AssetObject, value: bool) {
        let Some(helper) = self.helper.borrow().clone() else { return };
        object.update(|a| a.favorite = value);
        match helper.call("favorite", json!({"id": object.id(), "value": value})).await {
            Ok(_) => self.library.borrow().update(&object.asset()),
            Err(e) => {
                object.update(|a| a.favorite = !value);
                self.notify(Event::Toast(format!("Couldn't change Favorite: {}", e.message)));
            }
        }
    }

    /// Move to Recently Deleted in iCloud and drop it from the app.
    pub async fn delete(self: &Rc<Self>, object: &AssetObject) -> bool {
        let Some(helper) = self.helper.borrow().clone() else { return false };
        let id = object.id();
        if let Err(e) = helper.call("delete", json!({"id": id})).await {
            self.notify(Event::Toast(format!("Couldn't delete: {}", e.message)));
            return false;
        }
        self.library.borrow().remove(&id);
        self.images.forget(&id);
        self.by_id.borrow_mut().remove(&id);
        if let Some(pos) = self.all.find(object) {
            self.all.remove(pos);
        }
        self.notify(Event::Albums);
        true
    }
}

fn state_for(e: HelperError) -> State {
    match e.code.as_str() {
        "not_configured" => State::NeedsSetup(e.message),
        "needs_signin" => State::NeedsSignin(e.message),
        _ => State::Offline(e.message),
    }
}
