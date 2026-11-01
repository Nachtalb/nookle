use chrono::{DateTime, Utc};
use nookle_core::model::{Item, ParsedQuery};

pub fn matches(it: &Item, pq: &ParsedQuery) -> bool {
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
    let tag_hit = |t: &str| {
        it.tags
            .iter()
            .any(|x| x.starts_with(t) || x.split(' ').any(|w| w.starts_with(t)))
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
