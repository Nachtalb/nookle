//! End-to-end through the router with real media processing. Skips without ffmpeg.
use std::sync::{Arc, Mutex, RwLock};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::state::{App, Config, open_db};

const SAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../samples/inputs");

fn app() -> (Arc<App>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nookle-test-{}", crate::state::random_id(8)));
    let cfg = Config {
        public_url: "http://nookle.test".into(),
        data: dir.clone(),
        password: "pw".into(),
        bot_token: None,
        tg_api: String::new(),
        tg_chat: None,
        cf_zone: None,
        cf_token: None,
        max_upload: 50 << 20,
    };
    let crate::state::Loaded {
        db,
        items,
        aliases,
        blocked,
    } = open_db(&cfg).unwrap();
    let app = Arc::new(App {
        fetcher: nookle_fetch::Fetcher::new(nookle_fetch::Config::default()),
        cfg,
        db: Mutex::new(db),
        items: RwLock::new(items),
        aliases: RwLock::new(aliases),
        blocked: RwLock::new(blocked),
        leases: Mutex::new(Default::default()),
        http: reqwest::Client::new(),
        encode: tokio::sync::Semaphore::new(2),
    });
    (app, dir)
}

struct T {
    app: Arc<App>,
    key: String,
}

impl T {
    async fn call(
        &self,
        method: &str,
        uri: &str,
        body: Body,
        ctype: Option<String>,
    ) -> (StatusCode, Vec<u8>) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.key));
        if let Some(c) = ctype {
            req = req.header(header::CONTENT_TYPE, c);
        }
        let resp = crate::router(self.app.clone())
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        (
            status,
            to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
    }
    async fn json(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (b, c) = match body {
            Some(v) => (
                Body::from(v.to_string()),
                Some("application/json".to_string()),
            ),
            None => (Body::empty(), None),
        };
        let (s, out) = self.call(method, uri, b, c).await;
        (s, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }
    async fn upload(&self, name: &str, fields: &[(&str, &str)]) -> (StatusCode, Value) {
        let b = "XnookleBoundaryX";
        let mut body = Vec::new();
        for (k, v) in fields {
            body.extend(
                format!("--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                    .bytes(),
            );
        }
        body.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\r\n").bytes());
        body.extend(std::fs::read(format!("{SAMPLES}/{name}")).unwrap());
        body.extend(format!("\r\n--{b}--\r\n").bytes());
        let (s, out) = self
            .call(
                "POST",
                "/api/items",
                Body::from(body),
                Some(format!("multipart/form-data; boundary={b}")),
            )
            .await;
        (s, serde_json::from_slice(&out).unwrap())
    }
    async fn ready(&self, id: &str) -> Value {
        for _ in 0..600 {
            let (_, it) = self.json("GET", &format!("/api/items/{id}"), None).await;
            if it["status"] != "processing" {
                return it;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("{id} never finished processing");
    }
    async fn ids(&self, q: &str) -> Vec<String> {
        let q: String = q
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let (_, v) = self
            .json("GET", &format!("/api/items?q={q}&limit=200"), None)
            .await;
        let mut ids: Vec<String> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    }
}

#[tokio::test]
async fn full_flow() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("skipping: ffmpeg not installed");
        return;
    }
    let (app, dir) = app();
    let (admin, secret) = app.create_key("test", crate::state::Scope::Rw);
    let t = T {
        app: app.clone(),
        key: secret,
    };
    let (ro, ro_secret) = app.create_key("ro", crate::state::Scope::Ro);
    let ro_t = T {
        app: app.clone(),
        key: ro_secret,
    };
    let _ = (admin, ro);

    // auth
    assert_eq!(
        ro_t.json("DELETE", "/api/items/whatever", None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        T {
            app: app.clone(),
            key: "nk_nope".into()
        }
        .json("GET", "/api/items", None)
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        t.json("GET", "/api/keys", None).await.0,
        StatusCode::FORBIDDEN
    );

    // ingest every kind
    let mut ids = std::collections::HashMap::new();
    for (name, kind) in [
        ("build_green.png", "image"),
        ("transparent.png", "image"),
        ("dance.gif", "gif"),
        ("silent_short.mp4", "gif"),
        ("silent_long.mp4", "video"),
        ("with_sound.mp4", "video"),
    ] {
        let (s, it) = t
            .upload(
                name,
                &[
                    ("source", "https://x.com/nachtalb/status/1"),
                    ("tags", "Sample, test"),
                ],
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{name}: {it}");
        let it = t.ready(it["id"].as_str().unwrap()).await;
        assert_eq!(
            (it["status"].as_str(), it["kind"].as_str()),
            (Some("ready"), Some(kind)),
            "{name}: {it}"
        );
        assert_eq!(it["sources"][0]["author"], "nachtalb");
        let id = it["id"].as_str().unwrap().to_string();
        let path = it["url"]
            .as_str()
            .unwrap()
            .trim_start_matches("http://nookle.test")
            .to_string();
        let (s, bytes) = t.call("GET", &path, Body::empty(), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(bytes.len() as u64, it["size"].as_u64().unwrap());
        assert_eq!(
            t.call("GET", &format!("/{id}/thumb.webp"), Body::empty(), None)
                .await
                .0,
            StatusCode::OK
        );
        let (s, html) = t.call("GET", &format!("/{id}"), Body::empty(), None).await;
        assert!(s.is_success() && String::from_utf8_lossy(&html).contains("og:"));
        ids.insert(name, id);
    }

    // duplicates
    let (s, e) = t.upload("build_green.png", &[]).await;
    assert_eq!(
        (s, e["match"].as_str()),
        (StatusCode::CONFLICT, Some("exact"))
    );
    let (s, e) = t.upload("build_green_near_dup.jpg", &[]).await;
    assert_eq!(
        (s, e["match"].as_str(), e["existing"]["id"].as_str()),
        (
            StatusCode::CONFLICT,
            Some("near"),
            Some(ids["build_green.png"].as_str())
        )
    );
    assert_eq!(t.upload("x_photo.jpg", &[]).await.0, StatusCode::CREATED);

    // search
    assert_eq!(t.ids("video").await.len(), 2);
    assert_eq!(t.ids("new gif").await.len(), 2);
    assert!(t.ids("old").await.is_empty());
    assert_eq!(t.ids("sample image").await.len(), 2);

    // analysis + text search
    let (s, claim) = t.json("POST", "/api/analysis/claim", None).await;
    assert_eq!(s, StatusCode::OK);
    let id = claim["item"]["id"].as_str().unwrap().to_string();
    let frame = claim["frame_url"]
        .as_str()
        .unwrap()
        .trim_start_matches("http://nookle.test")
        .to_string();
    let (s, png) = t.call("GET", &frame, Body::empty(), None).await;
    assert_eq!((s, &png[..4]), (StatusCode::OK, &b"\x89PNG"[..]));
    let (_, it) = t
        .json(
            "POST",
            &format!("/api/items/{id}/analysis"),
            Some(json!({"tags": ["Celebration"], "text": "finally green", "style": "anime"})),
        )
        .await;
    assert_eq!(it["analysis"], "done");
    assert_eq!(t.ids("anime 🥳 finally").await, vec![id.clone()]);

    // learned aliases: a word finds the emoji it maps to; a re-analysis swaps only AI tags
    let (_, a) = t
        .json(
            "PUT",
            "/api/aliases",
            Some(json!({"Smug": ["😏", "nope"], "meh": []})),
        )
        .await;
    assert_eq!(a["aliases"], json!({"smug": ["😏"], "meh": []}));
    assert!(t.ids("smug").await.is_empty());
    t.json(
        "PATCH",
        &format!("/api/items/{id}"),
        Some(json!({"tags": ["celebration", "mine"]})),
    )
    .await;
    let (_, it) = t
        .json("POST", &format!("/api/items/{id}/reanalyse"), None)
        .await;
    assert_eq!(it["analysis"], "pending");
    let (_, it) = t
        .json(
            "POST",
            &format!("/api/items/{id}/analysis"),
            Some(json!({"tags": ["😏"], "text": "finally green", "style": "anime"})),
        )
        .await;
    assert_eq!(it["tags"], json!(["mine", "😏"]));

    // blocked tags leave every item and are dropped from later analyses
    let (_, b) = t
        .json(
            "PUT",
            "/api/blocked-tags",
            Some(json!({"tags": ["Mine", "face"]})),
        )
        .await;
    assert_eq!(b["tags"], json!(["face", "mine"]));
    let (_, it) = t.json("GET", &format!("/api/items/{id}"), None).await;
    assert_eq!(it["tags"], json!(["😏"]));
    t.json("POST", &format!("/api/items/{id}/reanalyse"), None)
        .await;
    let (_, it) = t
        .json(
            "POST",
            &format!("/api/items/{id}/analysis"),
            Some(json!({"tags": ["😏", "Face", "cat"], "text": "finally green", "style": "anime"})),
        )
        .await;
    assert_eq!(it["tags"], json!(["cat", "😏"]));
    t.json("PUT", "/api/blocked-tags", Some(json!({"tags": []})))
        .await;
    assert_eq!(t.ids("smug").await, vec![id.clone()]);

    // words an emoji already says are dropped: 'smirk' is a 😏 keyword, 'sly' an alias of it
    t.json("PUT", "/api/aliases", Some(json!({"sly": ["😏"]})))
        .await;
    t.json("POST", &format!("/api/items/{id}/reanalyse"), None)
        .await;
    let (_, it) = t
        .json(
            "POST",
            &format!("/api/items/{id}/analysis"),
            Some(json!({"tags": ["😏", "smirk", "sly", "cat"], "text": "", "style": "anime"})),
        )
        .await;
    assert_eq!(it["tags"], json!(["cat", "😏"]));
    assert_eq!(
        ro_t.json("PUT", "/api/aliases", Some(json!({"x": []})))
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // hit + sort, patch, delete
    assert_eq!(
        ro_t.json(
            "POST",
            &format!("/api/items/{}/hit", ids["dance.gif"]),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, page) = t.json("GET", "/api/items?limit=1", None).await;
    assert_eq!(page["items"][0]["id"], ids["dance.gif"].as_str());
    assert!(page["next_cursor"].is_string());
    let (_, it) = t
        .json(
            "PATCH",
            &format!("/api/items/{}", ids["transparent.png"]),
            Some(json!({"tags": ["OK"], "style": null, "sources": ["https://t.me/chan/5"]})),
        )
        .await;
    assert_eq!(
        (
            it["tags"].clone(),
            it["style"].clone(),
            it["sources"][0]["site"].clone()
        ),
        (json!(["ok"]), Value::Null, json!("telegram"))
    );
    assert_eq!(
        t.json(
            "DELETE",
            &format!("/api/items/{}", ids["transparent.png"]),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        t.call(
            "GET",
            &format!("/{}.webp", ids["transparent.png"]),
            Body::empty(),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // persistence: reopening the db gives the same items
    let reloaded = open_db(&app.cfg).unwrap().items;
    assert_eq!(reloaded.len(), app.items.read().unwrap().len());
    let _ = std::fs::remove_dir_all(dir);
}
