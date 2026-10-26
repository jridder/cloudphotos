//! Work on the library's files off the main thread: decoding thumbnails and
//! previews for display, downloading files into the library, and copying
//! originals out (export, playback). Every thread has its own SQLite
//! connection to the library.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
};

use gtk::{gdk, glib};

use crate::library;

const DECODERS: usize = 3;
/// Two downloads at a time: plenty for a home connection, gentle on the account.
pub const DOWNLOADERS: usize = 2;
const MEMORY_LIMIT: usize = 400;
/// Decoded textures kept, in bytes: previews (a zoomed-in grid shows them)
/// are some 5 MB each, so the count alone could hold gigabytes.
const MEMORY_BYTES: usize = 768 << 20;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The link was refused (CloudKit links expire after minutes).
    Expired,
    /// Not in the library yet.
    Missing,
    Failed(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Download {
    pub id: String,
    pub kind: String,
    pub url: String,
    pub checksum: Option<String>,
    pub size: Option<u64>,
    pub reply: async_channel::Sender<(String, String, Result<()>)>,
}

enum Work {
    Decode(String, String, async_channel::Sender<Result<gdk::Texture>>),
    Exif(String, async_channel::Sender<serde_json::Value>),
    Has(String, String, async_channel::Sender<bool>),
    CopyOut(String, String, PathBuf, async_channel::Sender<Result<PathBuf>>),
}

pub struct Store {
    temp: PathBuf,
    work: async_channel::Sender<Work>,
    pub downloads: async_channel::Sender<Download>,
    memory: RefCell<HashMap<String, gdk::Texture>>,
    order: RefCell<VecDeque<String>>,
    /// Cells waiting for a thumbnail the sync hasn't downloaded yet.
    waiting: RefCell<HashMap<String, Vec<async_channel::Sender<()>>>>,
}

thread_local! {
    static DB: RefCell<Option<rusqlite::Connection>> = const { RefCell::new(None) };
}

fn with_db<T>(path: &Path, f: impl FnOnce(&rusqlite::Connection) -> T) -> Option<T> {
    DB.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            *cell = library::connect(path).ok();
        }
        cell.as_ref().map(f)
    })
}

impl Store {
    pub fn new(library: PathBuf, temp: PathBuf) -> Rc<Self> {
        let _ = std::fs::create_dir_all(&temp);
        let (work, queue) = async_channel::unbounded::<Work>();
        for _ in 0..DECODERS {
            let (queue, library) = (queue.clone(), library.clone());
            std::thread::spawn(move || {
                while let Ok(job) = queue.recv_blocking() {
                    match job {
                        Work::Decode(id, kind, reply) => {
                            let _ = reply.send_blocking(decode(&library, &id, &kind));
                        }
                        Work::Has(id, kind, reply) => {
                            let has = with_db(&library, |db| {
                                db.query_row("SELECT 1 FROM blobs WHERE asset_id = ?1 AND kind = ?2", [&id, &kind], |_| Ok(()))
                                    .is_ok()
                            })
                            .unwrap_or(false);
                            let _ = reply.send_blocking(has);
                        }
                        Work::Exif(id, reply) => {
                            let exif = with_db(&library, |db| crate::exif::read(db, &id)).unwrap_or_default();
                            let _ = reply.send_blocking(exif);
                        }
                        Work::CopyOut(id, kind, target, reply) => {
                            let _ = reply.send_blocking(copy_out(&library, &id, &kind, &target));
                        }
                    }
                }
            });
        }
        let (downloads, queue) = async_channel::unbounded::<Download>();
        for _ in 0..DOWNLOADERS {
            let (queue, library, temp) = (queue.clone(), library.clone(), temp.clone());
            std::thread::spawn(move || {
                while let Ok(job) = queue.recv_blocking() {
                    let result = download_into(&library, &temp, &job);
                    let _ = job.reply.send_blocking((job.id, job.kind, result));
                }
            });
        }
        Rc::new(Self {
            temp,
            work,
            downloads,
            memory: RefCell::default(),
            order: RefCell::default(),
            waiting: RefCell::default(),
        })
    }

    pub fn cached(&self, id: &str, kind: &str) -> Option<gdk::Texture> {
        self.memory.borrow().get(&format!("{id}/{kind}")).cloned()
    }

    /// A thumbnail or preview from the library, decoded.
    pub async fn texture(&self, id: &str, kind: &str) -> Result<gdk::Texture> {
        let key = format!("{id}/{kind}");
        if let Some(texture) = self.memory.borrow().get(&key) {
            return Ok(texture.clone());
        }
        let (tx, rx) = async_channel::bounded(1);
        let _ = self.work.send(Work::Decode(id.into(), kind.into(), tx)).await;
        let result = rx.recv().await.unwrap_or_else(|_| Err(Error::Failed("decoder stopped".into())));
        if let Ok(texture) = &result {
            self.remember(key, texture.clone());
        }
        result
    }

    /// Wait until the sync saves this rendition (see `saved`).
    pub async fn wait_for(&self, id: &str, kind: &str) {
        let (tx, rx) = async_channel::bounded(1);
        self.waiting.borrow_mut().entry(format!("{id}/{kind}")).or_default().push(tx);
        let _ = rx.recv().await;
    }

    pub fn saved(&self, id: &str, kind: &str) {
        let key = format!("{id}/{kind}");
        self.memory.borrow_mut().remove(&key);
        for waiter in self.waiting.borrow_mut().remove(&key).unwrap_or_default() {
            let _ = waiter.try_send(());
        }
    }

    pub async fn has(&self, id: &str, kind: &str) -> bool {
        let (tx, rx) = async_channel::bounded(1);
        let _ = self.work.send(Work::Has(id.into(), kind.into(), tx)).await;
        rx.recv().await.unwrap_or(false)
    }

    /// Camera details from the stored original (`Null` if none or not downloaded).
    pub async fn exif(&self, id: &str) -> serde_json::Value {
        let (tx, rx) = async_channel::bounded(1);
        let _ = self.work.send(Work::Exif(id.into(), tx)).await;
        rx.recv().await.unwrap_or_default()
    }

    /// Copy a stored file out to `target` (export).
    pub async fn copy_out(&self, id: &str, kind: &str, target: PathBuf) -> Result<PathBuf> {
        let (tx, rx) = async_channel::bounded(1);
        let _ = self.work.send(Work::CopyOut(id.into(), kind.into(), target, tx)).await;
        rx.recv().await.unwrap_or_else(|_| Err(Error::Failed("worker stopped".into())))
    }

    /// A playable temporary copy of a video or Live Photo clip. Kept until the
    /// next one is made, since GStreamer reads it while playing.
    pub async fn playable(&self, id: &str, kind: &str) -> Result<PathBuf> {
        let dir = self.temp.join("play");
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join(format!("{}-{kind}.mov", glib::compute_checksum_for_string(glib::ChecksumType::Sha1, id).unwrap_or_default()));
        if target.exists() {
            return Ok(target);
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        self.copy_out(id, kind, target).await
    }

    fn remember(&self, key: String, texture: gdk::Texture) {
        let mut memory = self.memory.borrow_mut();
        let mut order = self.order.borrow_mut();
        if memory.insert(key.clone(), texture).is_none() {
            order.push_back(key);
        }
        let size = |t: &gdk::Texture| {
            use gdk::prelude::TextureExt;
            t.width() as usize * t.height() as usize * 4
        };
        let mut bytes: usize = memory.values().map(size).sum();
        while order.len() > MEMORY_LIMIT || (bytes > MEMORY_BYTES && order.len() > 1) {
            let Some(old) = order.pop_front() else { break };
            if let Some(texture) = memory.remove(&old) {
                bytes -= size(&texture);
            }
        }
    }

    pub fn forget(&self, id: &str) {
        let prefix = format!("{id}/");
        self.memory.borrow_mut().retain(|k, _| !k.starts_with(&prefix));
        self.order.borrow_mut().retain(|k| !k.starts_with(&prefix));
    }
}

fn decode(library: &Path, id: &str, kind: &str) -> Result<gdk::Texture> {
    let bytes = with_db(library, |db| library::read_blob(db, id, kind))
        .flatten()
        .ok_or(Error::Missing)?;
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).map_err(|e| Error::Failed(e.to_string()))
}

fn copy_out(library: &Path, id: &str, kind: &str, target: &Path) -> Result<PathBuf> {
    let tmp = target.with_extension("part");
    let result = with_db(library, |db| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        library::copy_blob_to(db, id, kind, &mut file)?;
        file.sync_all()
    })
    .unwrap_or_else(|| Err(std::io::Error::other("no database")));
    match result {
        Ok(()) => std::fs::rename(&tmp, target).map(|_| target.to_owned()).map_err(|e| Error::Failed(e.to_string())),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(if e.kind() == std::io::ErrorKind::NotFound { Error::Missing } else { Error::Failed(e.to_string()) })
        }
    }
}

/// Download one file to a temp file, check its size, then stream it into the library.
fn download_into(library: &Path, temp: &Path, job: &Download) -> Result<()> {
    let tmp = temp.join(format!(
        "{}-{}.download",
        glib::compute_checksum_for_string(glib::ChecksumType::Sha1, &job.id).unwrap_or_default(),
        job.kind
    ));
    let result = (|| {
        let written = fetch_to(&job.url, &tmp)?;
        if let Some(expected) = job.size.filter(|s| *s > 0) {
            if written != expected {
                return Err(Error::Failed(format!("got {written} bytes, expected {expected}")));
            }
        }
        with_db(library, |db| library::write_blob_from(db, &job.id, &job.kind, job.checksum.as_deref(), &tmp))
            .unwrap_or_else(|| Err(std::io::Error::other("no database")))
            .map_err(|e| Error::Failed(e.to_string()))
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

fn fetch_to(url: &str, path: &Path) -> Result<u64> {
    let mut reader: Box<dyn Read> = if let Some(local) = url.strip_prefix("file://") {
        Box::new(std::fs::File::open(local).map_err(|e| Error::Failed(e.to_string()))?)
    } else {
        let response = ureq::get(url).call().map_err(|e| match e {
            ureq::Error::StatusCode(400..=499) => Error::Expired,
            other => Error::Failed(other.to_string()),
        })?;
        Box::new(response.into_body().into_reader())
    };
    let mut file = std::fs::File::create(path).map_err(|e| Error::Failed(e.to_string()))?;
    std::io::copy(&mut reader, &mut file).map_err(|e| Error::Failed(e.to_string()))
}
