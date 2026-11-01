use chrono::{DateTime, Utc};

use crate::state::Aliases;
use nookle_core::{
    model::{Item, ParsedQuery},
    query::emoji_keywords,
};

pub fn matches(it: &Item, pq: &ParsedQuery, aliases: &Aliases) -> bool {
    if !pq.kinds.is_empty() && !pq.kinds.contains(&it.kind) {
        return false;
    }
    if !pq.styles.is_empty() && !it.style.is_some_and(|s| pq.styles.contains(&s)) {
        return false;
    }
    if pq.added_after.is_some_and(|a| it.added_at < a)
        || pq.added_before.is_some_and(|b| it.added_at >= b)
    {
        return false;
    }
    let text = it.text.to_lowercase();
    let authors: String = it
        .sources
        .iter()
        .filter_map(|s| s.author.as_deref())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    // An emoji tag (😏) also answers to its Telegram keywords (smirk) and to the words the tagger
    // learned for it (smug), whole words only. Words map to emoji, never emoji back out to words.
    let tag_hit = |t: &str| {
        let alias = aliases.get(t);
        it.tags.iter().any(|x| {
            x.starts_with(t)
                || x.split(' ').any(|w| w.starts_with(t))
                || emoji_keywords(x).is_some_and(|ks| ks.iter().any(|k| k == t))
                || alias.is_some_and(|a| a.contains(x))
        })
    };
    if !pq
        .terms
        .iter()
        .all(|t| tag_hit(t) || text.contains(t.as_str()) || authors.contains(t.as_str()))
    {
        return false;
    }
    let hay = format!("{} {text}", it.tags.join(" "));
    if !pq.phrases.iter().all(|p| hay.contains(p.as_str())) {
        return false;
    }
    pq.emoji.iter().all(|e| {
        it.tags.contains(&e.emoji)
            || e.keywords.iter().any(|w| {
                let w = w.to_lowercase();
                it.tags.contains(&w) || has_word(&text, &w)
            })
    })
}

fn has_word(text: &str, w: &str) -> bool {
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after = text[i + w.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

pub fn sort(items: &mut [Item], how: &str) {
    let epoch = DateTime::<Utc>::MIN_UTC;
    match how {
        "recent" => {
            items.sort_by_key(|i| std::cmp::Reverse((i.last_used_at.unwrap_or(epoch), i.added_at)))
        }
        "added" => items.sort_by_key(|i| std::cmp::Reverse(i.added_at)),
        _ => items.sort_by_key(|i| {
            std::cmp::Reverse((i.hits, i.last_used_at.unwrap_or(epoch), i.added_at))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nookle_core::query::parse;

    #[test]
    fn emoji_tag_matches_its_keywords() {
        let it: Item = serde_json::from_value(serde_json::json!({
            "id": "abcd1234", "kind": "image", "mime": "image/webp",
            "url": "u", "thumb_url": "t", "page_url": "p", "width": 1, "height": 1,
            "duration_ms": null, "has_audio": false, "size": 1, "style": null,
            "tags": ["cat", "😏"], "text": "", "sources": [], "hits": 0,
            "added_at": "2026-10-30T12:00:00Z", "last_used_at": null,
            "status": "ready", "analysis": "done", "telegram": null
        }))
        .unwrap();
        let aliases = Aliases::from([("smug".to_string(), vec!["😏".to_string()])]);
        let hit = |q: &str| matches(&it, &parse(q, Utc::now()), &aliases);
        assert!(hit("smirk"));
        assert!(hit("😏"));
        assert!(hit("cat smirk"));
        assert!(hit("smug"));
        assert!(!hit("smu"));
        assert!(!hit("smi"));
        assert!(!hit("dog"));
    }
}
