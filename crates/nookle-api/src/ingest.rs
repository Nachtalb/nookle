use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use nookle_core::model::{
    Analysis, DupMatch, Item, Kind, Source, Status, TelegramFile, TelegramType,
};
use tokio::io::AsyncReadExt;

use crate::routes::ApiErr;
use crate::state::{App, Rec, fix_urls, random_id};

pub const NEAR_DISTANCE: u32 = 6;

pub enum Outcome {
    Created(Item),
    Dup(DupMatch, u32, Item),
}

async fn blake3_file(path: &Path) -> std::io::Result<String> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            return Ok(h.finalize().to_hex().to_string());
        }
        h.update(&buf[..n]);
    }
}

pub fn tmp_path(app: &App) -> PathBuf {
    app.cfg.tmp().join(random_id(12))
}

/// Takes ownership of `input` (a temp file): it is moved into processing or deleted.
pub async fn ingest(
    app: &Arc<App>,
    input: PathBuf,
    sources: Vec<Source>,
    tags: Vec<String>,
    force: bool,
) -> Result<Outcome, ApiErr> {
    let res = check(app, &input, force).await;
    let (hash, phash, probe) = match res {
        Ok(Ok(v)) => v,
        Ok(Err(dup)) => {
            let _ = tokio::fs::remove_file(&input).await;
            return Ok(dup);
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&input).await;
            return Err(e);
        }
    };
    let id = loop {
        let id = random_id(8);
        if app.get(&id).is_none() {
            break id;
        }
    };
    let mut tags: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    let mut item = Item {
        id: id.clone(),
        kind: probe.kind,
        mime: String::new(),
        url: String::new(),
        thumb_url: String::new(),
        page_url: String::new(),
        width: Some(probe.width),
        height: Some(probe.height),
        duration_ms: probe.duration_ms,
        has_audio: probe.has_audio,
        size: 0,
        style: None,
        tags,
        text: String::new(),
        sources,
        hits: 0,
        added_at: Utc::now(),
        last_used_at: None,
        status: Status::Processing,
        error: None,
        analysis: Analysis::Pending,
        telegram: None,
    };
    fix_urls(&mut item, &app.cfg.public_url);
    let rec = Rec {
        item: item.clone(),
        input_hash: hash,
        phash,
        ai_tags: Vec::new(),
    };
    app.items.write().unwrap().insert(id.clone(), rec.clone());
    app.save(&rec);
    let app = app.clone();
    tokio::spawn(async move {
        let r = process(&app, &id, &input).await;
        let _ = tokio::fs::remove_file(&input).await;
        if let Err(e) = r {
            tracing::error!(id, "processing failed: {e:#}");
            app.update(&id, |r| {
                r.item.status = Status::Failed;
                r.item.error = Some(format!("{e:#}"));
            });
        }
    });
    Ok(Outcome::Created(item))
}

type Checked = (String, Vec<u64>, nookle_media::Probe);

async fn check(app: &App, input: &Path, force: bool) -> Result<Result<Checked, Outcome>, ApiErr> {
    let hash = blake3_file(input)
        .await
        .map_err(|e| ApiErr::internal(e.into()))?;
    if let Some(r) = app
        .items
        .read()
        .unwrap()
        .values()
        .find(|r| r.input_hash == hash)
    {
        return Ok(Err(Outcome::Dup(DupMatch::Exact, 0, r.item.clone())));
    }
    let probe = nookle_media::probe(input).await.map_err(media_err)?;
    let phash = nookle_media::phash(input).await.map_err(media_err)?;
    if !force {
        let best = app
            .items
            .read()
            .unwrap()
            .values()
            .filter(|r| !r.phash.is_empty())
            .map(|r| (nookle_media::distance(&phash, &r.phash), r))
            .filter(|(d, _)| *d <= NEAR_DISTANCE)
            .min_by_key(|(d, _)| *d)
            .map(|(d, r)| (d, r.item.clone()));
        if let Some((d, item)) = best {
            return Ok(Err(Outcome::Dup(DupMatch::Near, d, item)));
        }
    }
    Ok(Ok((hash, phash, probe)))
}

fn media_err(e: nookle_media::Error) -> ApiErr {
    match e {
        nookle_media::Error::Unsupported(m) => ApiErr::new(415, "unsupported_media", &m),
        e => ApiErr::internal(anyhow!(e)),
    }
}

async fn process(app: &Arc<App>, id: &str, input: &Path) -> Result<()> {
    let _permit = app.encode.acquire().await?;
    let stored = nookle_media::normalize(input, &app.cfg.media(), id)
        .await
        .context("normalize")?;
    nookle_media::thumbnail(
        &stored.path,
        &app.cfg.media().join(format!("{id}.thumb.webp")),
    )
    .await
    .context("thumbnail")?;
    app.update(id, |r| {
        let i = &mut r.item;
        i.kind = stored.probe.kind;
        i.width = Some(stored.probe.width);
        i.height = Some(stored.probe.height);
        i.duration_ms = stored.probe.duration_ms;
        i.has_audio = stored.probe.has_audio;
        i.size = stored.size;
        i.status = Status::Ready;
        fix_urls(i, &app.cfg.public_url);
    });
    drop(_permit);
    if let Err(e) = telegram(app, id).await {
        tracing::warn!(id, "telegram upload failed, retried later: {e:#}");
    }
    Ok(())
}

pub fn stored_path(app: &App, item: &Item) -> PathBuf {
    app.cfg
        .media()
        .join(format!("{}.{}", item.id, crate::state::ext(item.kind)))
}

/// Sends the item once to the storage chat, keeps the file_id, deletes the message and the temp copy.
pub async fn telegram(app: &App, id: &str) -> Result<()> {
    let (Some(token), Some(chat)) = (&app.cfg.bot_token, &app.cfg.tg_chat) else {
        return Ok(());
    };
    let item = app.get(id).context("item vanished")?.item;
    let stored_file = stored_path(app, &item);
    let probe = nookle_media::Probe {
        kind: item.kind,
        width: item.width.unwrap_or(0),
        height: item.height.unwrap_or(0),
        duration_ms: item.duration_ms,
        has_audio: item.has_audio,
    };
    let stored = nookle_media::Stored {
        path: stored_file.clone(),
        ext: crate::state::ext(item.kind),
        mime: if item.kind == Kind::Video {
            "video/mp4"
        } else {
            "image/webp"
        },
        probe,
        size: item.size,
    };
    let _permit = app.encode.acquire().await?;
    let copy = nookle_media::telegram_copy(&stored, &app.cfg.tmp()).await?;
    drop(_permit);
    let send = copy.clone().unwrap_or(stored_file);
    let (method, field, kind) = match item.kind {
        Kind::Image => ("sendPhoto", "photo", TelegramType::Photo),
        Kind::Gif => ("sendAnimation", "animation", TelegramType::Animation),
        Kind::Video => ("sendVideo", "video", TelegramType::Video),
    };
    let bytes = tokio::fs::read(&send).await?;
    if let Some(c) = &copy {
        let _ = tokio::fs::remove_file(c).await;
    }
    let name = format!(
        "{id}.{}",
        send.extension().and_then(|e| e.to_str()).unwrap_or("bin")
    );
    let mut form = reqwest::multipart::Form::new()
        .text("chat_id", chat.clone())
        .text("disable_notification", "true")
        .part(
            field,
            reqwest::multipart::Part::bytes(bytes).file_name(name),
        );
    if item.kind != Kind::Image {
        form = form
            .text("width", item.width.unwrap_or(0).to_string())
            .text("height", item.height.unwrap_or(0).to_string())
            .text(
                "duration",
                (item.duration_ms.unwrap_or(0) / 1000).to_string(),
            );
    }
    if item.kind == Kind::Video {
        form = form.text("supports_streaming", "true");
    }
    let base = format!("{}/bot{token}", app.cfg.tg_api);
    let resp: serde_json::Value = app
        .http
        .post(format!("{base}/{method}"))
        .multipart(form)
        .timeout(Duration::from_secs(600))
        .send()
        .await?
        .json()
        .await?;
    if resp["ok"] != true {
        return Err(anyhow!("{method}: {}", resp["description"]));
    }
    let msg = &resp["result"];
    let file_id = match kind {
        TelegramType::Photo => msg["photo"]
            .as_array()
            .and_then(|a| a.last())
            .map(|p| &p["file_id"]),
        TelegramType::Animation => Some(&msg["animation"]["file_id"]),
        TelegramType::Video => Some(&msg["video"]["file_id"]),
    }
    .and_then(|v| v.as_str())
    .with_context(|| format!("{method} returned no {field}: {msg}"))?
    .to_string();
    let _ = app
        .http
        .post(format!("{base}/deleteMessage"))
        .json(&serde_json::json!({"chat_id": chat, "message_id": msg["message_id"]}))
        .send()
        .await;
    app.update(id, |r| {
        r.item.telegram = Some(TelegramFile { kind, file_id })
    });
    Ok(())
}

/// Retries Telegram uploads that failed (bot API down, no token at the time).
pub async fn telegram_retry_loop(app: Arc<App>) {
    loop {
        tokio::time::sleep(Duration::from_secs(600)).await;
        let todo: Vec<String> = app
            .items
            .read()
            .unwrap()
            .values()
            .filter(|r| r.item.status == Status::Ready && r.item.telegram.is_none())
            .map(|r| r.item.id.clone())
            .collect();
        for id in todo {
            if let Err(e) = telegram(&app, &id).await {
                tracing::warn!(id, "telegram retry: {e:#}");
            }
        }
    }
}

pub async fn purge_cdn(app: &App, item: &Item) {
    let (Some(zone), Some(token)) = (&app.cfg.cf_zone, &app.cfg.cf_token) else {
        return;
    };
    let r = app
        .http
        .post(format!(
            "https://api.cloudflare.com/client/v4/zones/{zone}/purge_cache"
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({"files": [item.url, item.thumb_url, item.page_url]}))
        .send()
        .await;
    if let Err(e) = r.and_then(|r| r.error_for_status()) {
        tracing::warn!(id = item.id, "cloudflare purge: {e}");
    }
}
