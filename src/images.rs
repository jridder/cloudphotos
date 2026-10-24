//! Downloads and caches Apple's JPEG renditions (thumbnails and previews).
//!
//! Files land in the cache directory keyed by asset id and rendition, so
//! they survive restarts and never need the (short-lived) URL again.
//! Decoding happens on the download threads; the main thread only gets a
//! finished texture.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
};

use gtk::{gdk, glib};

const WORKERS: usize = 4;
const MEMORY_LIMIT: usize = 400;

#[derive(Debug, Clone, PartialEq)]
pub enum ImageError {
    /// The URL was refused (CloudKit URLs expire); fetch a fresh one.
    Expired,
    Failed(String),
}

type Result<T> = std::result::Result<T, ImageError>;

struct Job {
    url: String,
    path: PathBuf,
    reply: async_channel::Sender<Result<gdk::Texture>>,
}

pub struct Images {
    dir: PathBuf,
    jobs: async_channel::Sender<Job>,
    memory: RefCell<HashMap<String, gdk::Texture>>,
    order: RefCell<VecDeque<String>>,
    waiting: RefCell<HashMap<String, Vec<async_channel::Sender<Result<gdk::Texture>>>>>,
}

impl Images {
    pub fn new(dir: PathBuf) -> Rc<Self> {
        let _ = std::fs::create_dir_all(&dir);
        let (jobs, queue) = async_channel::unbounded::<Job>();
        for _ in 0..WORKERS {
            let queue = queue.clone();
            std::thread::spawn(move || {
                while let Ok(job) = queue.recv_blocking() {
                    let _ = job.reply.send_blocking(fetch(&job.url, &job.path));
                }
            });
        }
        Rc::new(Self {
            dir,
            jobs,
            memory: RefCell::default(),
            order: RefCell::default(),
            waiting: RefCell::default(),
        })
    }

    pub fn path(&self, asset_id: &str, rendition: &str) -> PathBuf {
        let digest = glib::compute_checksum_for_string(glib::ChecksumType::Sha1, asset_id).unwrap_or_default();
        self.dir.join(format!("{digest}-{rendition}.jpg"))
    }

    pub fn cached(&self, asset_id: &str, rendition: &str) -> Option<gdk::Texture> {
        self.memory.borrow().get(&format!("{asset_id}/{rendition}")).cloned()
    }

    /// The texture for one rendition, downloading it with `url` if needed.
    pub async fn get(&self, asset_id: &str, rendition: &str, url: Option<&str>) -> Result<gdk::Texture> {
        let key = format!("{asset_id}/{rendition}");
        if let Some(texture) = self.memory.borrow().get(&key) {
            return Ok(texture.clone());
        }
        let path = self.path(asset_id, rendition);
        let (tx, rx) = async_channel::bounded(1);
        let first = {
            let mut waiting = self.waiting.borrow_mut();
            let entry = waiting.entry(key.clone()).or_default();
            entry.push(tx.clone());
            entry.len() == 1
        };
        if first {
            if !path.exists() && url.is_none() {
                self.waiting.borrow_mut().remove(&key);
                return Err(ImageError::Expired);
            }
            let (reply, done) = async_channel::bounded(1);
            let _ = self.jobs.send(Job { url: url.unwrap_or_default().to_owned(), path, reply }).await;
            let result = done.recv().await.unwrap_or_else(|_| Err(ImageError::Failed("worker stopped".into())));
            if let Ok(texture) = &result {
                self.remember(key.clone(), texture.clone());
            }
            for waiter in self.waiting.borrow_mut().remove(&key).unwrap_or_default() {
                let _ = waiter.try_send(result.clone());
            }
        }
        rx.recv().await.unwrap_or_else(|_| Err(ImageError::Failed("cancelled".into())))
    }

    fn remember(&self, key: String, texture: gdk::Texture) {
        let mut memory = self.memory.borrow_mut();
        let mut order = self.order.borrow_mut();
        if memory.insert(key.clone(), texture).is_none() {
            order.push_back(key);
        }
        while order.len() > MEMORY_LIMIT {
            if let Some(old) = order.pop_front() {
                memory.remove(&old);
            }
        }
    }

    pub fn forget(&self, asset_id: &str) {
        let prefix = format!("{asset_id}/");
        self.memory.borrow_mut().retain(|key, _| !key.starts_with(&prefix));
        self.order.borrow_mut().retain(|key| !key.starts_with(&prefix));
    }
}

/// Download (if not cached on disk) and decode. Runs on a worker thread.
fn fetch(url: &str, path: &Path) -> Result<gdk::Texture> {
    if !path.exists() {
        let bytes = download(url)?;
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, &bytes).map_err(|e| ImageError::Failed(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| ImageError::Failed(e.to_string()))?;
    }
    gdk::Texture::from_filename(path).map_err(|e| {
        // A bad file would fail forever; let the next attempt download it again.
        let _ = std::fs::remove_file(path);
        ImageError::Failed(e.to_string())
    })
}

pub fn download(url: &str) -> Result<Vec<u8>> {
    if let Some(local) = url.strip_prefix("file://") {
        return std::fs::read(local).map_err(|e| ImageError::Failed(e.to_string()));
    }
    let response = ureq::get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(400..=499) => ImageError::Expired,
        other => ImageError::Failed(other.to_string()),
    })?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| ImageError::Failed(e.to_string()))?;
    Ok(bytes)
}
