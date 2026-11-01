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
    // An emoji finds its keywords as whole words in tags and text (🇺🇸: `usa`, and `america`
    // also as the start of `american flag`, since keywords of 5+ letters match word prefixes).
    pq.emoji.iter().all(|e| {
        it.tags.contains(&e.emoji)
            || e.keywords.iter().any(|w| {
                let w = w.to_lowercase();
                it.tags.iter().any(|t| has_word(t, &w, w.len() >= 5)) || has_word(&text, &w, false)
            })
    })
}

pub use nookle_core::model::Highlight;

pub fn highlight(it: &Item, pq: &ParsedQuery, aliases: &Aliases) -> Highlight {
    let text = it.text.to_lowercase();
    let mut h = Highlight::default();
    let mut tag = |x: &String| {
        if !h.tags.contains(x) {
            h.tags.push(x.clone());
        }
    };
    for t in &pq.terms {
        let alias = aliases.get(t);
        it.tags
            .iter()
            .filter(|x| {
                x.starts_with(t.as_str())
                    || x.split(' ').any(|w| w.starts_with(t.as_str()))
                    || emoji_keywords(x).is_some_and(|ks| ks.iter().any(|k| k == t))
                    || alias.is_some_and(|a| a.contains(x))
            })
            .for_each(&mut tag);
    }
    for p in &pq.phrases {
        it.tags
            .iter()
            .filter(|x| x.contains(p.as_str()))
            .for_each(&mut tag);
    }
    for e in &pq.emoji {
        it.tags.iter().filter(|x| **x == e.emoji).for_each(&mut tag);
        for w in &e.keywords {
            let w = w.to_lowercase();
            it.tags
                .iter()
                .filter(|t| has_word(t, &w, w.len() >= 5))
                .for_each(&mut tag);
        }
    }
    let mut piece = |w: &str| {
        if !w.is_empty() && text.contains(w) && !h.text.iter().any(|x| x == w) {
            h.text.push(w.to_string());
        }
    };
    pq.terms.iter().for_each(|t| piece(t));
    pq.phrases.iter().for_each(|p| piece(p));
    for e in &pq.emoji {
        for w in &e.keywords {
            let w = w.to_lowercase();
            if has_word(&text, &w, false) {
                piece(&w);
            }
        }
    }
    h
}

/// `w` as a whole word of `text`, or as the start of a word when `prefix` is set.
fn has_word(text: &str, w: &str, prefix: bool) -> bool {
    text.match_indices(w).any(|(i, _)| {
        let before = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after = text[i + w.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        before && (after || prefix)
    })
}

pub fn sort(items: &mut [Item], how: &str) {
    let epoch = DateTime::<Utc>::MIN_UTC;
    match how {
        "recent" => {
            items.sort_by_key(|i| std::cmp::Reverse((i.last_used_at.unwrap_or(epoch), i.added_at)))
        }
        "added" => items.sort_by_key(|i| std::cmp::Reverse(i.added_at)),
        // reviewed first, latest review on top; the rest after, newest first
        "reviewed" => items.sort_by_key(|i| std::cmp::Reverse((i.reviewed_at, i.added_at))),
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
    fn highlights_name_the_matching_tags_and_text() {
        let it: Item = serde_json::from_value(serde_json::json!({
            "id": "abcd1234", "kind": "image", "mime": "image/webp",
            "url": "u", "thumb_url": "t", "page_url": "p", "width": 1, "height": 1,
            "duration_ms": null, "has_audio": false, "size": 1, "style": null,
            "tags": ["cat", "black cat", "😏", "dog"], "text": "When the Cat is smug", "sources": [], "hits": 0,
            "added_at": "2026-10-30T12:00:00Z", "last_used_at": null,
            "status": "ready", "analysis": "done", "telegram": null
        }))
        .unwrap();
        let aliases = Aliases::from([("smug".to_string(), vec!["😏".to_string()])]);
        let h = highlight(&it, &parse("cat smug", Utc::now()), &aliases);
        assert_eq!(h.tags, ["cat", "black cat", "😏"]);
        assert_eq!(h.text, ["cat", "smug"]);
    }

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

    #[test]
    fn flag_emoji_finds_its_country_not_every_flag() {
        let item = |tags: &[&str]| -> Item {
            serde_json::from_value(serde_json::json!({
                "id": "abcd1234", "kind": "image", "mime": "image/webp",
                "url": "u", "thumb_url": "t", "page_url": "p", "width": 1, "height": 1,
                "duration_ms": null, "has_audio": false, "size": 1, "style": null,
                "tags": tags, "text": "", "sources": [], "hits": 0,
                "added_at": "2026-10-30T12:00:00Z", "last_used_at": null,
                "status": "ready", "analysis": "done", "telegram": null
            }))
            .unwrap()
        };
        let hit =
            |tags: &[&str], q: &str| matches(&item(tags), &parse(q, Utc::now()), &Aliases::new());
        assert!(hit(&["american flag", "suit"], "🇺🇸"));
        assert!(hit(&["usa"], "🇺🇸"));
        assert!(!hit(&["german flag"], "🇺🇸"));
        assert!(!hit(&["user"], "🇺🇸"));
    }
}
