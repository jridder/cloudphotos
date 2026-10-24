//! The local copy of the library listing (SQLite), so the app opens
//! instantly and works offline for anything already cached.

use std::{collections::HashMap, path::Path};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resource {
    pub url: Option<String>,
    pub size: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
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
    /// thumb, preview, original, and video or live when present
    pub resources: HashMap<String, Resource>,
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

pub struct Library {
    db: Connection,
}

impl Library {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS assets (id TEXT PRIMARY KEY, taken REAL NOT NULL, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS albums (pos INTEGER PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
        )?;
        Ok(Self { db })
    }

    /// Newest first.
    pub fn assets(&self) -> Vec<Asset> {
        let Ok(mut stmt) = self.db.prepare("SELECT data FROM assets ORDER BY taken DESC, id") else { return vec![] };
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|data| serde_json::from_str(&data).ok()).collect())
            .unwrap_or_default()
    }

    pub fn albums(&self) -> Vec<Album> {
        let Ok(mut stmt) = self.db.prepare("SELECT data FROM albums ORDER BY pos") else { return vec![] };
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|data| serde_json::from_str(&data).ok()).collect())
            .unwrap_or_default()
    }

    pub fn replace(&mut self, assets: &[Asset], albums: &[Album], sync_token: &str) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM assets", [])?;
        tx.execute("DELETE FROM albums", [])?;
        {
            let mut insert = tx.prepare("INSERT INTO assets (id, taken, data) VALUES (?1, ?2, ?3)")?;
            for asset in assets {
                insert.execute(params![asset.id, asset.taken, serde_json::to_string(asset).unwrap_or_default()])?;
            }
            let mut insert = tx.prepare("INSERT INTO albums (pos, data) VALUES (?1, ?2)")?;
            for (pos, album) in albums.iter().enumerate() {
                insert.execute(params![pos as i64, serde_json::to_string(album).unwrap_or_default()])?;
            }
        }
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('sync_token', ?1)", [sync_token])?;
        tx.commit()
    }

    pub fn update(&self, asset: &Asset) {
        let _ = self.db.execute(
            "UPDATE assets SET data = ?2 WHERE id = ?1",
            params![asset.id, serde_json::to_string(asset).unwrap_or_default()],
        );
    }

    pub fn remove(&self, id: &str) {
        let _ = self.db.execute("DELETE FROM assets WHERE id = ?1", [id]);
    }

    pub fn sync_token(&self) -> Option<String> {
        self.db.query_row("SELECT value FROM meta WHERE key = 'sync_token'", [], |row| row.get(0)).ok()
    }
}
