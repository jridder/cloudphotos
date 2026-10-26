//! App state: the local library (SQLite), the sync with iCloud, and what
//! the UI listens to.

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use gtk::{gdk, gio, glib, prelude::*};
use serde_json::{Value, json};

use crate::{
    asset_object::AssetObject,
    helper::{Helper, HelperError},
    edit::Recipe,
    library::{self, Album, Asset, EDITED_PREVIEW, EDITED_THUMB, Library, ORIGINAL, PREVIEW, THUMB},
    render,
    store::{self, Download, Store},
};

/// Whether the app signs in itself (Windows) rather than reusing
/// icloud-linux's sign-in.
pub const OWN_SIGNIN: bool = cfg!(windows);

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Starting,
    /// icloud-linux isn't set up on this computer (with OWN_SIGNIN: never
    /// signed in).
    NeedsSetup(String),
    /// The session expired; sign in again (in the iCloud Drive app, unless
    /// OWN_SIGNIN).
    NeedsSignin(String),
    /// Offline or the helper failed; the local library still shows.
    Offline(String),
    Ready,
}

#[derive(Debug, Clone)]
pub enum Event {
    State,
    /// Listing the library (items seen so far), or done.
    Scanning(Option<usize>),
    Albums,
    /// An item's edit changed (asset id).
    Edited(String),
    /// Downloading into the library: (files done, files total, bytes done), or idle.
    Syncing(Option<(usize, usize, u64)>),
    Toast(String),
}

/// Items whose links are fetched in one request, then downloaded.
const BATCH: usize = 100;
/// Bumped when scans start recording something new for existing items,
/// so the next start rescans once to fill it in.
const SCAN_VERSION: &str = "4";

pub struct Model {
    helper: RefCell<Option<Rc<Helper>>>,
    library: RefCell<Library>,
    pub store: Rc<Store>,
    /// Every visible item, newest first.
    pub all: gio::ListStore,
    /// Recently Deleted, the most recently deleted first.
    pub trash: gio::ListStore,
    by_id: RefCell<HashMap<String, AssetObject>>,
    albums: RefCell<Vec<Album>>,
    state: RefCell<State>,
    scanning: Cell<bool>,
    syncing: Cell<bool>,
    /// Asking iCloud what changed (see `catch_up`).
    checking: Cell<bool>,
    /// When iCloud was last asked (monotonic µs).
    last_check: Cell<i64>,
    /// The periodic check is running (see `watch`).
    watching: Cell<bool>,
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

fn state_dir() -> std::path::PathBuf {
    let dir = std::env::var_os("CLOUDPHOTOS_DATA_DIR")
        .map(Into::into)
        .unwrap_or_else(|| glib::user_data_dir().join("cloudphotos"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn temp_dir() -> std::path::PathBuf {
    std::env::var_os("CLOUDPHOTOS_CACHE_DIR")
        .map(Into::into)
        .unwrap_or_else(|| glib::user_cache_dir().join("cloudphotos"))
}

impl Model {
    fn new() -> Rc<Self> {
        let path = library::default_path();
        let library = Library::open(&path).expect("photo library");
        Rc::new(Self {
            helper: RefCell::default(),
            store: Store::new(path, temp_dir()),
            library: RefCell::new(library),
            all: gio::ListStore::new::<AssetObject>(),
            trash: gio::ListStore::new::<AssetObject>(),
            by_id: RefCell::default(),
            albums: RefCell::default(),
            state: RefCell::new(State::Starting),
            scanning: Cell::new(false),
            syncing: Cell::new(false),
            checking: Cell::new(false),
            last_check: Cell::new(0),
            watching: Cell::new(false),
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

    pub fn library_path(&self) -> std::path::PathBuf {
        self.library.borrow().path().to_owned()
    }

    /// (files, bytes) stored and wanted.
    pub fn totals(&self) -> ((u64, u64), (u64, u64)) {
        self.library.borrow().totals()
    }

    // ── startup, scanning, syncing ──────────────────────────────────────────

    /// Show the local library, then connect and bring it up to date.
    pub fn start(self: &Rc<Self>) {
        self.reload();
        let this = self.clone();
        glib::spawn_future_local(async move { this.connect().await });
    }

    async fn connect(self: Rc<Self>) {
        // Browse what's on this computer without contacting iCloud.
        if std::env::var_os("CLOUDPHOTOS_OFFLINE").is_some() {
            return self.set_state(State::Offline("Offline: showing the library on this computer.".into()));
        }
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
        let mut params = json!({"session_dir": state_dir().join("session")});
        if OWN_SIGNIN {
            params["account"] = json!(state_dir().join("account.json"));
        }
        if let Err(e) = helper.call("connect", params).await {
            return self.set_state(state_for(e));
        }
        self.connected().await;
    }

    /// Sign in with an Apple ID and password (OWN_SIGNIN). `Ok(true)` means
    /// Apple sent a two-factor code: pass it to `verify`.
    pub async fn sign_in(self: &Rc<Self>, username: &str, password: &str) -> Result<bool, String> {
        let helper = self.helper.borrow().clone().ok_or("The iCloud helper isn't running.")?;
        let params = json!({
            "username": username,
            "password": password,
            "account": state_dir().join("account.json"),
            "session_dir": state_dir().join("session"),
        });
        let value = helper.call("signin", params).await.map_err(|e| e.message)?;
        if value.get("needs_code").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(true);
        }
        self.after_sign_in();
        Ok(false)
    }

    /// The two-factor code after `sign_in`.
    pub async fn verify(self: &Rc<Self>, code: &str) -> Result<(), String> {
        let helper = self.helper.borrow().clone().ok_or("The iCloud helper isn't running.")?;
        helper.call("verify", json!({"code": code})).await.map_err(|e| e.message)?;
        self.after_sign_in();
        Ok(())
    }

    fn after_sign_in(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move { this.connected().await });
    }

    /// Bring the library up to date, then keep it so.
    async fn connected(self: &Rc<Self>) {
        self.set_state(State::Ready);
        let outdated = self.library.borrow().meta("scan_version").as_deref() != Some(SCAN_VERSION);
        if outdated || self.all.n_items() == 0 {
            self.scan().await;
        } else {
            self.catch_up().await;
        }
        self.watch();
        self.sync().await;
    }

    /// Check iCloud for changes now (the refresh button).
    pub fn refresh(self: &Rc<Self>) {
        self.check_for_changes(Duration::ZERO);
    }

    /// Keep up with changes made elsewhere (deleted on the iPhone, say) by
    /// asking iCloud every couple of minutes while the app is open.
    fn watch(self: &Rc<Self>) {
        // Once: signing in again after a session expired comes back here.
        if self.watching.replace(true) {
            return;
        }
        let seconds = std::env::var("CLOUDPHOTOS_POLL_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(seconds, move || {
            let Some(model) = weak.upgrade() else { return glib::ControlFlow::Break };
            model.check_for_changes(Duration::ZERO);
            glib::ControlFlow::Continue
        });
    }

    /// Catch up with iCloud, unless it was asked within `unless_within`
    /// (coming back to the window shouldn't ask every time), then download
    /// whatever is new.
    pub fn check_for_changes(self: &Rc<Self>, unless_within: Duration) {
        let since_last = glib::monotonic_time() - self.last_check.get();
        if self.scanning.get() || self.checking.get() || self.state() != State::Ready || since_last < unless_within.as_micros() as i64 {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            if this.catch_up().await {
                this.sync().await;
            }
        });
    }

    /// Ask iCloud what changed since the last look and merge it: one small
    /// request when nothing did. Album changes (or an expired sync token)
    /// mean a full scan. Returns whether anything changed.
    async fn catch_up(self: &Rc<Self>) -> bool {
        let Some(helper) = self.helper.borrow().clone() else { return false };
        let since = self.library.borrow().meta("sync_token");
        if since.is_none() {
            self.scan().await;
            return true;
        }
        self.checking.set(true);
        self.last_check.set(glib::monotonic_time());
        let result = helper.call("changes", json!({"since": since})).await;
        self.checking.set(false);
        let value = match result {
            Ok(value) => value,
            Err(e) if matches!(e.code.as_str(), "needs_signin" | "not_connected") => {
                self.set_state(state_for(e));
                return false;
            }
            Err(e) => {
                eprintln!("cloudphotos: checking for changes failed: {}", e.message);
                return false;
            }
        };
        let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
        if flag("full_scan") || flag("albums_changed") {
            self.scan().await;
            return true;
        }
        fn list<T: serde::de::DeserializeOwned>(value: &Value, key: &str) -> Vec<T> {
            value.get(key).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
        }
        let assets: Vec<Asset> = list(&value, "assets");
        let trashed: Vec<library::Trashed> = list(&value, "trashed");
        let removed: Vec<String> = list(&value, "removed");
        let token = value.get("sync_token").and_then(Value::as_str).unwrap_or_default();
        let nothing = assets.is_empty() && trashed.is_empty() && removed.is_empty();
        let result = self.library.borrow_mut().apply_changes(&assets, &trashed, &removed, token);
        match result {
            Ok(diff) if !nothing => {
                eprintln!("cloudphotos: changes {diff:?}, {} updated", assets.len());
                self.reload();
                true
            }
            Ok(_) => false,
            Err(e) => {
                self.notify(Event::Toast(format!("Couldn't update the library: {e}")));
                false
            }
        }
    }

    /// List the whole library from iCloud and merge it into the local one.
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
                let assets = std::mem::take(&mut *self.scan_assets.borrow_mut());
                let albums = std::mem::take(&mut *self.scan_albums.borrow_mut());
                let token = value.get("sync_token").and_then(Value::as_str).unwrap_or_default();
                let trash: Vec<library::Trashed> = value
                    .get("recently_deleted")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let result = self.library.borrow_mut().apply_scan(&assets, &albums, &trash, token);
                if result.is_ok() {
                    self.library.borrow().set_meta("scan_version", SCAN_VERSION);
                }
                match result {
                    Ok(diff) if diff.added + diff.updated + diff.deleted + diff.purged > 0 => {
                        eprintln!("cloudphotos: scan {diff:?}");
                    }
                    Ok(_) => {}
                    Err(e) => self.notify(Event::Toast(format!("Couldn't update the library: {e}"))),
                }
                self.reload();
            }
            Err(e) if matches!(e.code.as_str(), "needs_signin" | "not_connected") => self.set_state(state_for(e)),
            Err(e) => self.notify(Event::Toast(format!("Couldn't check iCloud: {}", e.message))),
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

    /// Show what the library holds, keeping existing objects so open views stay attached.
    fn reload(&self) {
        let (assets, albums, deleted) = {
            let library = self.library.borrow();
            (library.assets(), library.albums(), library.deleted_assets())
        };
        let mut old = std::mem::take(&mut *self.by_id.borrow_mut());
        // Objects move between the library and Recently Deleted as they are.
        for object in self.trash.iter::<AssetObject>().flatten() {
            old.insert(object.id(), object);
        }
        let mut reuse = |asset: Asset| match old.remove(&asset.id) {
            Some(object) => {
                object.update(|a| *a = asset);
                object
            }
            None => AssetObject::new(asset),
        };
        let trash: Vec<AssetObject> = deleted.into_iter().map(&mut reuse).collect();
        let objects: Vec<AssetObject> = assets.into_iter().map(&mut reuse).collect();
        for id in old.keys() {
            self.store.forget(id);
        }
        self.by_id.replace(objects.iter().map(|o| (o.id(), o.clone())).collect());
        replace_items(&self.all, &objects);
        replace_items(&self.trash, &trash);
        self.albums.replace(albums);
        self.notify(Event::Albums);
    }

    /// Download everything missing: thumbnails first, then previews, then
    /// originals, newest first. Links are fetched per batch right before use,
    /// so they never expire mid-download.
    async fn sync(self: &Rc<Self>) {
        if self.syncing.replace(true) {
            return;
        }
        let missing = self.library.borrow().missing();
        let total = missing.len();
        let mut done = 0usize;
        let mut bytes = 0u64;
        let mut failed = 0usize;
        // Consecutive renditions of one item (same phase) share a link request.
        let mut by_item: Vec<(String, Vec<String>)> = Vec::new();
        for (id, kind) in missing {
            match by_item.last_mut() {
                Some((last, kinds)) if *last == id => kinds.push(kind),
                _ => by_item.push((id, vec![kind])),
            }
        }
        if total > 0 {
            self.notify(Event::Syncing(Some((0, total, 0))));
        }
        for batch in by_item.chunks(BATCH) {
            if self.state() != State::Ready {
                break;
            }
            let Some(helper) = self.helper.borrow().clone() else { break };
            let ids: Vec<&str> = batch.iter().map(|(id, _)| id.as_str()).collect();
            let masters: serde_json::Map<String, Value> = {
                let by_id = self.by_id.borrow();
                batch
                    .iter()
                    .filter_map(|(id, _)| by_id.get(id).and_then(|o| o.with(|a| a.master.clone())).map(|m| (id.clone(), Value::from(m))))
                    .collect()
            };
            let urls = match helper.call("urls", json!({"ids": ids, "masters": masters})).await {
                Ok(urls) => urls,
                Err(e) if matches!(e.code.as_str(), "needs_signin" | "not_connected") => {
                    self.set_state(state_for(e));
                    break;
                }
                Err(e) => {
                    self.notify(Event::Toast(format!("Couldn't reach iCloud: {}", e.message)));
                    break;
                }
            };
            let (reply, results) = async_channel::unbounded();
            let mut queued = 0;
            for (id, kinds) in batch {
                let Some(object) = self.by_id.borrow().get(id).cloned() else { continue };
                for kind in kinds {
                    let Some(url) = urls.get(id).and_then(|u| u.get(kind)).and_then(Value::as_str) else {
                        done += 1;
                        failed += 1;
                        continue;
                    };
                    let (checksum, size) =
                        object.with(|a| a.resources.get(kind).map(|r| (r.checksum.clone(), r.size)).unwrap_or_default());
                    let job = Download { id: id.clone(), kind: kind.clone(), url: url.into(), checksum, size, reply: reply.clone() };
                    let _ = self.store.downloads.send(job).await;
                    queued += 1;
                }
            }
            drop(reply);
            for _ in 0..queued {
                let Ok((id, kind, result)) = results.recv().await else { break };
                done += 1;
                match result {
                    Ok(()) => {
                        bytes += self
                            .by_id
                            .borrow()
                            .get(&id)
                            .and_then(|o| o.with(|a| a.resources.get(&kind).and_then(|r| r.size)))
                            .unwrap_or(0);
                        self.store.saved(&id, &kind);
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("cloudphotos: download {id} {kind} failed: {e:?}");
                    }
                }
                if done % 10 == 0 || done == total {
                    self.notify(Event::Syncing(Some((done, total, bytes))));
                }
            }
        }
        self.syncing.set(false);
        self.notify(Event::Syncing(None));
        if failed > 0 {
            self.notify(Event::Toast(format!("{failed} files couldn't be downloaded; they'll be tried again next time.")));
        }
    }

    /// Items of one album, newest first (ids not in the library are skipped).
    pub fn album_items(&self, album_id: &str) -> gio::ListStore {
        let store = gio::ListStore::new::<AssetObject>();
        let albums = self.albums.borrow();
        if let Some(album) = albums.iter().find(|a| a.id == album_id) {
            let by_id = self.by_id.borrow();
            let mut objects: Vec<AssetObject> = album.assets.iter().filter_map(|id| by_id.get(id).cloned()).collect();
            objects.sort_by(|a, b| b.with(|x| x.taken).total_cmp(&a.with(|x| x.taken)));
            store.extend_from_slice(&objects);
        }
        store
    }

    // ── files ───────────────────────────────────────────────────────────────

    /// What to show for `kind`: the edited rendering when the photo has an edit.
    pub fn display_kind(object: &AssetObject, kind: &str) -> &'static str {
        let edited = object.with(|a| a.edit.is_some());
        match (kind, edited) {
            (THUMB, true) => EDITED_THUMB,
            (PREVIEW, true) => EDITED_PREVIEW,
            (THUMB, false) => THUMB,
            _ => PREVIEW,
        }
    }

    /// A thumbnail or preview; waits for the sync if it isn't downloaded yet.
    pub async fn texture(self: &Rc<Self>, object: &AssetObject, kind: &str) -> Option<gdk::Texture> {
        let kind = Self::display_kind(object, kind);
        let id = object.id();
        match self.store.texture(&id, kind).await {
            Ok(texture) => Some(texture),
            Err(store::Error::Missing) if self.syncing.get() || self.state() == State::Starting => {
                self.store.wait_for(&id, kind).await;
                self.store.texture(&id, kind).await.ok()
            }
            Err(_) => None,
        }
    }

    /// Camera details for the Info panel: from the library, or read from the
    /// original (once it's downloaded) and saved.
    pub async fn exif(self: &Rc<Self>, object: &AssetObject) -> serde_json::Value {
        let id = object.id();
        if let Some(exif) = self.library.borrow().exif(&id) {
            return exif;
        }
        let exif = self.store.exif(&id).await;
        if !exif.is_null() {
            self.library.borrow().set_exif(&id, &exif);
        }
        exif
    }

    /// Whether the original is in the library yet.
    pub async fn has_original(self: &Rc<Self>, object: &AssetObject) -> bool {
        self.store.has(&object.id(), crate::library::ORIGINAL).await
    }

    // ── editing ─────────────────────────────────────────────────────────────

    /// Save a local edit (an identity recipe reverts). The version it
    /// replaces is kept for Undo Last Edit; the original is never touched.
    pub async fn save_edit(self: &Rc<Self>, object: &AssetObject, recipe: Recipe) -> Result<(), String> {
        let previous = object.with(|a| a.edit);
        if previous == Some(recipe) || (previous.is_none() && recipe.is_identity()) {
            return Ok(());
        }
        self.apply_edit(object, recipe).await?;
        self.library.borrow().push_history(&object.id(), previous.as_ref());
        Ok(())
    }

    /// Step back to the previously saved version (or the original).
    pub async fn undo_edit(self: &Rc<Self>, object: &AssetObject) -> Result<bool, String> {
        let Some(previous) = self.library.borrow().pop_history(&object.id()) else { return Ok(false) };
        if let Err(e) = self.apply_edit(object, previous.unwrap_or_default()).await {
            // Put it back so the step isn't lost.
            self.library.borrow().push_history(&object.id(), previous.as_ref());
            return Err(e);
        }
        Ok(true)
    }

    pub fn can_undo_edit(&self, object: &AssetObject) -> bool {
        self.library.borrow().has_history(&object.id())
    }

    /// Render and store `recipe` (identity = back to the original).
    async fn apply_edit(self: &Rc<Self>, object: &AssetObject, recipe: Recipe) -> Result<(), String> {
        let id = object.id();
        if recipe.is_identity() {
            self.library.borrow().remove_edit(&id);
            self.edited(object, None);
            return Ok(());
        }
        let started = std::time::Instant::now();
        render::store_display_images(&self.store, &temp_dir(), &self.library_path(), &id, recipe).await?;
        eprintln!("cloudphotos: rendered edit for {id} in {:?}", started.elapsed());
        let base = object.with(|a| a.resources.get(ORIGINAL).and_then(|r| r.checksum.clone()));
        self.library.borrow().set_edit(&id, &recipe, base.as_deref());
        self.edited(object, Some(recipe));
        Ok(())
    }

    /// Refresh everything showing this item after its edit changed.
    fn edited(&self, object: &AssetObject, recipe: Option<Recipe>) {
        let id = object.id();
        self.store.forget(&id);
        object.update(|a| a.edit = recipe);
        for kind in [EDITED_THUMB, EDITED_PREVIEW] {
            self.store.saved(&id, kind);
        }
        // Grid cells watch this and reload their thumbnail.
        object.set_revision(object.revision() + 1);
        self.notify(Event::Edited(id));
    }

    /// The edited photo at full resolution as a JPEG, written to `target`.
    pub async fn render_edited(self: &Rc<Self>, object: &AssetObject, target: std::path::PathBuf) -> Result<std::path::PathBuf, String> {
        self.render(object, true, render::Output::default(), target).await
    }

    /// Write the photo (its edit, or the original's pixels) as `output` says.
    pub async fn render(
        self: &Rc<Self>,
        object: &AssetObject,
        edited: bool,
        output: render::Output,
        target: std::path::PathBuf,
    ) -> Result<std::path::PathBuf, String> {
        let recipe = if edited { Some(object.with(|a| a.edit).unwrap_or_default()) } else { None };
        render::render_full(&self.store, &temp_dir(), &self.library_path(), &object.id(), recipe, output, target).await
    }

    /// A remembered UI choice (stored in the library's meta table).
    pub fn setting(&self, key: &str) -> Option<String> {
        self.library.borrow().meta(&format!("ui.{key}"))
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        self.library.borrow().set_meta(&format!("ui.{key}"), value);
    }

    /// Upload the edited photo to iCloud as a new photo next to the original.
    pub async fn save_copy_to_icloud(self: &Rc<Self>, object: &AssetObject) -> Result<(), String> {
        let helper = self.helper.borrow().clone().ok_or("Not connected to iCloud.")?;
        let stem = object.with(|a| std::path::Path::new(&a.filename).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let dir = temp_dir().join("upload");
        let _ = std::fs::create_dir_all(&dir);
        let path = self.render_edited(object, dir.join(format!("{stem} (Edited).JPG"))).await?;
        let result = helper.call("upload", json!({"path": path})).await;
        let _ = std::fs::remove_file(&path);
        let value = result.map_err(|e| e.message)?;
        if let Some(asset) = value.get("asset").and_then(|a| serde_json::from_value::<Asset>(a.clone()).ok()) {
            if self.library.borrow().add_asset(&asset).is_ok() {
                self.reload();
                let this = self.clone();
                glib::spawn_future_local(async move { this.sync().await });
            }
        }
        Ok(())
    }

    // ── changes ─────────────────────────────────────────────────────────────

    pub async fn set_favorite(self: &Rc<Self>, object: &AssetObject, value: bool) {
        let Some(helper) = self.helper.borrow().clone() else {
            self.notify(Event::Toast("Couldn't change Favorite: not connected to iCloud.".into()));
            return;
        };
        object.update(|a| a.favorite = value);
        let master = object.with(|a| a.master.clone());
        match helper.call("favorite", json!({"id": object.id(), "master": master, "value": value})).await {
            Ok(_) => self.library.borrow().set_favorite(&object.id(), value),
            Err(e) => {
                object.update(|a| a.favorite = !value);
                self.notify(Event::Toast(format!("Couldn't change Favorite: {}", e.message)));
            }
        }
    }

    /// Move to Recently Deleted in iCloud; the local copy stays (hidden)
    /// until iCloud purges it.
    pub async fn delete(self: &Rc<Self>, object: &AssetObject) -> bool {
        let Some(helper) = self.helper.borrow().clone() else {
            self.notify(Event::Toast("Couldn't delete: not connected to iCloud.".into()));
            return false;
        };
        let id = object.id();
        let master = object.with(|a| a.master.clone());
        if let Err(e) = helper.call("delete", json!({"id": id, "master": master})).await {
            self.notify(Event::Toast(format!("Couldn't delete: {}", e.message)));
            return false;
        }
        let expires = self.library.borrow().mark_deleted(&id);
        self.by_id.borrow_mut().remove(&id);
        if let Some(pos) = self.all.find(object) {
            self.all.remove(pos);
        }
        object.update(|a| {
            a.deleted = true;
            a.expires = Some(expires);
        });
        object.set_revision(object.revision() + 1);
        self.trash.insert(0, object);
        self.notify(Event::Albums);
        true
    }

    /// Bring an item back from Recently Deleted, in iCloud and here.
    pub async fn recover(self: &Rc<Self>, object: &AssetObject) -> bool {
        let Some(helper) = self.helper.borrow().clone() else {
            self.notify(Event::Toast("Couldn't recover: not connected to iCloud.".into()));
            return false;
        };
        let (id, master) = (object.id(), object.with(|a| a.master.clone()));
        if let Err(e) = helper.call("recover", json!({"id": id, "master": master})).await {
            self.notify(Event::Toast(format!("Couldn't recover: {}", e.message)));
            return false;
        }
        self.library.borrow().restore(&id);
        if let Some(pos) = self.trash.find(object) {
            self.trash.remove(pos);
        }
        object.update(|a| {
            a.deleted = false;
            a.expires = None;
        });
        object.set_revision(object.revision() + 1);
        // Back in its place in the timeline (newest first).
        let taken = object.with(|a| a.taken);
        let position = (0..self.all.n_items())
            .find(|&i| self.all.item(i).and_downcast::<AssetObject>().is_some_and(|o| o.with(|a| a.taken) < taken))
            .unwrap_or(self.all.n_items());
        self.all.insert(position, object);
        self.by_id.borrow_mut().insert(id, object.clone());
        self.notify(Event::Albums);
        true
    }

    /// Remove an item from Recently Deleted for good, in iCloud and here.
    pub async fn delete_now(self: &Rc<Self>, object: &AssetObject) -> bool {
        let Some(helper) = self.helper.borrow().clone() else {
            self.notify(Event::Toast("Couldn't delete: not connected to iCloud.".into()));
            return false;
        };
        let (id, master) = (object.id(), object.with(|a| a.master.clone()));
        if let Err(e) = helper.call("purge", json!({"id": id, "master": master})).await {
            self.notify(Event::Toast(format!("Couldn't delete: {}", e.message)));
            return false;
        }
        self.library.borrow().purge(&id);
        self.store.forget(&id);
        if let Some(pos) = self.trash.find(object) {
            self.trash.remove(pos);
        }
        self.notify(Event::Albums);
        true
    }
}

/// Make `store` hold `objects`, in order, by removing, inserting and
/// moving only what differs, so views keep their place (and scroll
/// position) when a few items change.
pub fn replace_items(store: &gio::ListStore, objects: &[AssetObject]) {
    if store.n_items() == 0 {
        store.splice(0, 0, objects);
        return;
    }
    let wanted: std::collections::HashSet<String> = objects.iter().map(AssetObject::id).collect();
    for i in (0..store.n_items()).rev() {
        if store.item(i).and_downcast::<AssetObject>().is_none_or(|o| !wanted.contains(&o.id())) {
            store.remove(i);
        }
    }
    for (i, object) in objects.iter().enumerate() {
        let i = i as u32;
        if store.item(i).and_downcast_ref::<AssetObject>() == Some(object) {
            continue;
        }
        // Moved (its date changed): take it out of its old place first.
        if let Some(j) = (i + 1..store.n_items()).find(|&j| store.item(j).and_downcast_ref::<AssetObject>() == Some(object)) {
            store.remove(j);
        }
        store.insert(i, object);
    }
}

fn state_for(e: HelperError) -> State {
    match e.code.as_str() {
        "not_configured" => State::NeedsSetup(e.message),
        "needs_signin" => State::NeedsSignin(e.message),
        _ => State::Offline(e.message),
    }
}
