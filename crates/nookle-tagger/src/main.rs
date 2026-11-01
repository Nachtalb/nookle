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
objects and actions shown, what the text is about, the meme template name if you recognise it, \
and the characters and franchise if you recognise them. Single words or short phrases. Never \
describe colours, fonts, layout or image quality, never use meme, image or text, and never put \
emotions or expressions here.\n\
- emotions: 1 to 4 lowercase single words for the emotion, mood or facial expression shown \
(smug, crying, shocked, ...).";

/// The fixed head of every mapper request. Kept byte-identical so llama.cpp reuses its KV cache
/// and only the few new words get processed.
fn mapper_prompt() -> String {
    format!(
        "You map emotion and expression words to emoji for a meme search. For each word in the \
         user's JSON list, answer with the emoji from the list below that someone thinking of that \
         word would search with, best match first, at most 3, or [] if none fits. Answer only a \
         JSON object {{\"word\": [\"emoji\", ...]}}.\n\n{}",
        nookle_core::query::emoji_listing()
    )
}

/// Slots of the 27B server: image analysis in one, the mapper's cached prefix in the other.
const SLOT_TAGS: u32 = 0;
const SLOT_MAPPER: u32 = 1;

/// Outer error: transient (VLM/network), the lease just expires. Inner error: the item itself is bad.
type Outcome = anyhow::Result<anyhow::Result<AnalysisReport>>;

struct Worker {
    http: reqwest::Client,
    api: String,
    key: String,
    vlm: String,
    model: String,
    /// Models (of other apps) that must not be loaded when we start: they win the GPU.
    yield_to: Vec<String>,
    moss: Option<String>,
    mapper: String,
}

#[derive(Deserialize)]
struct Answer {
    tags: Vec<String>,
    #[serde(default)]
    emotions: Vec<String>,
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
        model: std::env::var("VLM_MODEL").unwrap_or_else(|_| "vlm".into()),
        yield_to: std::env::var("YIELD_TO")
            .unwrap_or_default()
            .split(',')
            .filter(|m| !m.is_empty())
            .map(String::from)
            .collect(),
        moss: std::env::var("MOSS_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .map(|u| u.trim_end_matches('/').to_string()),
        mapper: mapper_prompt(),
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
        if let Some(busy) = self.gpu_busy().await {
            info!("{busy} holds the GPU, not claiming");
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

    /// What else uses the GPU right now (MuskyImageBot's models win), if anything.
    async fn gpu_busy(&self) -> Option<String> {
        if let Some(moss) = &self.moss {
            let v: Option<serde_json::Value> = async {
                let r = self
                    .http
                    .get(format!("{moss}/healthz"))
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await
                    .ok()?;
                r.json().await.ok()
            }
            .await;
            if v.is_some_and(|v| v["loaded"] == true) {
                return Some("MOSS".into());
            }
        }
        if self.yield_to.is_empty() {
            return None;
        }
        let r = self
            .http
            .get(format!("{}/models", self.vlm))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        let v: serde_json::Value = r.json().await.ok()?;
        v["data"].as_array()?.iter().find_map(|m| {
            let id = m["id"].as_str()?;
            let state = m["status"]["value"].as_str()?;
            (self.yield_to.iter().any(|y| y == id) && state != "unloaded").then(|| id.to_string())
        })
    }

    /// POSTs a chat completion, waiting out the 503s the router sends while a model loads.
    async fn chat(&self, body: &serde_json::Value) -> anyhow::Result<(StatusCode, String)> {
        let started = Instant::now();
        let r = loop {
            let r = self
                .http
                .post(format!("{}/v1/chat/completions", self.vlm))
                .json(body)
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
        Ok((status, text))
    }

    /// Emotion words -> emoji: known words from the API's alias table, the rest asked from the
    /// model once and saved there. Words no emoji fits stay as plain tags.
    async fn emotions_to_tags(&self, words: Vec<String>) -> anyhow::Result<Vec<String>> {
        let words = normalise(words);
        if words.is_empty() {
            return Ok(words);
        }
        let known: std::collections::HashMap<String, Vec<String>> = self
            .http
            .get(format!("{}/api/aliases", self.api))
            .bearer_auth(&self.key)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v["aliases"].clone()).ok())
            .context("aliases response")?;
        let unknown: Vec<&String> = words.iter().filter(|w| !known.contains_key(*w)).collect();
        let mut learned = std::collections::HashMap::new();
        if !unknown.is_empty() {
            let body = json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": self.mapper},
                    {"role": "user", "content": serde_json::to_string(&unknown)?},
                ],
                "temperature": 0,
                "max_tokens": 40 * unknown.len() + 20,
                "cache_prompt": true,
                "id_slot": SLOT_MAPPER,
                "chat_template_kwargs": {"enable_thinking": false},
                "response_format": {"type": "json_object"},
            });
            let (status, text) = self.chat(&body).await?;
            anyhow::ensure!(status.is_success(), "mapper {status}: {text}");
            let v: serde_json::Value = serde_json::from_str(&text)?;
            let content = v["choices"][0]["message"]["content"]
                .as_str()
                .context("mapper: no content")?;
            // A malformed answer would repeat on retry (temperature 0): keep the words as plain tags.
            let Ok(map) =
                serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(content)
            else {
                warn!(content, "mapper answer is not a word map");
                return Ok(resolve(&words, &known, &learned));
            };
            for w in &unknown {
                let mut emoji: Vec<String> = map
                    .get(*w)
                    .into_iter()
                    .flatten()
                    .map(|e| e.replace('\u{fe0f}', ""))
                    .filter(|e| nookle_core::query::emoji_keywords(e).is_some())
                    .collect();
                emoji.dedup();
                emoji.truncate(3);
                learned.insert((*w).clone(), emoji);
            }
            info!(?learned, "learned emotion aliases");
            self.http
                .put(format!("{}/api/aliases", self.api))
                .bearer_auth(&self.key)
                .json(&learned)
                .send()
                .await?
                .error_for_status()?;
        }
        Ok(resolve(&words, &known, &learned))
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
            "model": self.model,
            "id_slot": SLOT_TAGS,
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
                    "emotions": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 4},
                },
                "required": ["text", "style", "tags", "emotions"],
            }}},
        });
        let (status, text) = self.chat(&body).await?;
        let a: Answer = match (|| {
            anyhow::ensure!(status.is_success(), "VLM {status}: {text}");
            let v: serde_json::Value = serde_json::from_str(&text)?;
            let content = v["choices"][0]["message"]["content"]
                .as_str()
                .context("VLM: no content")?;
            serde_json::from_str(content)
                .with_context(|| format!("VLM answer is not the schema: {content}"))
        })() {
            Ok(a) => a,
            Err(e) => return Ok(Err(e)),
        };
        let emotions = self.emotions_to_tags(a.emotions).await?;
        Ok(Ok(AnalysisReport::Done {
            tags: normalise(a.tags.into_iter().chain(emotions).collect()),
            text: a.text.trim().to_string(),
            style: a.style,
        }))
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

/// Each emotion word becomes its emoji, or stays a word when no emoji fits (`[]`) or it is unknown.
fn resolve(
    words: &[String],
    known: &std::collections::HashMap<String, Vec<String>>,
    learned: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut out = Vec::new();
    for w in words {
        match known.get(w).or_else(|| learned.get(w)) {
            Some(e) if !e.is_empty() => out.extend(e.iter().cloned()),
            _ => out.push(w.clone()),
        }
    }
    normalise(out)
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
    fn emotions_resolve_to_emoji() {
        let m = |k: &str, v: &[&str]| (k.to_string(), v.iter().map(|s| s.to_string()).collect());
        let known = std::collections::HashMap::from([m("smug", &["😏"]), m("meh", &[])]);
        let learned = std::collections::HashMap::from([m("sobbing", &["😭", "😢"])]);
        let words = ["smug", "meh", "sobbing", "odd"].map(String::from);
        assert_eq!(
            resolve(&words, &known, &learned),
            ["😏", "meh", "😭", "😢", "odd"]
        );
        let p = mapper_prompt();
        assert_eq!(p, mapper_prompt());
        assert!(p.contains("😏 ") && p.len() > 50_000);
    }

    #[test]
    fn flattens_transparency_onto_white() {
        let png = flatten(include_bytes!("../../../samples/inputs/transparent.png")).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.color(), image::ColorType::Rgb8);
        assert_eq!(img.to_rgb8().get_pixel(0, 0).0, [255, 255, 255]);
    }
}
