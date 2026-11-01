//! The backend: JSON API, public assets, view pages, ingest and Telegram copies.
mod ingest;
mod public;
mod routes;
mod search;
mod state;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::HeaderValue;
use axum::routing::{get, post};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::{App, Config};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

pub fn router(app: Arc<App>) -> Router {
    let api = Router::new()
        .route("/login", post(routes::login))
        .route("/logout", post(routes::logout))
        .route("/me", get(routes::me))
        .route("/keys", get(routes::keys_list).post(routes::keys_create))
        .route("/keys/{id}", axum::routing::delete(routes::keys_delete))
        .route("/items", get(routes::items_list).post(routes::items_upload))
        .route("/items/import", post(routes::items_import))
        .route(
            "/items/{id}",
            get(routes::items_get)
                .patch(routes::items_patch)
                .delete(routes::items_delete),
        )
        .route("/items/{id}/hit", post(routes::items_hit))
        .route("/items/{id}/analysis", post(routes::analysis_submit))
        .route("/analysis/claim", post(routes::analysis_claim))
        .route("/analysis/requeue", post(routes::analysis_requeue))
        .route("/items/{id}/reanalyse", post(routes::items_reanalyse))
        .route(
            "/aliases",
            get(routes::aliases_get).put(routes::aliases_put),
        )
        .route(
            "/blocked-tags",
            get(routes::blocked_get).put(routes::blocked_put),
        )
        .route(
            "/aliases/{word}",
            axum::routing::delete(routes::aliases_delete),
        )
        .route("/tags", get(routes::tags))
        .route("/{*rest}", axum::routing::options(routes::cors_preflight))
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        ))
        .layer(DefaultBodyLimit::disable());
    Router::new()
        .nest("/api", api)
        .route("/healthz", get(|| async { "ok" }))
        .route("/_frame/{seg}", get(public::frame))
        .route("/{seg}", get(public::asset_or_page))
        .route("/{id}/thumb.webp", get(public::thumb))
        .with_state(app)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(env("RUST_LOG").unwrap_or_else(|| "info".into()))
        .init();
    let cfg = Config {
        public_url: env("NOOKLE_PUBLIC_URL")
            .unwrap_or_else(|| "https://n.naa.gg".into())
            .trim_end_matches('/')
            .into(),
        data: env("NOOKLE_DATA").unwrap_or_else(|| "data".into()).into(),
        password: env("NOOKLE_PASSWORD").context("NOOKLE_PASSWORD is required")?,
        bot_token: env("BOT_TOKEN"),
        tg_api: env("TG_API_URL")
            .unwrap_or_else(|| "https://api.telegram.org".into())
            .trim_end_matches('/')
            .into(),
        tg_chat: env("TG_STORAGE_CHAT"),
        cf_zone: env("CF_ZONE_ID"),
        cf_token: env("CF_API_TOKEN"),
        max_upload: env("NOOKLE_MAX_UPLOAD_MB")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1024)
            << 20,
    };
    let state::Loaded {
        db,
        items,
        aliases,
        blocked,
    } = state::open_db(&cfg)?;
    tracing::info!(items = items.len(), "loaded");
    let workers = env("NOOKLE_WORKERS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let app = Arc::new(App {
        fetcher: nookle_fetch::Fetcher::new(nookle_fetch::Config::default()),
        cfg,
        db: Mutex::new(db),
        items: RwLock::new(items),
        aliases: RwLock::new(aliases),
        blocked: RwLock::new(blocked),
        leases: Mutex::new(HashMap::new()),
        http: reqwest::Client::new(),
        encode: tokio::sync::Semaphore::new(workers),
    });
    let swept = app.sweep_covered();
    if swept > 0 {
        tracing::info!(swept, "dropped words already covered by an emoji");
    }
    tokio::spawn(ingest::telegram_retry_loop(app.clone()));
    let bind = env("NOOKLE_BIND").unwrap_or_else(|| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("listening on {bind}");
    axum::serve(listener, router(app)).await?;
    Ok(())
}
