use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Gif,
    Video,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Anime,
    Cartoon,
    Photo,
    #[serde(rename = "3d")]
    ThreeD,
    Drawing,
    Screenshot,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Processing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Analysis {
    Pending,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Site {
    X,
    Telegram,
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub url: String,
    pub site: Site,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelegramType {
    Photo,
    Animation,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramFile {
    #[serde(rename = "type")]
    pub kind: TelegramType,
    pub file_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub mime: String,
    pub url: String,
    pub thumb_url: String,
    pub page_url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub has_audio: bool,
    pub size: u64,
    pub style: Option<Style>,
    pub tags: Vec<String>,
    pub text: String,
    pub sources: Vec<Source>,
    pub hits: u64,
    pub added_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub analysis: Analysis,
    #[serde(default)]
    pub telegram: Option<TelegramFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmojiMatch {
    pub emoji: String,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedQuery {
    pub kinds: Vec<Kind>,
    pub styles: Vec<Style>,
    pub added_after: Option<DateTime<Utc>>,
    pub added_before: Option<DateTime<Utc>>,
    pub terms: Vec<String>,
    pub phrases: Vec<String>,
    pub emoji: Vec<EmojiMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemPage {
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
    pub query: ParsedQuery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DupMatch {
    Exact,
    Near,
}

/// Body of every non-2xx response. `duplicate` (409) also fills the dup fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
    pub message: String,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub dup_match: Option<DupMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing: Option<Box<Item>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<Vec<usize>>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportStatus {
    Created,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportResult {
    pub index: usize,
    pub status: ImportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<Item>,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub dup_match: Option<DupMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing: Option<Item>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportResponse {
    pub results: Vec<ImportResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub item: Item,
    pub frame_url: String,
}

/// Body of `POST /api/items/{id}/analysis`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnalysisReport {
    Done {
        tags: Vec<String>,
        text: String,
        style: Style,
    },
    Failed {
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_item_roundtrips() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../samples/manifest.json")).unwrap();
        let s = &manifest["items"][0];
        let json = serde_json::json!({
            "id": s["id"], "kind": s["kind"], "mime": "image/webp",
            "url": "https://n.naa.gg/bUiLdGrn.webp", "thumb_url": "https://n.naa.gg/bUiLdGrn/thumb.webp",
            "page_url": "https://n.naa.gg/bUiLdGrn", "width": 640, "height": 480, "duration_ms": null,
            "has_audio": false, "size": 11700, "style": "3d", "tags": s["tags"], "text": s["text"],
            "sources": s["sources"], "hits": 42, "added_at": "2026-10-30T12:00:00Z", "last_used_at": null,
            "status": "ready", "analysis": "done", "telegram": {"type": "photo", "file_id": "x"}
        });
        let item: Item = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(item.style, Some(Style::ThreeD));
        assert_eq!(serde_json::to_value(&item).unwrap(), json);
    }

    #[test]
    fn analysis_report_shapes() {
        let ok: AnalysisReport =
            serde_json::from_str(r#"{"tags":["a"],"text":"","style":"photo"}"#).unwrap();
        assert!(matches!(
            ok,
            AnalysisReport::Done {
                style: Style::Photo,
                ..
            }
        ));
        let err: AnalysisReport = serde_json::from_str(r#"{"error":"vlm down"}"#).unwrap();
        assert!(matches!(err, AnalysisReport::Failed { .. }));
    }
}
