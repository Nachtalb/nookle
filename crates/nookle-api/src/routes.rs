use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{FromRequestParts, Multipart, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use nookle_core::model::{
    Analysis, AnalysisReport, ApiError, Claim, ImportRequest, ImportResponse, ImportResult,
    ImportStatus, Item, ItemPage, Site, Source, Status, Style,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::ingest::{self, Outcome};
use crate::search;
use crate::state::{App, Scope, parse_scope};

type S = State<Arc<App>>;

pub struct ApiErr(StatusCode, ApiError);

impl ApiErr {
    pub fn new(status: u16, code: &str, msg: &str) -> Self {
        ApiErr(
            StatusCode::from_u16(status).unwrap(),
            ApiError {
                error: code.into(),
                message: msg.into(),
                dup_match: None,
                distance: None,
                existing: None,
            },
        )
    }
    pub fn internal(e: anyhow::Error) -> Self {
        tracing::error!("internal error: {e:#}");
        Self::new(500, "internal", "internal error")
    }
    fn not_found() -> Self {
        Self::new(404, "not_found", "no such item")
    }
    fn bad(msg: &str) -> Self {
        Self::new(400, "bad_request", msg)
    }
}

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<nookle_fetch::Error> for ApiErr {
    fn from(e: nookle_fetch::Error) -> Self {
        use nookle_fetch::Error as E;
        match e {
            E::Unsupported(m) | E::Blocked(m) => Self::new(422, "unsupported_url", &m),
            E::Upstream(m) => Self::new(502, "upstream", &m),
            E::TooLarge => Self::new(413, "too_large", "file too large"),
            E::Io(e) => Self::internal(e.into()),
        }
    }
}

type R<T> = Result<T, ApiErr>;

pub struct Auth {
    pub scope: Scope,
    pub key: Option<String>,
}

impl Auth {
    fn need(&self, s: Scope) -> R<()> {
        if self.scope >= s {
            Ok(())
        } else {
            Err(ApiErr::new(403, "forbidden", "not allowed with this key"))
        }
    }
}

fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| {
            c.trim()
                .strip_prefix("nookle_session=")
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

impl FromRequestParts<Arc<App>> for Auth {
    type Rejection = ApiErr;
    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> R<Self> {
        let bearer = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let found = match bearer {
            Some(k) => app.key_auth(k.trim()).map(|(s, id)| (s, Some(id))),
            None => cookie(&parts.headers).and_then(|sid| app.session(&sid)),
        };
        let (scope, key) =
            found.ok_or_else(|| ApiErr::new(401, "unauthorized", "login or send a Bearer key"))?;
        Ok(Auth { scope, key })
    }
}

// ---------- auth & keys ----------

#[derive(Deserialize)]
pub struct Login {
    password: String,
}

pub async fn login(State(app): S, Json(b): Json<Login>) -> R<Response> {
    let (scope, key) = if constant_eq(&b.password, &app.cfg.password) {
        (Scope::Admin, None)
    } else {
        let (s, id) = app
            .key_auth(&b.password)
            .ok_or_else(|| ApiErr::new(401, "unauthorized", "wrong password or key"))?;
        (s, Some(id))
    };
    let sid = app.new_session(scope, key);
    let secure = if app.cfg.public_url.starts_with("https") {
        "; Secure"
    } else {
        ""
    };
    let c =
        format!("nookle_session={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{secure}");
    Ok((
        [(header::SET_COOKIE, c)],
        Json(json!({"scope": scope})).into_response(),
    )
        .into_response())
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub async fn logout(State(app): S, headers: HeaderMap) -> Response {
    if let Some(sid) = cookie(&headers) {
        app.drop_session(&sid);
    }
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, "nookle_session=; Path=/; Max-Age=0")],
    )
        .into_response()
}

pub async fn me(State(app): S, auth: Auth) -> Json<Value> {
    let key = auth
        .key
        .and_then(|id| app.keys().into_iter().find(|k| k.id == id))
        .map(|k| json!({"id": k.id, "name": k.name}));
    Json(json!({"scope": auth.scope, "key": key}))
}

pub async fn keys_list(State(app): S, auth: Auth) -> R<Json<Value>> {
    auth.need(Scope::Admin)?;
    Ok(Json(json!({"keys": app.keys()})))
}

#[derive(Deserialize)]
pub struct NewKey {
    name: String,
    scope: String,
}

pub async fn keys_create(
    State(app): S,
    auth: Auth,
    Json(b): Json<NewKey>,
) -> R<(StatusCode, Json<Value>)> {
    auth.need(Scope::Admin)?;
    let scope = parse_scope(&b.scope)
        .filter(|s| *s != Scope::Admin)
        .ok_or_else(|| ApiErr::bad("scope must be ro or rw"))?;
    if b.name.trim().is_empty() {
        return Err(ApiErr::bad("name is required"));
    }
    let (key, secret) = app.create_key(b.name.trim(), scope);
    let mut v = serde_json::to_value(key).unwrap();
    v["key"] = secret.into();
    Ok((StatusCode::CREATED, Json(v)))
}

pub async fn keys_delete(State(app): S, auth: Auth, Path(id): Path<String>) -> R<StatusCode> {
    auth.need(Scope::Admin)?;
    if app.delete_key(&id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiErr::new(404, "not_found", "no such key"))
    }
}

// ---------- items ----------

#[derive(Deserialize)]
pub struct ListQ {
    q: Option<String>,
    sort: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

pub async fn items_list(State(app): S, _auth: Auth, Query(p): Query<ListQ>) -> R<Json<ItemPage>> {
    let sort = p.sort.as_deref().unwrap_or("used");
    if !matches!(sort, "used" | "recent" | "added") {
        return Err(ApiErr::bad("sort must be used|recent|added"));
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 200);
    let offset: usize = match p.cursor.as_deref() {
        Some(c) => c.parse().map_err(|_| ApiErr::bad("bad cursor"))?,
        None => 0,
    };
    let pq = nookle_core::query::parse(p.q.as_deref().unwrap_or(""), Utc::now());
    let aliases = app.aliases.read().unwrap();
    let mut hits: Vec<Item> = app
        .items
        .read()
        .unwrap()
        .values()
        .filter(|r| search::matches(&r.item, &pq, &aliases))
        .map(|r| r.item.clone())
        .collect();
    search::sort(&mut hits, sort);
    let next_cursor = (offset + limit < hits.len()).then(|| (offset + limit).to_string());
    let items: Vec<Item> = hits.into_iter().skip(offset).take(limit).collect();
    let highlights = items
        .iter()
        .map(|it| (it.id.clone(), search::highlight(it, &pq, &aliases)))
        .filter(|(_, h)| !h.tags.is_empty() || !h.text.is_empty())
        .collect();
    drop(aliases);
    Ok(Json(ItemPage {
        items,
        next_cursor,
        query: pq,
        highlights,
    }))
}

pub async fn items_get(State(app): S, _auth: Auth, Path(id): Path<String>) -> R<Json<Item>> {
    app.get(&id)
        .map(|r| Json(r.item))
        .ok_or_else(ApiErr::not_found)
}

pub fn source_from_url(url: &str) -> Source {
    let u = reqwest::Url::parse(url).ok();
    let host = u
        .as_ref()
        .and_then(|u| u.host_str())
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_string();
    let site = if [
        "x.com",
        "twitter.com",
        "fxtwitter.com",
        "vxtwitter.com",
        "fixupx.com",
        "mobile.twitter.com",
        "mobile.x.com",
    ]
    .contains(&host.as_str())
    {
        Site::X
    } else if host == "t.me" || host == "telegram.me" {
        Site::Telegram
    } else {
        Site::Web
    };
    let first = u
        .as_ref()
        .and_then(|u| u.path_segments())
        .and_then(|mut s| s.next())
        .filter(|s| !s.is_empty());
    let author = (site != Site::Web)
        .then_some(first)
        .flatten()
        .filter(|s| !["i", "c", "s"].contains(s))
        .map(str::to_string);
    Source {
        url: url.to_string(),
        site,
        author,
    }
}

fn dup_err(m: nookle_core::model::DupMatch, d: u32, item: Item) -> ApiErr {
    let mut e = ApiErr::new(409, "duplicate", &format!("duplicate of {}", item.id));
    e.1.dup_match = Some(m);
    e.1.distance = Some(d);
    e.1.existing = Some(Box::new(item));
    e
}

pub async fn items_upload(
    State(app): S,
    auth: Auth,
    mut mp: Multipart,
) -> R<(StatusCode, Json<Item>)> {
    auth.need(Scope::Rw)?;
    let (mut file, mut sources, mut tags, mut force) = (None, vec![], vec![], false);
    while let Some(mut field) = mp
        .next_field()
        .await
        .map_err(|e| ApiErr::bad(&e.to_string()))?
    {
        let fname = field.name().unwrap_or("").to_string();
        match fname.as_str() {
            "file" => {
                if file.is_some() {
                    return Err(ApiErr::bad("send exactly one 'file' part"));
                }
                let path = ingest::tmp_path(&app);
                let mut f = tokio::fs::File::create(&path)
                    .await
                    .map_err(|e| ApiErr::internal(e.into()))?;
                let mut size = 0u64;
                let res: R<()> = async {
                    while let Some(chunk) = field
                        .chunk()
                        .await
                        .map_err(|e| ApiErr::bad(&e.to_string()))?
                    {
                        size += chunk.len() as u64;
                        if size > app.cfg.max_upload {
                            return Err(ApiErr::new(413, "too_large", "file too large"));
                        }
                        f.write_all(&chunk)
                            .await
                            .map_err(|e| ApiErr::internal(e.into()))?;
                    }
                    f.flush().await.map_err(|e| ApiErr::internal(e.into()))
                }
                .await;
                if let Err(e) = res {
                    let _ = tokio::fs::remove_file(&path).await;
                    return Err(e);
                }
                file = Some(path);
            }
            name => {
                let v = field
                    .text()
                    .await
                    .map_err(|e| ApiErr::bad(&e.to_string()))?;
                match name {
                    "source" if !v.trim().is_empty() => sources.push(source_from_url(v.trim())),
                    "tags" => tags.extend(v.split(',').map(str::to_string)),
                    "force" => force = matches!(v.as_str(), "1" | "true"),
                    _ => {}
                }
            }
        }
    }
    let file = file.ok_or_else(|| ApiErr::bad("send exactly one 'file' part"))?;
    match ingest::ingest(&app, file, sources, tags, force).await? {
        Outcome::Created(item) => Ok((StatusCode::CREATED, Json(item))),
        Outcome::Dup(m, d, item) => Err(dup_err(m, d, item)),
    }
}

pub async fn items_import(
    State(app): S,
    auth: Auth,
    Json(b): Json<ImportRequest>,
) -> R<Json<ImportResponse>> {
    auth.need(Scope::Rw)?;
    let resolved = app.fetcher.resolve(&b.url).await?;
    let n = resolved.media.len();
    let want = b.media.clone().unwrap_or_else(|| (1..=n).collect());
    if want.iter().any(|i| !(1..=n).contains(i)) {
        return Err(ApiErr::bad(&format!("media indexes must be 1..{n}")));
    }
    let mut results = vec![];
    for i in want {
        let path = ingest::tmp_path(&app);
        if let Err(e) = app
            .fetcher
            .download(&resolved.media[i - 1].url, &path, app.cfg.max_upload)
            .await
        {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(e.into());
        }
        let out = ingest::ingest(
            &app,
            path,
            vec![resolved.source.clone()],
            b.tags.clone(),
            b.force,
        )
        .await?;
        results.push(match out {
            Outcome::Created(item) => ImportResult {
                index: i,
                status: ImportStatus::Created,
                item: Some(item),
                dup_match: None,
                existing: None,
            },
            Outcome::Dup(m, _, item) => ImportResult {
                index: i,
                status: ImportStatus::Duplicate,
                item: None,
                dup_match: Some(m),
                existing: Some(item),
            },
        });
    }
    Ok(Json(ImportResponse { results }))
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum SourceIn {
    Url(String),
    Obj(Source),
}

#[derive(Deserialize)]
pub struct Patch {
    tags: Option<Vec<String>>,
    text: Option<String>,
    #[serde(default, deserialize_with = "serde_with_none::deserialize")]
    style: Option<Option<Style>>,
    sources: Option<Vec<SourceIn>>,
}

mod serde_with_none {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

pub fn clean_tags(tags: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Drops the words an emoji in `tags` already says: its Telegram keywords (`smirk` next to 😏)
/// and alias words mapped to it (`smug` -> 😏). The emoji are what search finds them by anyway.
pub fn drop_covered(tags: Vec<String>, aliases: &crate::state::Aliases) -> Vec<String> {
    let emoji: Vec<&String> = tags
        .iter()
        .filter(|t| nookle_core::query::split_emoji(t).is_some())
        .collect();
    if emoji.is_empty() {
        return tags;
    }
    let covered = |w: &str| {
        emoji.iter().any(|e| {
            nookle_core::query::emoji_keywords(e).is_some_and(|k| k.iter().any(|k| k == w))
        }) || aliases
            .get(w)
            .is_some_and(|m| m.iter().any(|e| emoji.contains(&e)))
    };
    let keep: Vec<String> = tags
        .iter()
        .filter(|t| nookle_core::query::split_emoji(t).is_some() || !covered(t))
        .cloned()
        .collect();
    keep
}

pub async fn items_patch(
    State(app): S,
    auth: Auth,
    Path(id): Path<String>,
    Json(b): Json<Patch>,
) -> R<Json<Item>> {
    auth.need(Scope::Rw)?;
    app.update(&id, |r| {
        let i = &mut r.item;
        if let Some(t) = b.tags {
            i.tags = clean_tags(t);
        }
        if let Some(t) = b.text {
            i.text = t;
        }
        if let Some(s) = b.style {
            i.style = s;
        }
        if let Some(s) = b.sources {
            i.sources = s
                .into_iter()
                .map(|s| match s {
                    SourceIn::Url(u) => source_from_url(&u),
                    SourceIn::Obj(o) => o,
                })
                .collect();
        }
    })
    .map(Json)
    .ok_or_else(ApiErr::not_found)
}

pub async fn items_delete(State(app): S, auth: Auth, Path(id): Path<String>) -> R<StatusCode> {
    auth.need(Scope::Rw)?;
    let rec = app.get(&id).ok_or_else(ApiErr::not_found)?;
    app.delete_rec(&id);
    let media = app.cfg.media();
    let _ = tokio::fs::remove_file(ingest::stored_path(&app, &rec.item)).await;
    let _ = tokio::fs::remove_file(media.join(format!("{id}.thumb.webp"))).await;
    ingest::purge_cdn(&app, &rec.item).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Queues an item for the tagger again; its previous AI tags are replaced, hand-made ones stay.
pub async fn items_reanalyse(State(app): S, auth: Auth, Path(id): Path<String>) -> R<Json<Item>> {
    auth.need(Scope::Rw)?;
    app.leases.lock().unwrap().remove(&id);
    app.update(&id, |r| r.item.analysis = Analysis::Pending)
        .map(Json)
        .ok_or_else(ApiErr::not_found)
}

/// Queues every ready item for the tagger again.
pub async fn analysis_requeue(State(app): S, auth: Auth) -> R<Json<Value>> {
    auth.need(Scope::Rw)?;
    let ids: Vec<String> = app.items.read().unwrap().keys().cloned().collect();
    let mut n = 0;
    for id in ids {
        app.update(&id, |r| {
            if r.item.status == Status::Ready && r.item.analysis != Analysis::Pending {
                r.item.analysis = Analysis::Pending;
                n += 1;
            }
        });
    }
    Ok(Json(json!({ "queued": n })))
}

pub async fn blocked_get(State(app): S, _auth: Auth) -> Json<Value> {
    Json(json!({ "tags": *app.blocked.read().unwrap() }))
}

/// Replaces the list of tags the tagger may never add; newly blocked ones leave every item.
pub async fn blocked_put(State(app): S, auth: Auth, Json(b): Json<BlockedBody>) -> R<Json<Value>> {
    auth.need(Scope::Rw)?;
    let tags = app.set_blocked(clean_tags(b.tags));
    Ok(Json(json!({ "tags": tags })))
}

#[derive(Deserialize)]
pub struct BlockedBody {
    tags: Vec<String>,
}

pub async fn aliases_get(State(app): S, _auth: Auth) -> Json<Value> {
    Json(json!({ "aliases": *app.aliases.read().unwrap() }))
}

/// Adds or replaces word -> emoji aliases. Unknown emoji are dropped; `[]` records "no emoji fits".
pub async fn aliases_put(
    State(app): S,
    auth: Auth,
    Json(b): Json<std::collections::HashMap<String, Vec<String>>>,
) -> R<Json<Value>> {
    auth.need(Scope::Rw)?;
    for (word, emoji) in b {
        let word = word.trim().to_lowercase();
        if word.is_empty() || word.chars().count() > 40 {
            return Err(ApiErr::bad("alias words must be 1-40 characters"));
        }
        let mut emoji: Vec<String> = emoji
            .into_iter()
            .map(|e| e.replace('\u{fe0f}', ""))
            .filter(|e| nookle_core::query::emoji_keywords(e).is_some())
            .collect();
        emoji.dedup();
        app.set_alias(&word, Some(&emoji));
    }
    app.sweep_covered();
    Ok(aliases_get(State(app), auth).await)
}

pub async fn aliases_delete(State(app): S, auth: Auth, Path(word): Path<String>) -> R<StatusCode> {
    auth.need(Scope::Rw)?;
    app.set_alias(&word.to_lowercase(), None);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn items_hit(State(app): S, _auth: Auth, Path(id): Path<String>) -> R<StatusCode> {
    app.update(&id, |r| {
        r.item.hits += 1;
        r.item.last_used_at = Some(Utc::now());
    })
    .map(|_| StatusCode::NO_CONTENT)
    .ok_or_else(ApiErr::not_found)
}

#[derive(Deserialize)]
pub struct TagsQ {
    q: Option<String>,
    limit: Option<usize>,
}

pub async fn tags(State(app): S, _auth: Auth, Query(p): Query<TagsQ>) -> Json<Value> {
    let pre = p.q.unwrap_or_default().to_lowercase();
    let mut counts = std::collections::HashMap::<String, u64>::new();
    for r in app.items.read().unwrap().values() {
        for t in r.item.tags.iter().filter(|t| t.starts_with(&pre)) {
            *counts.entry(t.clone()).or_default() += 1;
        }
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(p.limit.unwrap_or(20).clamp(1, 200));
    Json(
        json!({"tags": v.into_iter().map(|(tag, count)| json!({"tag": tag, "count": count})).collect::<Vec<_>>()}),
    )
}

// ---------- analysis ----------

const LEASE: Duration = Duration::from_secs(300);

pub async fn analysis_claim(State(app): S, auth: Auth) -> R<Response> {
    auth.need(Scope::Rw)?;
    let now = Instant::now();
    let mut leases = app.leases.lock().unwrap();
    leases.retain(|_, until| *until > now);
    let pick = app
        .items
        .read()
        .unwrap()
        .values()
        .filter(|r| {
            r.item.analysis == Analysis::Pending
                && r.item.status == Status::Ready
                && !leases.contains_key(&r.item.id)
        })
        .min_by_key(|r| r.item.added_at)
        .map(|r| r.item.clone());
    let Some(item) = pick else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    leases.insert(item.id.clone(), now + LEASE);
    let frame_url = format!("{}/_frame/{}.png", app.cfg.public_url, item.id);
    Ok(Json(Claim { item, frame_url }).into_response())
}

pub async fn analysis_submit(
    State(app): S,
    auth: Auth,
    Path(id): Path<String>,
    Json(b): Json<AnalysisReport>,
) -> R<Json<Item>> {
    auth.need(Scope::Rw)?;
    app.leases.lock().unwrap().remove(&id);
    app.update(&id, |r| match b {
        AnalysisReport::Done { tags, text, style } => {
            let old = std::mem::take(&mut r.ai_tags);
            let kept = r.item.tags.drain(..).filter(|t| !old.contains(t));
            let blocked = app.blocked.read().unwrap();
            r.ai_tags = drop_covered(
                clean_tags(
                    tags.into_iter()
                        .filter(|t| !blocked.contains(&t.trim().to_lowercase())),
                ),
                &app.aliases.read().unwrap(),
            );
            r.item.tags = clean_tags(kept.chain(r.ai_tags.iter().cloned()));
            r.item.text = text;
            r.item.style = Some(style);
            r.item.analysis = Analysis::Done;
        }
        AnalysisReport::Failed { error } => {
            tracing::warn!(id = r.item.id, "analysis failed: {error}");
            r.item.analysis = Analysis::Failed;
        }
    })
    .map(Json)
    .ok_or_else(ApiErr::not_found)
}

pub async fn cors_preflight() -> Response {
    let mut r = StatusCode::NO_CONTENT.into_response();
    let h = r.headers_mut();
    h.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("GET, POST, PATCH, DELETE, OPTIONS"),
    );
    h.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("Authorization, Content-Type"),
    );
    h.insert("access-control-max-age", HeaderValue::from_static("86400"));
    r
}
