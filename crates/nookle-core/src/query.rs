//! The search query language from docs/API.md.
use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};

use crate::model::{EmojiMatch, Kind, ParsedQuery, Style};

pub const NEW_DAYS: i64 = 30;

static EMOJI: LazyLock<(HashMap<String, Vec<String>>, usize)> = LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct File {
        keywords: HashMap<String, Vec<String>>,
    }
    let f: File =
        serde_json::from_str(include_str!("../../../assets/emoji_keywords_en.json")).unwrap();
    let mut inv: HashMap<String, Vec<String>> = HashMap::new();
    let mut keys: Vec<_> = f.keywords.into_iter().collect();
    keys.sort();
    for (kw, ems) in keys {
        for e in ems {
            inv.entry(e.replace('\u{fe0f}', ""))
                .or_default()
                .push(kw.clone());
        }
    }
    let max = inv.keys().map(|k| k.chars().count()).max().unwrap_or(1);
    (inv, max)
});

/// Telegram keywords for one emoji (variation selectors ignored).
pub fn emoji_keywords(emoji: &str) -> Option<&'static [String]> {
    EMOJI
        .0
        .get(&emoji.replace('\u{fe0f}', ""))
        .map(Vec::as_slice)
}

/// Every emoji with its keywords, one per line (`😏 smirk, smug, …`), sorted, so the text is the same
/// on every call: the tagger's emotion mapper keeps it as a cached prompt prefix.
pub fn emoji_listing() -> String {
    let mut lines: Vec<String> = EMOJI
        .0
        .iter()
        .map(|(e, kws)| format!("{e} {}", kws.join(", ")))
        .collect();
    lines.sort();
    lines.join("\n")
}

/// Greedy split of a token made only of known emoji.
pub fn split_emoji(tok: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = tok.chars().filter(|&c| c != '\u{fe0f}').collect();
    let (map, max) = &*EMOJI;
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < chars.len() {
        for n in (1..=(*max).min(chars.len() - i)).rev() {
            let s: String = chars[i..i + n].iter().collect();
            if map.contains_key(&s) {
                out.push(s);
                i += n;
                continue 'outer;
            }
        }
        return None;
    }
    (!out.is_empty()).then_some(out)
}

fn kind(w: &str) -> Option<Kind> {
    Some(match w {
        "image" | "images" => Kind::Image,
        "gif" | "gifs" => Kind::Gif,
        "video" | "videos" => Kind::Video,
        _ => return None,
    })
}

fn style(w: &str) -> Option<Style> {
    Some(match w {
        "anime" => Style::Anime,
        "cartoon" => Style::Cartoon,
        "photo" | "photograph" => Style::Photo,
        "3d" | "render" => Style::ThreeD,
        "drawing" => Style::Drawing,
        "screenshot" => Style::Screenshot,
        _ => return None,
    })
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

fn year(w: &str) -> Option<i32> {
    (w.len() == 4
        && (w.starts_with("19") || w.starts_with("20"))
        && w.bytes().all(|b| b.is_ascii_digit()))
    .then(|| w.parse().unwrap())
}

fn month_range(y: i32, m: u32) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0).unwrap();
    let end = if m == 12 {
        Utc.with_ymd_and_hms(y + 1, 1, 1, 0, 0, 0)
    } else {
        Utc.with_ymd_and_hms(y, m + 1, 1, 0, 0, 0)
    };
    (start, end.unwrap())
}

enum Tok {
    Phrase(String),
    Word(String),
}

fn tokenize(q: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut rest = q;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return out;
        }
        if let Some(r) = rest.strip_prefix('"')
            && let Some(end) = r.find('"')
        {
            if !r[..end].trim().is_empty() {
                out.push(Tok::Phrase(r[..end].to_lowercase()));
            }
            rest = &r[end + 1..];
            continue;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        out.push(Tok::Word(rest[..end].to_string()));
        rest = &rest[end..];
    }
}

pub fn parse(q: &str, now: DateTime<Utc>) -> ParsedQuery {
    let mut pq = ParsedQuery::default();
    let clamp = |pq: &mut ParsedQuery, a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>| {
        if let Some(a) = a
            && pq.added_after.is_none_or(|x| a > x)
        {
            pq.added_after = Some(a);
        }
        if let Some(b) = b
            && pq.added_before.is_none_or(|x| b < x)
        {
            pq.added_before = Some(b);
        }
    };
    let toks = tokenize(q);
    let mut i = 0;
    while i < toks.len() {
        let tok = &toks[i];
        i += 1;
        let raw = match tok {
            Tok::Phrase(p) => {
                pq.phrases.push(p.clone());
                continue;
            }
            Tok::Word(w) => w,
        };
        let w = raw.to_lowercase();
        if let Some(k) = kind(&w) {
            pq.kinds.push(k);
        } else if let Some(s) = style(&w) {
            pq.styles.push(s);
        } else if w == "new" {
            clamp(&mut pq, Some(now - Duration::days(NEW_DAYS)), None);
        } else if w == "old" {
            clamp(&mut pq, None, Some(now - Duration::days(NEW_DAYS)));
        } else if let Some(y) = year(&w) {
            let (a, _) = month_range(y, 1);
            let (b, _) = month_range(y + 1, 1);
            clamp(&mut pq, Some(a), Some(b));
        } else if let Some((y, m)) = w.split_once('-').and_then(|(y, m)| {
            let m: u32 = m
                .parse()
                .ok()
                .filter(|v| m.len() == 2 && (1..=12).contains(v))?;
            Some((year(y)?, m))
        }) {
            let (a, b) = month_range(y, m);
            clamp(&mut pq, Some(a), Some(b));
        } else if let Some(m) = MONTHS.iter().position(|&n| n == w).map(|p| p as u32 + 1) {
            let y = match toks.get(i) {
                Some(Tok::Word(next)) if year(next).is_some() => {
                    i += 1;
                    year(next).unwrap()
                }
                _ if m <= now.month() => now.year(),
                _ => now.year() - 1,
            };
            let (a, b) = month_range(y, m);
            clamp(&mut pq, Some(a), Some(b));
        } else if let Some(ems) = split_emoji(raw) {
            for e in ems {
                let keywords = emoji_keywords(&e).unwrap_or_default().to_vec();
                pq.emoji.push(EmojiMatch { emoji: e, keywords });
            }
        } else {
            pq.terms.push(w);
        }
    }
    pq.kinds.sort_by_key(|k| *k as u8);
    pq.kinds.dedup();
    pq.styles.sort_by_key(|s| *s as u8);
    pq.styles.dedup();
    pq
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn filters() {
        let now = at("2026-11-01T12:00:00Z");
        let pq = parse("new video Cat \"may\" 😂🔥", now);
        assert_eq!(pq.kinds, vec![Kind::Video]);
        assert_eq!(pq.added_after, Some(at("2026-10-02T12:00:00Z")));
        assert_eq!(pq.terms, vec!["cat"]);
        assert_eq!(pq.phrases, vec!["may"]);
        assert_eq!(
            pq.emoji
                .iter()
                .map(|e| e.emoji.as_str())
                .collect::<Vec<_>>(),
            ["😂", "🔥"]
        );
        assert!(pq.emoji[0].keywords.contains(&"lol".to_string()));

        let pq = parse("march", now);
        assert_eq!(
            (pq.added_after, pq.added_before),
            (
                Some(at("2026-03-01T00:00:00Z")),
                Some(at("2026-04-01T00:00:00Z"))
            )
        );
        let pq = parse("december", now);
        assert_eq!(pq.added_after, Some(at("2025-12-01T00:00:00Z")));
        let pq = parse("march 2024 photograph render", now);
        assert_eq!(pq.added_before, Some(at("2024-04-01T00:00:00Z")));
        assert_eq!(pq.styles, vec![Style::Photo, Style::ThreeD]);
        let pq = parse("2025-03 old", now);
        assert_eq!(
            (pq.added_after, pq.added_before),
            (
                Some(at("2025-03-01T00:00:00Z")),
                Some(at("2025-04-01T00:00:00Z"))
            )
        );
        let pq = parse("2024", now);
        assert_eq!(pq.added_before, Some(at("2025-01-01T00:00:00Z")));
        assert_eq!(parse("2024-13 1999x", now).terms, vec!["2024-13", "1999x"]);
    }
}
