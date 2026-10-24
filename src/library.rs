//! The local photo library: one SQLite file holding the metadata, the album
//! structure and the files themselves (originals, Live Photo videos, and the
//! JPEG thumbnail and preview used for display).
//!
//! Default location: ~/Pictures/iCloud Photos/Photos Library.sqlite
//! (CLOUDPHOTOS_LIBRARY overrides it, for tests).
//!
//! Blobs live in their own table so browsing never touches their pages, and
//! go in and out through SQLite's incremental blob I/O so even multi-GB
//! videos are streamed, not held in memory. The main thread owns one
//! connection; download and decode threads open their own (WAL mode).

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, MAIN_DB, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Renditions the library keeps.
pub const THUMB: &str = "thumb";
pub const PREVIEW: &str = "preview";
pub const ORIGINAL: &str = "original";
pub const LIVE: &str = "live";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resource {
    #[serde(default)]
    pub url: Option<String>,
    pub size: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub checksum: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    /// CloudKit master record (holds the download URLs).
    #[serde(default)]
    pub master: Option<String>,
    pub filename: String,
    /// "photo" or "video"
    pub kind: String,
    pub taken: f64,
    pub added: f64,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub live: bool,
    pub favorite: bool,
    /// thumb, preview, original, and live when present
    pub resources: HashMap<String, Resource>,
    /// From Apple's records: caption, keywords, latitude/longitude, time
    /// zone, video duration… (see the helper's asset_details).
    #[serde(default)]
    pub details: serde_json::Value,
}

impl Asset {
    pub fn is_video(&self) -> bool {
        self.kind == "video"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    /// "smart" or "user"
    pub kind: String,
    pub assets: Vec<String>,
}

/// What a scan changed in the local library.
#[derive(Debug, Default)]
pub struct Diff {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub purged: usize,
}

pub fn default_path() -> PathBuf {
    std::env::var_os("CLOUDPHOTOS_LIBRARY").map(PathBuf::from).unwrap_or_else(|| {
        let pictures = gtk::glib::user_special_dir(gtk::glib::UserDirectory::Pictures)
            .unwrap_or_else(|| gtk::glib::home_dir().join("Pictures"));
        pictures.join("iCloud Photos").join("Photos Library.sqlite")
    })
}

fn configure(db: &Connection) -> rusqlite::Result<()> {
    db.busy_timeout(Duration::from_secs(60))?;
    db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")
}

/// A connection for a worker thread.
pub fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    configure(&db)?;
    Ok(db)
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS assets (
        id TEXT PRIMARY KEY,
        master TEXT,
        filename TEXT NOT NULL,
        kind TEXT NOT NULL,
        taken REAL NOT NULL,
        added REAL NOT NULL,
        width INTEGER NOT NULL,
        height INTEGER NOT NULL,
        size INTEGER NOT NULL,
        live INTEGER NOT NULL,
        favorite INTEGER NOT NULL,
        -- 1 while the photo is in iCloud's Recently Deleted: hidden, kept.
        deleted INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS assets_taken ON assets (taken DESC);
    CREATE TABLE IF NOT EXISTS resources (
        asset_id TEXT NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        size INTEGER,
        width INTEGER,
        height INTEGER,
        uti TEXT,
        checksum TEXT,
        PRIMARY KEY (asset_id, kind)
    );
    CREATE TABLE IF NOT EXISTS blobs (
        asset_id TEXT NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        -- the resource checksum this file was downloaded for
        checksum TEXT,
        saved REAL NOT NULL,
        data BLOB NOT NULL,
        UNIQUE (asset_id, kind)
    );
    -- Files larger than PART_SIZE: blobs holds the first part, the rest
    -- continue here in order (SQLite caps a single value at 1 GB).
    CREATE TABLE IF NOT EXISTS blob_parts (
        asset_id TEXT NOT NULL REFERENCES assets (id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        part INTEGER NOT NULL,
        data BLOB NOT NULL,
        UNIQUE (asset_id, kind, part)
    );
    CREATE TABLE IF NOT EXISTS albums (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        pos INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS album_assets (
        album_id TEXT NOT NULL REFERENCES albums (id) ON DELETE CASCADE,
        asset_id TEXT NOT NULL,
        pos INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS album_assets_album ON album_assets (album_id, pos);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
";

pub struct Library {
    path: PathBuf,
    db: Connection,
}

impl Library {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let fresh = !path.exists();
        let db = Connection::open(path)?;
        if fresh {
            // Space from purged photos can be given back without a full VACUUM.
            db.execute_batch("PRAGMA auto_vacuum = INCREMENTAL;")?;
        }
        configure(&db)?;
        db.execute_batch(SCHEMA)?;
        // Columns added after the first release.
        for (column, ddl) in [("details", "ALTER TABLE assets ADD COLUMN details TEXT"), ("exif", "ALTER TABLE assets ADD COLUMN exif TEXT")] {
            let present: bool = db
                .prepare("SELECT 1 FROM pragma_table_info('assets') WHERE name = ?1")?
                .exists([column])?;
            if !present {
                db.execute(ddl, [])?;
            }
        }
        Ok(Self { path: path.to_owned(), db })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    // ── reading ─────────────────────────────────────────────────────────────

    /// Visible items, newest first.
    pub fn assets(&self) -> Vec<Asset> {
        let mut resources: HashMap<String, HashMap<String, Resource>> = HashMap::new();
        if let Ok(mut stmt) = self.db.prepare("SELECT asset_id, kind, size, width, height, uti, checksum FROM resources") {
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    Resource {
                        url: None,
                        size: r.get(2)?,
                        width: r.get(3)?,
                        height: r.get(4)?,
                        kind: r.get(5)?,
                        checksum: r.get(6)?,
                    },
                ))
            });
            for (id, kind, res) in rows.into_iter().flatten().flatten() {
                resources.entry(id).or_default().insert(kind, res);
            }
        }
        let Ok(mut stmt) = self.db.prepare(
            "SELECT id, master, filename, kind, taken, added, width, height, size, live, favorite, details
             FROM assets WHERE deleted = 0 ORDER BY taken DESC, id",
        ) else {
            return vec![];
        };
        stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            Ok(Asset {
                resources: resources.remove(&id).unwrap_or_default(),
                id,
                master: r.get(1)?,
                filename: r.get(2)?,
                kind: r.get(3)?,
                taken: r.get(4)?,
                added: r.get(5)?,
                width: r.get(6)?,
                height: r.get(7)?,
                size: r.get(8)?,
                live: r.get::<_, i64>(9)? != 0,
                favorite: r.get::<_, i64>(10)? != 0,
                details: r
                    .get::<_, Option<String>>(11)?
                    .and_then(|d| serde_json::from_str(&d).ok())
                    .unwrap_or_default(),
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn albums(&self) -> Vec<Album> {
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(mut stmt) = self.db.prepare("SELECT album_id, asset_id FROM album_assets ORDER BY album_id, pos") {
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
            for (album, asset) in rows.into_iter().flatten().flatten() {
                members.entry(album).or_default().push(asset);
            }
        }
        let Ok(mut stmt) = self.db.prepare("SELECT id, name, kind FROM albums ORDER BY pos") else { return vec![] };
        stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            Ok(Album { assets: members.remove(&id).unwrap_or_default(), id, name: r.get(1)?, kind: r.get(2)? })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    /// (asset id, rendition) still to download, newest items first, thumbnails
    /// for everything before any previews, previews before originals.
    pub fn missing(&self) -> Vec<(String, String)> {
        let Ok(mut stmt) = self.db.prepare(
            "SELECT r.asset_id, r.kind FROM resources r
             JOIN assets a ON a.id = r.asset_id
             LEFT JOIN blobs b ON b.asset_id = r.asset_id AND b.kind = r.kind
             WHERE a.deleted = 0 AND (b.asset_id IS NULL OR b.checksum IS NOT r.checksum)
             ORDER BY CASE r.kind WHEN 'thumb' THEN 0 WHEN 'preview' THEN 1 ELSE 2 END, a.taken DESC",
        ) else {
            return vec![];
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// (files stored, bytes stored) and (files wanted, bytes wanted).
    pub fn totals(&self) -> ((u64, u64), (u64, u64)) {
        let stored = self
            .db
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(data)), 0) + (SELECT COALESCE(SUM(length(data)), 0) FROM blob_parts) FROM blobs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, 0));
        let wanted = self
            .db
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(r.size), 0) FROM resources r JOIN assets a ON a.id = r.asset_id WHERE a.deleted = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, 0));
        (stored, wanted)
    }

    // ── writing ─────────────────────────────────────────────────────────────

    /// Merge a full listing from iCloud. Items now in Recently Deleted are
    /// hidden but kept; items gone from iCloud entirely are purged, files and all.
    pub fn apply_scan(&mut self, assets: &[Asset], albums: &[Album], recently_deleted: &[String], sync_token: &str) -> rusqlite::Result<Diff> {
        let mut diff = Diff::default();
        let tx = self.db.transaction()?;
        let existing: HashMap<String, (Option<String>, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT a.id, (SELECT checksum FROM resources WHERE asset_id = a.id AND kind = 'original'), a.deleted FROM assets a",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.flatten().collect()
        };
        {
            let mut upsert = tx.prepare(
                "INSERT INTO assets (id, master, filename, kind, taken, added, width, height, size, live, favorite, deleted, details)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)
                 ON CONFLICT (id) DO UPDATE SET master = ?2, filename = ?3, kind = ?4, taken = ?5, added = ?6,
                     width = ?7, height = ?8, size = ?9, live = ?10, favorite = ?11, deleted = 0, details = ?12",
            )?;
            let mut clear = tx.prepare("DELETE FROM resources WHERE asset_id = ?1")?;
            let mut resource = tx.prepare(
                "INSERT INTO resources (asset_id, kind, size, width, height, uti, checksum) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for a in assets {
                let original = a.resources.get(ORIGINAL).and_then(|r| r.checksum.clone());
                match existing.get(&a.id) {
                    None => diff.added += 1,
                    Some((old, _)) if *old != original => diff.updated += 1,
                    _ => {}
                }
                let details = (!a.details.is_null()).then(|| a.details.to_string());
                upsert.execute(params![
                    a.id, a.master, a.filename, a.kind, a.taken, a.added, a.width, a.height, a.size, a.live as i64, a.favorite as i64,
                    details
                ])?;
                if diff_changed(&existing, a, &original) {
                    // A new original means new EXIF.
                    tx.execute("UPDATE assets SET exif = NULL WHERE id = ?1", [&a.id])?;
                }
                clear.execute([&a.id])?;
                for (kind, r) in &a.resources {
                    resource.execute(params![a.id, kind, r.size, r.width, r.height, r.kind, r.checksum])?;
                }
            }
        }
        let listed: std::collections::HashSet<&str> = assets.iter().map(|a| a.id.as_str()).collect();
        let in_trash: std::collections::HashSet<&str> = recently_deleted.iter().map(String::as_str).collect();
        for id in existing.keys().filter(|id| !listed.contains(id.as_str())) {
            if in_trash.contains(id.as_str()) {
                if tx.execute("UPDATE assets SET deleted = 1 WHERE id = ?1 AND deleted = 0", [id])? > 0 {
                    diff.deleted += 1;
                }
            } else {
                tx.execute("DELETE FROM assets WHERE id = ?1", [id])?;
                diff.purged += 1;
            }
        }
        tx.execute("DELETE FROM albums", [])?;
        tx.execute("DELETE FROM album_assets", [])?;
        {
            let mut album = tx.prepare("INSERT INTO albums (id, name, kind, pos) VALUES (?1, ?2, ?3, ?4)")?;
            let mut member = tx.prepare("INSERT INTO album_assets (album_id, asset_id, pos) VALUES (?1, ?2, ?3)")?;
            for (pos, a) in albums.iter().enumerate() {
                album.execute(params![a.id, a.name, a.kind, pos as i64])?;
                for (i, asset) in a.assets.iter().enumerate() {
                    member.execute(params![a.id, asset, i as i64])?;
                }
            }
        }
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('sync_token', ?1)", [sync_token])?;
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('scanned_at', ?1)",
            [gtk::glib::real_time().to_string()],
        )?;
        tx.commit()?;
        if diff.purged > 0 {
            let _ = self.db.execute_batch("PRAGMA incremental_vacuum;");
        }
        Ok(diff)
    }

    pub fn set_meta(&self, key: &str, value: &str) {
        let _ = self.db.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", [key, value]);
    }

    /// EXIF read from the original, if it has been read before.
    pub fn exif(&self, id: &str) -> Option<serde_json::Value> {
        self.db
            .query_row("SELECT exif FROM assets WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0))
            .optional()
            .ok()
            .flatten()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn set_exif(&self, id: &str, exif: &serde_json::Value) {
        let _ = self.db.execute("UPDATE assets SET exif = ?2 WHERE id = ?1", params![id, exif.to_string()]);
    }

    pub fn set_favorite(&self, id: &str, value: bool) {
        let _ = self.db.execute("UPDATE assets SET favorite = ?2 WHERE id = ?1", params![id, value as i64]);
    }

    /// Moved to Recently Deleted from this app: hide it, keep its files.
    pub fn mark_deleted(&self, id: &str) {
        let _ = self.db.execute("UPDATE assets SET deleted = 1 WHERE id = ?1", [id]);
    }
}

fn diff_changed(existing: &HashMap<String, (Option<String>, i64)>, asset: &Asset, original: &Option<String>) -> bool {
    existing.get(&asset.id).is_some_and(|(old, _)| old != original)
}

/// Read-only access to one blob, for parsers that need to seek (EXIF).
pub fn open_blob<'a>(db: &'a Connection, id: &str, kind: &str) -> rusqlite::Result<rusqlite::blob::Blob<'a>> {
    let rowid: i64 = db.query_row("SELECT rowid FROM blobs WHERE asset_id = ?1 AND kind = ?2", [id, kind], |r| r.get(0))?;
    db.blob_open(MAIN_DB, c"blobs", c"data", rowid, true)
}

// ── blobs (any thread, own connection) ──────────────────────────────────────

/// The whole blob (thumbnails and previews; small).
pub fn read_blob(db: &Connection, id: &str, kind: &str) -> Option<Vec<u8>> {
    db.query_row("SELECT data FROM blobs WHERE asset_id = ?1 AND kind = ?2", [id, kind], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
}

/// Largest piece stored in one row; bigger files are split across rows.
#[cfg(not(test))]
pub const PART_SIZE: u64 = 256 << 20;
#[cfg(test)]
pub const PART_SIZE: u64 = 1 << 20;

/// Stream a stored file into `out` (originals, videos), joining its parts.
pub fn copy_blob_to(db: &Connection, id: &str, kind: &str, out: &mut impl Write) -> std::io::Result<u64> {
    let rowid: i64 = db
        .query_row("SELECT rowid FROM blobs WHERE asset_id = ?1 AND kind = ?2", [id, kind], |r| r.get(0))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
    let mut blob = db.blob_open(MAIN_DB, c"blobs", c"data", rowid, true).map_err(std::io::Error::other)?;
    let mut total = std::io::copy(&mut blob, out)?;
    let parts: Vec<i64> = db
        .prepare("SELECT rowid FROM blob_parts WHERE asset_id = ?1 AND kind = ?2 ORDER BY part")
        .and_then(|mut stmt| stmt.query_map([id, kind], |r| r.get(0))?.collect())
        .map_err(std::io::Error::other)?;
    for rowid in parts {
        let mut blob = db.blob_open(MAIN_DB, c"blob_parts", c"data", rowid, true).map_err(std::io::Error::other)?;
        total += std::io::copy(&mut blob, out)?;
    }
    Ok(total)
}

/// Store the file at `source`, streaming it in, split into PART_SIZE pieces.
pub fn write_blob_from(db: &Connection, id: &str, kind: &str, checksum: Option<&str>, source: &Path) -> std::io::Result<()> {
    let len = std::fs::metadata(source)?.len();
    let mut file = std::fs::File::open(source)?;
    let tx = db.unchecked_transaction().map_err(std::io::Error::other)?;
    tx.execute("DELETE FROM blobs WHERE asset_id = ?1 AND kind = ?2", [id, kind]).map_err(std::io::Error::other)?;
    tx.execute("DELETE FROM blob_parts WHERE asset_id = ?1 AND kind = ?2", [id, kind]).map_err(std::io::Error::other)?;
    let first = len.min(PART_SIZE);
    tx.execute(
        "INSERT INTO blobs (asset_id, kind, checksum, saved, data) VALUES (?1, ?2, ?3, ?4, zeroblob(?5))",
        params![id, kind, checksum, gtk::glib::real_time() as f64 / 1e6, first as i64],
    )
    .map_err(std::io::Error::other)?;
    fill(&tx, c"blobs", tx.last_insert_rowid(), &mut file, first)?;
    let mut written = first;
    let mut part = 1i64;
    while written < len {
        let size = (len - written).min(PART_SIZE);
        tx.execute(
            "INSERT INTO blob_parts (asset_id, kind, part, data) VALUES (?1, ?2, ?3, zeroblob(?4))",
            params![id, kind, part, size as i64],
        )
        .map_err(std::io::Error::other)?;
        fill(&tx, c"blob_parts", tx.last_insert_rowid(), &mut file, size)?;
        written += size;
        part += 1;
    }
    tx.commit().map_err(std::io::Error::other)
}

/// Copy exactly `size` bytes from `file` into a zeroblob row.
fn fill(db: &Connection, table: &std::ffi::CStr, rowid: i64, file: &mut std::fs::File, size: u64) -> std::io::Result<()> {
    let mut blob = db.blob_open(MAIN_DB, table, c"data", rowid, false).map_err(std::io::Error::other)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut left = size;
    while left > 0 {
        let want = left.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..want])?;
        blob.write_all(&buffer[..want])?;
        left -= want as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, checksum: &str) -> Asset {
        let mut resources = HashMap::new();
        for kind in [THUMB, ORIGINAL] {
            resources.insert(
                kind.to_string(),
                Resource { size: Some(5), checksum: Some(format!("{checksum}-{kind}")), ..Default::default() },
            );
        }
        Asset {
            id: id.into(),
            master: Some(format!("M-{id}")),
            filename: format!("{id}.HEIC"),
            kind: "photo".into(),
            taken: 1.0,
            added: 1.0,
            width: 1,
            height: 1,
            size: 5,
            live: false,
            favorite: false,
            resources,
            details: serde_json::Value::Null,
        }
    }

    fn store(lib: &Library, id: &str, kind: &str, checksum: &str, dir: &Path) {
        let file = dir.join("f");
        std::fs::write(&file, b"12345").unwrap();
        let db = connect(lib.path()).unwrap();
        write_blob_from(&db, id, kind, Some(&format!("{checksum}-{kind}")), &file).unwrap();
    }

    fn setup() -> (PathBuf, Library) {
        let dir = std::env::temp_dir().join(format!("cloudphotos-test-{}", std::process::id() as u64 * 1000 + rand_suffix()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = Library::open(&dir.join("lib.sqlite")).unwrap();
        (dir, lib)
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn first_scan_adds_and_everything_is_missing() {
        let (_dir, mut lib) = setup();
        let diff = lib.apply_scan(&[asset("a", "1"), asset("b", "1")], &[], &[], "t1").unwrap();
        assert_eq!(diff.added, 2);
        assert_eq!(lib.missing().len(), 4);
        // Thumbnails come before originals.
        assert!(lib.missing()[..2].iter().all(|(_, k)| k == THUMB));
        assert_eq!(lib.meta("sync_token").as_deref(), Some("t1"));
    }

    #[test]
    fn stored_files_are_not_missing_until_the_photo_changes() {
        let (dir, mut lib) = setup();
        lib.apply_scan(&[asset("a", "1")], &[], &[], "t1").unwrap();
        store(&lib, "a", THUMB, "1", &dir);
        store(&lib, "a", ORIGINAL, "1", &dir);
        assert!(lib.missing().is_empty());
        // Edited on the phone: new checksums, so it downloads again.
        let diff = lib.apply_scan(&[asset("a", "2")], &[], &[], "t2").unwrap();
        assert_eq!(diff.updated, 1);
        assert_eq!(lib.missing().len(), 2);
    }

    #[test]
    fn recently_deleted_is_hidden_but_kept_then_purged() {
        let (dir, mut lib) = setup();
        lib.apply_scan(&[asset("a", "1"), asset("b", "1")], &[], &[], "t1").unwrap();
        store(&lib, "a", ORIGINAL, "1", &dir);

        // Deleted on the phone: in Recently Deleted, gone from the library listing.
        let diff = lib.apply_scan(&[asset("b", "1")], &[], &["a".into()], "t2").unwrap();
        assert_eq!((diff.deleted, diff.purged), (1, 0));
        assert_eq!(lib.assets().iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["b"]);
        let db = connect(lib.path()).unwrap();
        assert!(read_blob(&db, "a", ORIGINAL).is_some(), "kept while in Recently Deleted");

        // Restored on the phone: visible again, file still there.
        lib.apply_scan(&[asset("a", "1"), asset("b", "1")], &[], &[], "t3").unwrap();
        assert_eq!(lib.assets().len(), 2);

        // Deleted and then purged from Recently Deleted: gone, file and all.
        lib.apply_scan(&[asset("b", "1")], &[], &[], "t4").unwrap();
        assert!(read_blob(&db, "a", ORIGINAL).is_none(), "purged with the photo");
        assert_eq!(lib.assets().len(), 1);
    }

    #[test]
    fn blobs_stream_in_and_out() {
        let (dir, mut lib) = setup();
        lib.apply_scan(&[asset("a", "1")], &[], &[], "t1").unwrap();
        let big: Vec<u8> = (0..3_500_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("big"), &big).unwrap();
        let db = connect(lib.path()).unwrap();
        write_blob_from(&db, "a", ORIGINAL, Some("1-original"), &dir.join("big")).unwrap();
        let mut out = Vec::new();
        assert_eq!(copy_blob_to(&db, "a", ORIGINAL, &mut out).unwrap(), big.len() as u64);
        assert_eq!(out, big);
        assert_eq!(lib.totals().0, (1, big.len() as u64));
        let parts: i64 = db.query_row("SELECT COUNT(*) FROM blob_parts", [], |r| r.get(0)).unwrap();
        assert_eq!(parts, 3, "3.5 MB in 1 MB parts: first in blobs, three more");

        // Replacing it with a smaller file drops the old parts.
        std::fs::write(dir.join("small"), b"tiny").unwrap();
        write_blob_from(&db, "a", ORIGINAL, Some("1-original"), &dir.join("small")).unwrap();
        let parts: i64 = db.query_row("SELECT COUNT(*) FROM blob_parts", [], |r| r.get(0)).unwrap();
        assert_eq!(parts, 0);
        let mut out = Vec::new();
        copy_blob_to(&db, "a", ORIGINAL, &mut out).unwrap();
        assert_eq!(out, b"tiny");

        // Purging the photo removes its parts too.
        std::fs::write(dir.join("big"), &big).unwrap();
        write_blob_from(&db, "a", ORIGINAL, Some("1-original"), &dir.join("big")).unwrap();
        lib.apply_scan(&[], &[], &[], "t2").unwrap();
        let parts: i64 = db.query_row("SELECT COUNT(*) FROM blob_parts", [], |r| r.get(0)).unwrap();
        assert_eq!(parts, 0);
    }

    #[test]
    fn albums_round_trip() {
        let (_dir, mut lib) = setup();
        let albums = vec![Album { id: "x".into(), name: "Trips/Banff".into(), kind: "user".into(), assets: vec!["b".into(), "a".into()] }];
        lib.apply_scan(&[asset("a", "1"), asset("b", "1")], &albums, &[], "t1").unwrap();
        let got = lib.albums();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].assets, ["b", "a"]);
    }
}
