use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::Instant;

use anyhow::Result;
use chrono::{DateTime, Utc};
use nookle_core::model::{Item, Kind, Status};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub struct Config {
    pub public_url: String,
    pub data: PathBuf,
    pub password: String,
    pub bot_token: Option<String>,
    pub tg_api: String,
    pub tg_chat: Option<String>,
    pub cf_zone: Option<String>,
    pub cf_token: Option<String>,
    pub max_upload: u64,
}

impl Config {
    pub fn media(&self) -> PathBuf {
        self.data.join("media")
    }
    pub fn tmp(&self) -> PathBuf {
        self.data.join("tmp")
    }
}

/// What we keep per item besides the public `Item`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Rec {
    pub item: Item,
    /// Tags the last analysis added, so a re-analysis replaces them and keeps hand-made ones.
    #[serde(default)]
    pub ai_tags: Vec<String>,
    pub input_hash: String,
    pub phash: Vec<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Ro,
    Rw,
    Admin,
}

#[derive(Clone, Serialize)]
pub struct Key {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

pub struct App {
    pub cfg: Config,
    pub db: Mutex<Connection>,
    pub items: RwLock<HashMap<String, Rec>>,
    pub leases: Mutex<HashMap<String, Instant>>,
    /// Emotion word -> emoji, learned by the tagger (`[]` = no emoji fits). Search maps words through it.
    pub aliases: RwLock<HashMap<String, Vec<String>>>,
    /// Tags never kept from the tagger, managed by the owner (`expression`, `face`, …).
    pub blocked: RwLock<Vec<String>>,
    pub http: reqwest::Client,
    pub fetcher: nookle_fetch::Fetcher,
    pub encode: tokio::sync::Semaphore,
}

pub fn random_id(n: usize) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("os rng");
    buf.iter()
        .map(|b| ABC[(*b as usize) % ABC.len()] as char)
        .collect()
}

pub fn hash_secret(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

pub fn ext(kind: Kind) -> &'static str {
    if kind == Kind::Video { "mp4" } else { "webp" }
}

/// Recomputes the URL fields so a changed public URL never leaves stale links.
pub fn fix_urls(item: &mut Item, base: &str) {
    let id = &item.id;
    item.url = format!("{base}/{id}.{}", ext(item.kind));
    item.mime = if item.kind == Kind::Video {
        "video/mp4"
    } else {
        "image/webp"
    }
    .into();
    item.thumb_url = format!("{base}/{id}/thumb.webp");
    item.page_url = format!("{base}/{id}");
}

pub type Aliases = HashMap<String, Vec<String>>;

pub struct Loaded {
    pub db: Connection,
    pub items: HashMap<String, Rec>,
    pub aliases: Aliases,
    pub blocked: Vec<String>,
}

pub fn open_db(cfg: &Config) -> Result<Loaded> {
    std::fs::create_dir_all(cfg.media())?;
    std::fs::create_dir_all(cfg.tmp())?;
    let db = Connection::open(cfg.data.join("nookle.db"))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, rec TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS keys (id TEXT PRIMARY KEY, name TEXT NOT NULL, scope TEXT NOT NULL,
            hash TEXT NOT NULL UNIQUE, prefix TEXT NOT NULL, created_at TEXT NOT NULL, last_used_at TEXT);
         CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, scope TEXT NOT NULL, key_id TEXT);
         CREATE TABLE IF NOT EXISTS aliases (word TEXT PRIMARY KEY, emoji TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS blocked_tags (tag TEXT PRIMARY KEY);",
    )?;
    let mut items = HashMap::new();
    {
        let mut st = db.prepare("SELECT rec FROM items")?;
        for row in st.query_map([], |r| r.get::<_, String>(0))? {
            let mut rec: Rec = serde_json::from_str(&row?)?;
            fix_urls(&mut rec.item, &cfg.public_url);
            if rec.item.status == Status::Processing {
                rec.item.status = Status::Failed;
                rec.item.error = Some("interrupted by a restart".into());
            }
            items.insert(rec.item.id.clone(), rec);
        }
    }
    let mut aliases = HashMap::new();
    {
        let mut st = db.prepare("SELECT word, emoji FROM aliases")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (word, emoji) = row?;
            aliases.insert(word, serde_json::from_str(&emoji)?);
        }
    }
    let blocked = db
        .prepare("SELECT tag FROM blocked_tags ORDER BY tag")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let _ = std::fs::remove_dir_all(cfg.tmp());
    std::fs::create_dir_all(cfg.tmp())?;
    Ok(Loaded {
        db,
        items,
        aliases,
        blocked,
    })
}

impl App {
    pub fn save(&self, rec: &Rec) {
        let json = serde_json::to_string(rec).unwrap();
        let db = self.db.lock().unwrap();
        if let Err(e) = db.execute(
            "INSERT OR REPLACE INTO items (id, rec) VALUES (?1, ?2)",
            params![rec.item.id, json],
        ) {
            tracing::error!(id = rec.item.id, "saving item: {e}");
        }
    }

    pub fn delete_rec(&self, id: &str) {
        self.items.write().unwrap().remove(id);
        let _ = self
            .db
            .lock()
            .unwrap()
            .execute("DELETE FROM items WHERE id = ?1", [id]);
    }

    /// Mutates an item in memory and persists it; returns the new public item.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut Rec)) -> Option<Item> {
        let rec = {
            let mut items = self.items.write().unwrap();
            let rec = items.get_mut(id)?;
            f(rec);
            rec.clone()
        };
        self.save(&rec);
        Some(rec.item)
    }

    pub fn set_alias(&self, word: &str, emoji: Option<&[String]>) {
        let db = self.db.lock().unwrap();
        let mut aliases = self.aliases.write().unwrap();
        let r = match emoji {
            Some(e) => {
                aliases.insert(word.into(), e.to_vec());
                db.execute(
                    "INSERT OR REPLACE INTO aliases (word, emoji) VALUES (?1, ?2)",
                    params![word, serde_json::to_string(e).unwrap()],
                )
            }
            None => {
                aliases.remove(word);
                db.execute("DELETE FROM aliases WHERE word = ?1", [word])
            }
        };
        if let Err(e) = r {
            tracing::error!(word, "saving alias: {e}");
        }
    }

    /// Drops tagger words that an emoji on the same item already covers (see `drop_covered`),
    /// e.g. after aliases were learned. Hand-made tags stay.
    pub fn sweep_covered(&self) -> usize {
        let aliases = self.aliases.read().unwrap().clone();
        let todo: Vec<(String, Vec<String>)> = self
            .items
            .read()
            .unwrap()
            .values()
            .filter_map(|r| {
                let ai = crate::routes::drop_covered(r.ai_tags.clone(), &aliases);
                // emoji the item already has by hand count as covering too
                let all = crate::routes::drop_covered(
                    r.item
                        .tags
                        .iter()
                        .filter(|t| !r.ai_tags.contains(t))
                        .cloned()
                        .chain(ai)
                        .collect(),
                    &aliases,
                );
                let ai: Vec<String> = all
                    .into_iter()
                    .filter(|t| !r.item.tags.contains(t) || r.ai_tags.contains(t))
                    .collect();
                (ai != r.ai_tags).then(|| (r.item.id.clone(), ai))
            })
            .collect();
        for (id, ai) in &todo {
            self.update(id, |r| {
                let old = std::mem::replace(&mut r.ai_tags, ai.clone());
                r.item.tags.retain(|t| !old.contains(t));
                r.item.tags.extend(ai.iter().cloned());
                r.item.tags.sort();
                r.item.tags.dedup();
            });
        }
        todo.len()
    }

    /// Replaces the blocked list and strips newly blocked tags from every item.
    pub fn set_blocked(&self, tags: Vec<String>) -> Vec<String> {
        let added: Vec<String> = {
            let old = self.blocked.read().unwrap();
            tags.iter().filter(|t| !old.contains(t)).cloned().collect()
        };
        {
            let db = self.db.lock().unwrap();
            let _ = db.execute("DELETE FROM blocked_tags", []);
            for t in &tags {
                let _ = db.execute("INSERT OR IGNORE INTO blocked_tags (tag) VALUES (?1)", [t]);
            }
        }
        *self.blocked.write().unwrap() = tags.clone();
        if !added.is_empty() {
            let ids: Vec<String> = self
                .items
                .read()
                .unwrap()
                .values()
                .filter(|r| r.item.tags.iter().any(|t| added.contains(t)))
                .map(|r| r.item.id.clone())
                .collect();
            for id in ids {
                self.update(&id, |r| {
                    r.item.tags.retain(|t| !added.contains(t));
                    r.ai_tags.retain(|t| !added.contains(t));
                });
            }
        }
        tags
    }

    pub fn get(&self, id: &str) -> Option<Rec> {
        self.items.read().unwrap().get(id).cloned()
    }

    pub fn key_auth(&self, secret: &str) -> Option<(Scope, String)> {
        let db = self.db.lock().unwrap();
        let row: Option<(String, String)> = db
            .query_row(
                "SELECT id, scope FROM keys WHERE hash = ?1",
                [hash_secret(secret)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()?;
        let (id, scope) = row?;
        let _ = db.execute(
            "UPDATE keys SET last_used_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        );
        Some((parse_scope(&scope)?, id))
    }

    pub fn session(&self, sid: &str) -> Option<(Scope, Option<String>)> {
        let db = self.db.lock().unwrap();
        let (scope, key): (String, Option<String>) = db
            .query_row(
                "SELECT scope, key_id FROM sessions WHERE id = ?1",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()??;
        // a session made from a key dies with the key
        if let Some(k) = &key
            && db
                .query_row("SELECT 1 FROM keys WHERE id = ?1", [k], |_| Ok(()))
                .optional()
                .ok()?
                .is_none()
        {
            return None;
        }
        Some((parse_scope(&scope)?, key))
    }

    pub fn new_session(&self, scope: Scope, key: Option<String>) -> String {
        let sid = random_id(32);
        let _ = self.db.lock().unwrap().execute(
            "INSERT INTO sessions (id, scope, key_id) VALUES (?1, ?2, ?3)",
            params![sid, scope_str(scope), key],
        );
        sid
    }

    pub fn drop_session(&self, sid: &str) {
        let _ = self
            .db
            .lock()
            .unwrap()
            .execute("DELETE FROM sessions WHERE id = ?1", [sid]);
    }

    pub fn keys(&self) -> Vec<Key> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT id, name, scope, prefix, created_at, last_used_at FROM keys ORDER BY created_at").unwrap();
        st.query_map([], |r| {
            Ok(Key {
                id: r.get(0)?,
                name: r.get(1)?,
                scope: parse_scope(&r.get::<_, String>(2)?).unwrap_or(Scope::Ro),
                prefix: r.get(3)?,
                created_at: r.get::<_, String>(4)?.parse().unwrap_or_default(),
                last_used_at: r.get::<_, Option<String>>(5)?.and_then(|s| s.parse().ok()),
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn create_key(&self, name: &str, scope: Scope) -> (Key, String) {
        let secret = format!("nk_{}", random_id(32));
        let key = Key {
            id: random_id(6),
            name: name.into(),
            scope,
            prefix: secret[..7].into(),
            created_at: Utc::now(),
            last_used_at: None,
        };
        self.db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO keys (id, name, scope, hash, prefix, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![key.id, key.name, scope_str(scope), hash_secret(&secret), key.prefix, key.created_at.to_rfc3339()],
            )
            .expect("insert key");
        (key, secret)
    }

    pub fn delete_key(&self, id: &str) -> bool {
        let db = self.db.lock().unwrap();
        let _ = db.execute("DELETE FROM sessions WHERE key_id = ?1", [id]);
        db.execute("DELETE FROM keys WHERE id = ?1", [id])
            .unwrap_or(0)
            > 0
    }
}

pub fn parse_scope(s: &str) -> Option<Scope> {
    Some(match s {
        "ro" => Scope::Ro,
        "rw" => Scope::Rw,
        "admin" => Scope::Admin,
        _ => return None,
    })
}

pub fn scope_str(s: Scope) -> &'static str {
    match s {
        Scope::Ro => "ro",
        Scope::Rw => "rw",
        Scope::Admin => "admin",
    }
}
