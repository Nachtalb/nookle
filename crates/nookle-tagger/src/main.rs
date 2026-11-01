//! Analysis worker: claims items from the API and tags them with the desktop VLM.

use std::io::Cursor;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use nookle_core::model::{AnalysisReport, Claim, Style};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use tokio::time::sleep;
use tracing::{info, warn};

const PROMPT: &str = "Analyse this meme image for a search index. Answer as JSON:\n\
- text: all visible text verbatim (OCR), keeping line order; empty string if there is none.\n\
- style: anime, cartoon, photo, 3d, drawing, screenshot or other.\n\
- tags: 5 to 15 lowercase English keywords someone would search to find this meme: the subjects, \
objects and actions shown, what the text is about, the emotion or mood, the meme template name if \
you recognise it, and the characters and franchise if you recognise them. Single words or short \
phrases. Never describe colours, fonts, layout or image quality, and never use meme, image or text.";

/// Outer error: transient (VLM/network), the lease just expires. Inner error: the item itself is bad.
type Outcome = anyhow::Result<anyhow::Result<AnalysisReport>>;

struct Worker {
    http: reqwest::Client,
    api: String,
    key: String,
    vlm: String,
}

#[derive(Deserialize)]
struct Answer {
    tags: Vec<String>,
    text: String,
    style: Style,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let env = |k: &str| std::env::var(k).with_context(|| format!("set {k}"));
    let poll = Duration::from_secs(match std::env::var("POLL_SECS") {
        Ok(v) => v.parse().context("POLL_SECS")?,
        Err(_) => 30,
    });
    let w = Worker {
        http: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(240))
            .build()?,
        api: env("NOOKLE_API")?.trim_end_matches('/').to_string(),
        key: env("NOOKLE_KEY")?,
        vlm: env("VLM_URL")?.trim_end_matches('/').to_string(),
    };
    loop {
        match w.tick().await {
            Ok(true) => {}
            Ok(false) => sleep(poll).await,
            Err(e) => {
                warn!("{e:#}");
                sleep(poll).await;
            }
        }
    }
}

impl Worker {
    /// One claim → analyse → submit. `Ok(false)` = nothing was done, wait before the next try.
    async fn tick(&self) -> anyhow::Result<bool> {
        let up = self
            .http
            .get(format!("{}/health", self.vlm))
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        if !matches!(&up, Ok(r) if r.status().is_success()) {
            info!("VLM not reachable, not claiming");
            return Ok(false);
        }
        let r = self
            .http
            .post(format!("{}/api/analysis/claim", self.api))
            .bearer_auth(&self.key)
            .send()
            .await?
            .error_for_status()?;
        if r.status() == StatusCode::NO_CONTENT {
            return Ok(false);
        }
        let claim: Claim = r.json().await.context("claim response")?;
        let id = claim.item.id;
        let started = Instant::now();
        let report = match self.analyse(&claim.frame_url).await {
            Ok(Ok(report)) => report,
            Ok(Err(e)) => AnalysisReport::Failed {
                error: format!("{e:#}"),
            },
            Err(e) => {
                return Err(e.context(format!("{id}: transient, leaving the lease to expire")));
            }
        };
        self.http
            .post(format!("{}/api/items/{id}/analysis", self.api))
            .bearer_auth(&self.key)
            .json(&report)
            .send()
            .await?
            .error_for_status()?;
        info!(
            id,
            secs = started.elapsed().as_secs_f32(),
            ?report,
            "analysed"
        );
        Ok(true)
    }

    async fn analyse(&self, frame_url: &str) -> Outcome {
        let mut req = self.http.get(frame_url);
        if frame_url.starts_with(&self.api) {
            req = req.bearer_auth(&self.key);
        }
        let r = req.send().await.context("frame")?;
        if r.status().is_server_error() {
            bail!("frame: {}", r.status());
        }
        if !r.status().is_success() {
            return Ok(Err(anyhow!("frame: {}", r.status())));
        }
        let png = match flatten(&r.bytes().await.context("frame")?) {
            Ok(png) => png,
            Err(e) => return Ok(Err(e.context("frame"))),
        };
        let url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        );
        let body = json!({
            "model": "vlm",
            "messages": [{"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": url}},
                {"type": "text", "text": PROMPT},
            ]}],
            "temperature": 0,
            "max_tokens": 800,
            "chat_template_kwargs": {"enable_thinking": false},
            "response_format": {"type": "json_schema", "json_schema": {"name": "analysis", "schema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "style": {"type": "string", "enum": ["anime", "cartoon", "photo", "3d", "drawing", "screenshot", "other"]},
                    "tags": {"type": "array", "items": {"type": "string"}, "minItems": 5, "maxItems": 15},
                },
                "required": ["text", "style", "tags"],
            }}},
        });
        // The VLM answers 503 while it wakes and loads the model.
        let started = Instant::now();
        let r = loop {
            let r = self
                .http
                .post(format!("{}/v1/chat/completions", self.vlm))
                .json(&body)
                .send()
                .await
                .context("VLM")?;
            if r.status() != StatusCode::SERVICE_UNAVAILABLE
                || started.elapsed() > Duration::from_secs(120)
            {
                break r;
            }
            sleep(Duration::from_secs(3)).await;
        };
        let status = r.status();
        let text = r.text().await.context("VLM")?;
        if status.is_server_error() {
            bail!("VLM {status}: {text}");
        }
        Ok((|| {
            anyhow::ensure!(status.is_success(), "VLM {status}: {text}");
            let v: serde_json::Value = serde_json::from_str(&text)?;
            let content = v["choices"][0]["message"]["content"]
                .as_str()
                .context("VLM: no content")?;
            let a: Answer = serde_json::from_str(content)
                .with_context(|| format!("VLM answer is not the schema: {content}"))?;
            Ok(AnalysisReport::Done {
                tags: normalise(a.tags),
                text: a.text.trim().to_string(),
                style: a.style,
            })
        })())
    }
}

/// Flattens alpha onto white (the VLM sees transparency as black) and re-encodes as PNG.
fn flatten(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let img = image::load_from_memory(bytes)?.into_rgba8();
    let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let [r, g, b, a] = img.get_pixel(x, y).0;
        let a = a as u32;
        image::Rgb([r, g, b].map(|c| ((c as u32 * a + 255 * (255 - a)) / 255) as u8))
    });
    let mut out = Cursor::new(Vec::new());
    rgb.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn normalise(tags: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim().to_lowercase();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_tags() {
        let tags = [" Cat", "cat", "", "Happy Dog "].map(String::from).to_vec();
        assert_eq!(normalise(tags), ["cat", "happy dog"]);
    }

    #[test]
    fn flattens_transparency_onto_white() {
        let png = flatten(include_bytes!("../../../samples/inputs/transparent.png")).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.color(), image::ColorType::Rgb8);
        assert_eq!(img.to_rgb8().get_pixel(0, 0).0, [255, 255, 255]);
    }
}
