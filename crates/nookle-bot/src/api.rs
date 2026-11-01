use std::path::Path;

use anyhow::{Context, Result, bail};
use nookle_core::model::{ApiError, DupMatch, ImportRequest, ImportResponse, Item, ItemPage};
use reqwest::{Client, RequestBuilder, Response, StatusCode, multipart};

#[derive(Clone)]
pub struct Api {
    http: Client,
    base: String,
    key: String,
}

#[derive(Debug)]
pub enum Upload {
    Created(Box<Item>),
    Duplicate {
        dup_match: DupMatch,
        distance: Option<u32>,
        existing: Box<Item>,
    },
}

impl Api {
    pub fn new(base: &str, key: &str) -> Self {
        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("http client");
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.key)
    }

    pub async fn search(&self, q: &str, cursor: Option<&str>, limit: u32) -> Result<ItemPage> {
        let mut query = vec![("q", q.to_string()), ("limit", limit.to_string())];
        if let Some(c) = cursor {
            query.push(("cursor", c.to_string()));
        }
        let res = self
            .req(reqwest::Method::GET, "/api/items")
            .query(&query)
            .send()
            .await?;
        Ok(ok(res).await?.json().await?)
    }

    pub async fn item(&self, id: &str) -> Result<Item> {
        let res = self
            .req(reqwest::Method::GET, &format!("/api/items/{id}"))
            .send()
            .await?;
        Ok(ok(res).await?.json().await?)
    }

    pub async fn upload(
        &self,
        file: &Path,
        filename: &str,
        sources: &[String],
        tags: &[String],
        force: bool,
    ) -> Result<Upload> {
        let f = tokio::fs::File::open(file).await.context("open upload")?;
        let len = f.metadata().await?.len();
        let mut form = multipart::Form::new().part(
            "file",
            multipart::Part::stream_with_length(f, len).file_name(filename.to_string()),
        );
        for s in sources {
            form = form.text("source", s.clone());
        }
        if !tags.is_empty() {
            form = form.text("tags", tags.join(","));
        }
        if force {
            form = form.text("force", "1");
        }
        let res = self
            .req(reqwest::Method::POST, "/api/items")
            .multipart(form)
            .send()
            .await?;
        if res.status() == StatusCode::CONFLICT {
            let e: ApiError = res.json().await?;
            return match (e.dup_match, e.existing) {
                (Some(dup_match), Some(existing)) => Ok(Upload::Duplicate {
                    dup_match,
                    distance: e.distance,
                    existing,
                }),
                _ => bail!("{}: {}", e.error, e.message),
            };
        }
        Ok(Upload::Created(ok(res).await?.json().await?))
    }

    pub async fn import(&self, req: &ImportRequest) -> Result<ImportResponse> {
        let res = self
            .req(reqwest::Method::POST, "/api/items/import")
            .json(req)
            .send()
            .await?;
        Ok(ok(res).await?.json().await?)
    }

    pub async fn hit(&self, id: &str) -> Result<()> {
        let res = self
            .req(reqwest::Method::POST, &format!("/api/items/{id}/hit"))
            .send()
            .await?;
        ok(res).await?;
        Ok(())
    }
}

async fn ok(res: Response) -> Result<Response> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    match serde_json::from_str::<ApiError>(&body) {
        Ok(e) => bail!("{}", e.message),
        Err(_) => bail!("API answered {status}: {body}"),
    }
}
