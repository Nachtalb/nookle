use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use nookle_core::model::{Kind, Status};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::ingest::stored_path;
use crate::state::App;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

async fn serve(path: std::path::PathBuf, mime: &'static str, req: Request<Body>) -> Response {
    match ServeFile::new_with_mime(path, &mime.parse().unwrap())
        .oneshot(req)
        .await
    {
        Ok(mut r) => {
            if r.status().is_success() {
                r.headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE));
            }
            r.map(Body::new)
        }
        Err(e) => match e {},
    }
}

/// `/{id}.webp`, `/{id}.mp4` and `/{id}` (view page) share one path segment.
pub async fn asset_or_page(
    State(app): State<Arc<App>>,
    Path(seg): Path<String>,
    req: Request<Body>,
) -> Response {
    let (id, ext) = match seg.split_once('.') {
        Some((id, ext)) => (id, Some(ext)),
        None => (seg.as_str(), None),
    };
    let Some(rec) = app.get(id).filter(|r| r.item.status == Status::Ready) else {
        return not_found();
    };
    let item = rec.item;
    match ext {
        None => page(&item),
        Some(e) if e == crate::state::ext(item.kind) => {
            let mime = if item.kind == Kind::Video {
                "video/mp4"
            } else {
                "image/webp"
            };
            serve(stored_path(&app, &item), mime, req).await
        }
        Some(_) => not_found(),
    }
}

pub async fn thumb(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    req: Request<Body>,
) -> Response {
    if app.get(&id).is_none_or(|r| r.item.status != Status::Ready) {
        return not_found();
    }
    serve(
        app.cfg.media().join(format!("{id}.thumb.webp")),
        "image/webp",
        req,
    )
    .await
}

pub async fn frame(State(app): State<Arc<App>>, Path(seg): Path<String>) -> Response {
    let Some(rec) = seg.strip_suffix(".png").and_then(|id| app.get(id)) else {
        return not_found();
    };
    match nookle_media::frame_png(&stored_path(&app, &rec.item)).await {
        Ok(png) => ([(header::CONTENT_TYPE, "image/png")], png).into_response(),
        Err(e) => {
            tracing::warn!(id = rec.item.id, "frame: {e}");
            not_found()
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// Like the browser's own image view: an image bigger than the window fits it and a click shows it
// at full size, scrolled to where it was clicked (browser zoom can't enlarge a fitted image).
const ZOOM: &str = "const i=document.querySelector('img');if(i){const s=()=>i.classList.toggle('fit',!i.classList.contains('full')&&(i.naturalWidth>innerWidth||i.naturalHeight>innerHeight));i.complete?s():i.onload=s;onresize=s;i.onclick=e=>{if(!i.classList.contains('fit')&&!i.classList.contains('full'))return;const x=e.offsetX/i.width,y=e.offsetY/i.height;i.classList.toggle('full');s();if(i.classList.contains('full'))scrollTo(x*i.naturalWidth-innerWidth/2,y*i.naturalHeight-innerHeight/2)}}";

fn page(it: &nookle_core::model::Item) -> Response {
    let title = esc(&if it.tags.is_empty() {
        it.id.clone()
    } else {
        it.tags
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    });
    let (w, h) = (it.width.unwrap_or(0), it.height.unwrap_or(0));
    let (og, media) = if it.kind == Kind::Video {
        (
            format!(
                r#"<meta property="og:type" content="video.other"><meta property="og:video" content="{u}"><meta property="og:video:type" content="video/mp4"><meta property="og:video:width" content="{w}"><meta property="og:video:height" content="{h}"><meta property="og:image" content="{t}">"#,
                u = it.url,
                t = it.thumb_url
            ),
            format!(
                r#"<video src="{}" poster="{}" controls autoplay loop playsinline></video>"#,
                it.url, it.thumb_url
            ),
        )
    } else {
        (
            format!(
                r#"<meta property="og:type" content="website"><meta property="og:image" content="{}"><meta property="og:image:width" content="{w}"><meta property="og:image:height" content="{h}">"#,
                it.url
            ),
            format!(r#"<img src="{}" alt="{}">"#, it.url, esc(&it.text)),
        )
    };
    Html(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title>
<meta property="og:title" content="{title}">{og}<meta name="twitter:card" content="summary_large_image">
<style>html,body{{margin:0;height:100%;background:#111}}body{{display:grid;place-items:center}}img,video{{max-width:100vw;max-height:100dvh}}img.fit{{cursor:zoom-in}}img.full{{max-width:none;max-height:none;cursor:zoom-out}}body:has(.full){{display:block;width:max-content;height:auto}}</style>
</head><body>{media}<script>{ZOOM}</script></body></html>"#
    ))
    .into_response()
}
