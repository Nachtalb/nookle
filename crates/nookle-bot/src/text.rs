use nookle_core::model::{
    Analysis, DupMatch, ImportResponse, ImportStatus, Item, Kind, Status, TelegramType,
};
use teloxide::types::{
    FileId, InlineQueryResult, InlineQueryResultCachedMpeg4Gif, InlineQueryResultCachedPhoto,
    InlineQueryResultCachedVideo,
};

/// `#tags` and http(s) URLs in a caption or message text.
pub fn parse_caption(text: &str) -> (Vec<String>, Vec<String>) {
    let mut tags = Vec::new();
    for word in text.split_whitespace() {
        if let Some(rest) = word.strip_prefix('#') {
            let tag: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
                .to_lowercase();
            if !tag.is_empty() && !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    (tags, urls(text))
}

pub fn urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        let w = word.trim_start_matches(['(', '<', '"', '\'']);
        if !(w.starts_with("http://") || w.starts_with("https://")) {
            continue;
        }
        let w = w.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '>', '"', '\'']);
        if w.len() > "https://".len() && !out.iter().any(|u| u == w) {
            out.push(w.to_string());
        }
    }
    out
}

pub fn inline_result(item: &Item) -> Option<InlineQueryResult> {
    let tg = item.telegram.as_ref()?;
    let file = FileId(tg.file_id.clone());
    let id = item.id.clone();
    Some(match tg.kind {
        TelegramType::Photo => InlineQueryResultCachedPhoto::new(id, file).into(),
        TelegramType::Animation => InlineQueryResultCachedMpeg4Gif::new(id, file).into(),
        TelegramType::Video => InlineQueryResultCachedVideo::new(id, file, title(item)).into(),
    })
}

/// Telegram caps `next_offset` at 64 bytes; a longer cursor ends paging.
pub fn next_offset(cursor: Option<String>) -> String {
    cursor.filter(|c| c.len() <= 64).unwrap_or_default()
}

fn title(item: &Item) -> String {
    if !item.tags.is_empty() {
        return item
            .tags
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
    }
    let text: String = item.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return "Video".into();
    }
    text.chars().take(60).collect()
}

fn kind(item: &Item) -> &'static str {
    match item.kind {
        Kind::Image => "image",
        Kind::Gif => "gif",
        Kind::Video => "video",
    }
}

pub fn item_summary(item: &Item) -> String {
    if item.status == Status::Failed {
        return format!(
            "❌ Processing failed: {}",
            item.error.as_deref().unwrap_or("unknown error")
        );
    }
    let tags = match (item.tags.is_empty(), item.analysis) {
        (true, Analysis::Pending) => "⏳ tagging…".to_string(),
        (true, _) => "none".to_string(),
        (false, _) => item.tags.join(", "),
    };
    format!("✅ {}\nKind: {}\nTags: {tags}", item.page_url, kind(item))
}

pub fn duplicate_text(dup: DupMatch, distance: Option<u32>, existing: &Item) -> String {
    match dup {
        DupMatch::Exact => format!("♻️ Already in the collection: {}", existing.page_url),
        DupMatch::Near => format!(
            "🤏 Looks like {} (distance {}).",
            existing.page_url,
            distance.map_or("?".into(), |d| d.to_string())
        ),
    }
}

pub fn import_summary(res: &ImportResponse) -> String {
    if res.results.is_empty() {
        return "No media found.".into();
    }
    res.results
        .iter()
        .map(|r| match (r.status, &r.item, &r.existing) {
            (ImportStatus::Created, Some(it), _) => {
                format!("{}. ✅ added {}: {}", r.index, kind(it), it.page_url)
            }
            (ImportStatus::Duplicate, _, Some(it)) => {
                format!("{}. ♻️ already there: {}", r.index, it.page_url)
            }
            _ => format!("{}. {:?}", r.index, r.status),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nookle_core::model::{ImportResult, TelegramFile};

    fn item(tg: Option<(TelegramType, &str)>, tags: &[&str], text: &str) -> Item {
        let mut it: Item = serde_json::from_value(serde_json::json!({
            "id": "abcd1234", "kind": "image", "mime": "image/webp",
            "url": "https://n.naa.gg/abcd1234.webp", "thumb_url": "https://n.naa.gg/abcd1234/thumb.webp",
            "page_url": "https://n.naa.gg/abcd1234", "width": 1, "height": 1, "duration_ms": null,
            "has_audio": false, "size": 1, "style": null, "tags": tags, "text": text, "sources": [],
            "hits": 0, "added_at": "2026-10-30T12:00:00Z", "last_used_at": null,
            "status": "ready", "analysis": "pending", "telegram": null
        }))
        .unwrap();
        it.telegram = tg.map(|(kind, id)| TelegramFile {
            kind,
            file_id: id.into(),
        });
        it
    }

    #[test]
    fn summary_tags() {
        let mut it = item(None, &[], "");
        assert!(item_summary(&it).ends_with("Tags: ⏳ tagging…"));
        it.analysis = Analysis::Done;
        assert!(item_summary(&it).ends_with("Tags: none"));
        it.tags = vec!["cat".into(), "😏".into()];
        assert!(item_summary(&it).ends_with("Tags: cat, 😏"));
    }

    #[test]
    fn caption_tags_and_sources() {
        let (tags, sources) = parse_caption(
            "lol #Cat #cat, #dog_2 # see (https://x.com/a/status/1). and http://e.org/x?y=1 #",
        );
        assert_eq!(tags, ["cat", "dog_2"]);
        assert_eq!(sources, ["https://x.com/a/status/1", "http://e.org/x?y=1"]);
        assert_eq!(parse_caption("nothing here"), (vec![], vec![]));
        assert!(urls("https:// x").is_empty());
    }

    #[test]
    fn inline_mapping() {
        assert!(inline_result(&item(None, &[], "")).is_none());
        let json = |it: &Item| serde_json::to_value(inline_result(it).unwrap()).unwrap();

        let p = json(&item(Some((TelegramType::Photo, "P1")), &[], ""));
        assert_eq!(
            (
                p["type"].as_str(),
                p["id"].as_str(),
                p["photo_file_id"].as_str()
            ),
            (Some("photo"), Some("abcd1234"), Some("P1"))
        );
        let g = json(&item(Some((TelegramType::Animation, "G1")), &[], ""));
        assert_eq!(
            (g["type"].as_str(), g["mpeg4_file_id"].as_str()),
            (Some("mpeg4_gif"), Some("G1"))
        );
        let v = json(&item(
            Some((TelegramType::Video, "V1")),
            &["a", "b", "c", "d"],
            "",
        ));
        assert_eq!(
            (
                v["type"].as_str(),
                v["video_file_id"].as_str(),
                v["title"].as_str()
            ),
            (Some("video"), Some("V1"), Some("a, b, c"))
        );
        let v = json(&item(
            Some((TelegramType::Video, "V1")),
            &[],
            " WHEN\nIT  works ",
        ));
        assert_eq!(v["title"], "WHEN IT works");
        let v = json(&item(Some((TelegramType::Video, "V1")), &[], ""));
        assert_eq!(v["title"], "Video");
        assert_eq!(next_offset(None), "");
        assert_eq!(next_offset(Some("NTA=".into())), "NTA=");
        assert_eq!(next_offset(Some("x".repeat(65))), "");
    }

    #[test]
    fn summaries() {
        let it = item(None, &[], "");
        assert_eq!(
            item_summary(&it),
            "✅ https://n.naa.gg/abcd1234\nKind: image\nTags: ⏳ tagging…"
        );
        assert!(duplicate_text(DupMatch::Near, Some(3), &it).contains("distance 3"));
        let res = ImportResponse {
            results: vec![
                ImportResult {
                    index: 1,
                    status: ImportStatus::Created,
                    item: Some(it.clone()),
                    dup_match: None,
                    existing: None,
                },
                ImportResult {
                    index: 2,
                    status: ImportStatus::Duplicate,
                    item: None,
                    dup_match: Some(DupMatch::Exact),
                    existing: Some(it),
                },
            ],
        };
        assert_eq!(
            import_summary(&res),
            "1. ✅ added image: https://n.naa.gg/abcd1234\n2. ♻️ already there: https://n.naa.gg/abcd1234"
        );
    }
}
