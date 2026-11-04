# Nookle API

One backend serves the JSON API under `/api`, the public assets and the view
pages. Public base URL: `https://n.naa.gg`. Mock: `python3 mock/server.py`
(see `mock/README.md`).

## Auth

Three scopes:

| scope   | how                                    | can                                      |
|---------|----------------------------------------|------------------------------------------|
| `admin` | the password                           | everything, incl. API key management     |
| `rw`    | rw API key                             | read, upload, import, edit, delete, hit, analysis |
| `ro`    | ro API key                             | read, search, hit                        |

Send a key as `Authorization: Bearer <key>`. Keys look like `nk_<32 base62>`.

The web UI logs in with `POST /api/login {"password": "..."}`. The value may be
the password or any API key, the session gets that scope. Session cookie:
`nookle_session`, `HttpOnly; SameSite=Lax; Secure`. Cookies only work
same-origin, so other origins (The Lounge, userscripts) use the header.

`/api/*` answers CORS with `Access-Control-Allow-Origin: *` and allows the
`Authorization` and `Content-Type` headers, so browser clients on other origins
can use a (ro) key.

Errors are always `{"error": "<code>", "message": "<human text>"}`:
`unauthorized` 401, `forbidden` 403, `not_found` 404, `duplicate` 409,
`unsupported_media` 415, `too_large` 413, `bad_request` 400,
`unsupported_url` 422, `upstream` 502.

```
POST   /api/login    {"password"}           -> 200 {"scope"}  + Set-Cookie
POST   /api/logout                          -> 204
GET    /api/me                              -> 200 {"scope", "key": {"id","name"}|null}
GET    /api/keys                  admin     -> 200 {"keys": [Key]}
POST   /api/keys     {"name","scope"} admin -> 201 Key + "key": "nk_..." (shown once)
DELETE /api/keys/{id}             admin     -> 204
```

`Key = {"id", "name", "scope": "ro"|"rw", "prefix": "nk_AbCd", "created_at", "last_used_at"}`

## Items

```json
{
  "id": "bUiLdGrn",
  "kind": "image",
  "mime": "image/webp",
  "url": "https://n.naa.gg/bUiLdGrn.webp",
  "thumb_url": "https://n.naa.gg/bUiLdGrn/thumb.webp",
  "page_url": "https://n.naa.gg/bUiLdGrn",
  "width": 640, "height": 480, "duration_ms": null, "has_audio": false, "size": 11700,
  "style": "cartoon",
  "tags": ["build", "ci"],
  "text": "WHEN THE BUILD IS FINALLY GREEN",
  "sources": [{"url": "https://x.com/nachtalb/status/1", "site": "x", "author": "nachtalb"}],
  "hits": 42,
  "added_at": "2026-10-30T12:00:00Z",
  "last_used_at": "2026-10-31T12:00:00Z",
  "reviewed_at": null,
  "status": "ready",
  "analysis": "done",
  "telegram": {"type": "photo", "file_id": "AgAC..."}
}
```

- `kind`: `image` | `gif` | `video`. `gif` = anything stored as animated WebP.
- `status`: `processing` → `ready` | `failed` (`"error"` then holds the reason).
  Assets and `telegram` only exist once `ready`.
- `analysis`: `pending` | `done` | `failed`. Runs when the desktop VLM is up.
- `style`: `anime` | `cartoon` | `photo` | `3d` | `drawing` | `screenshot` | `other` | `null` (not analysed yet).
- `site`: `x` | `telegram` | `web`.
- `telegram`: `null` until the Telegram copy has been uploaded.
- Times are RFC 3339 UTC.
- `highlights` (search results only): `{id: {"tags": [...], "text": [...]}}`, the item's tags that matched and the lowercase pieces of its text that matched; items without a match, and empty searches, have no entry.

```
GET    /api/items?q=&sort=&limit=&cursor=   ro  -> 200 {"items", "next_cursor", "query", "highlights"}
GET    /api/items/{id}                      ro  -> 200 Item
POST   /api/items            multipart      rw  -> 201 Item | 409 duplicate
POST   /api/items/import     json           rw  -> 200 {"results": [ImportResult]}
PATCH  /api/items/{id}       json           rw  -> 200 Item
DELETE /api/items/{id}                      rw  -> 204
POST   /api/items/{id}/hit                  ro  -> 204
GET    /api/tags?q=&limit=                  ro  -> 200 {"tags": [{"tag", "count"}]}
```

### List / search

- `sort`: `used` (default: hits desc, then last use, then added), `recent`
  (last used desc, never-used last), `added` (newest first), `unreviewed` (only items not
  yet reviewed, newest first).
- `limit`: 1–200, default 50. `cursor`: opaque, at most 64 bytes (fits Telegram's inline `next_offset`), from `next_cursor` (`null` = end).
- `query` echoes how `q` was understood, so UIs can show it as chips:

```json
{"kinds": ["video"], "styles": [], "added_after": "2026-10-02T00:00:00Z", "added_before": null,
 "terms": ["cat"], "phrases": [], "emoji": [{"emoji": "😂", "keywords": ["lol", "laugh"]}]}
```

### Query language (`q`)

Whitespace-separated tokens, case-insensitive. All tokens must match (AND).

| token | meaning |
|---|---|
| `image` `images` `gif` `gifs` `video` `videos` | kind filter; several kinds = OR |
| `anime` `cartoon` `photo` `photograph` `3d` `render` `drawing` `screenshot` | style filter; several = OR |
| `new` | added in the last 30 days |
| `old` | added more than 30 days ago |
| `2024` | added in that year |
| `2024-03` | added in that month |
| `march` | added in the most recent March (this month counts) |
| `march 2024` | added in March 2024 |
| emoji, e.g. `😂` | the emoji itself as a tag, or any of its Telegram keywords (`assets/emoji_keywords_en.json`) as a whole word in a tag or the text; keywords of 5+ letters also match word starts (`🇺🇸` → `america` finds `american flag`). Keywords shared by more than 40 emoji (`flag`) are skipped |
| `"some phrase"` | literal: never a filter, matched as a phrase |
| anything else | substring match on tags, text and source author (`bleed` finds `nosebleed`); an emoji tag also matches its exact keywords (`smirk` finds items tagged `😏`) and the alias words mapped to it (`smug`, see below) |

Month names are full English names only (`may` is a month, quote it to search
for the word).

### Upload

`POST /api/items`, `multipart/form-data`:

- `file` (required, one per request)
- `source` (optional, repeatable): URL
- `tags` (optional): comma-separated, merged with the analysis tags
- `force` (optional, `1`): store even if it is a near duplicate

Max 1 GiB. Note: Cloudflare's free plan rejects request bodies over 100 MB, so
big uploads only work from the LAN or via import.

`201` returns the item with `status: "processing"`. Poll `GET /api/items/{id}`
until `ready`.

`409`:

```json
{"error": "duplicate", "message": "...", "match": "exact"|"near", "distance": 3, "existing": Item}
```

`exact` (same original bytes) is always rejected. `near` (perceptual hash
distance ≤ 6) can be overridden with `force=1`.

### Import from a URL

The backend downloads the media itself, so the userscript and the bot never
move the files. `POST /api/items/import`:

```json
{"url": "https://x.com/user/status/123", "media": [1, 3], "tags": ["optional"], "force": false}
```

- `media`: 1-based indexes into the post's media, omitted = all.
- x.com / twitter.com / fxtwitter / vxtwitter post URLs are resolved via
  api.fxtwitter.com. The post URL becomes the source.
- Any other `http(s)` URL is fetched directly and must be an image or video.

```json
{"results": [
  {"index": 1, "status": "created", "item": Item},
  {"index": 3, "status": "duplicate", "match": "exact", "existing": Item}
]}
```

`422 unsupported_url` for anything else and for URLs that resolve to private
addresses; `413 too_large` when a file exceeds the upload limit; `502 upstream`
when fxtwitter or the download fails.

### Edit

`PATCH /api/items/{id}` with any of `tags` (replaces), `text`, `style`,
`sources` (replaces, list of URLs or source objects). Any of those sets `reviewed_at` to now;
`reviewed: true|false` sets or clears it on its own. `reviewed_at` stays `null` while only the
tagger has touched an item.

### Hit

`POST /api/items/{id}/hit` records one use (`hits + 1`, `last_used_at = now`).
Assets are cached by Cloudflare and never counted, so every integration that
actually uses an item calls this (the bot does it for chosen inline results).

## Analysis worker

The tagger is an API client with an `rw` key.

```
POST /api/analysis/claim                     rw -> 200 {"item": Item, "frame_url"} | 204 nothing to do
POST /api/items/{id}/analysis                rw -> 200 Item
```

Claim leases the oldest `pending` ready item for 5 minutes. `frame_url` is a
public (no auth) PNG of the first frame, transparency flattened onto white.
Report back with:

```json
{"tags": ["cat", "keyboard"], "text": "ocr text or empty", "style": "photo"}
```

or `{"error": "why"}` to mark it `failed`. A new report replaces the tags the
previous one added; tags added by hand stay.

```
POST /api/items/{id}/reanalyse               rw -> 200 Item (analysis = pending)
POST /api/analysis/requeue                   rw -> 200 {"queued": n} (every ready item)
```

### Blocked tags

Tags the tagger may never add, managed by the owner. Putting a new tag on the
list also removes it from every item; later analyses drop it. Hand-made tags
are not filtered.

```
GET /api/blocked-tags                        any -> {"tags": ["expression", "face"]}
PUT /api/blocked-tags {"tags": [...]}        rw  -> same; replaces the list
```

### Emotion aliases

The tagger stores emotions and expressions as emoji. Its model names them
freely (`smug`, `sobbing`); each word goes through this table, and words it
doesn't know yet are mapped by the model once (against the whole emoji list,
kept as a cached prompt prefix) and saved here. `[]` means no emoji fits; the
word then stays a plain tag. Search maps a word through the table to emoji
(never emoji back out to words), so `smug` finds items tagged `😏`.

```
GET    /api/aliases                          any -> {"aliases": {"smug": ["😏"], "meh": []}}
PUT    /api/aliases  {"word": ["😏", ...]}   rw  -> same; adds/replaces, unknown emoji dropped
DELETE /api/aliases/{word}                   rw  -> 204
```

## Public (no auth, cached forever by Cloudflare)

```
GET /{id}.webp | /{id}.mp4     the stored file, Range supported
GET /{id}/thumb.webp           static WebP, max 320 px wide (first frame for gif/video)
GET /{id}                      HTML view page with OpenGraph tags
GET /healthz                   "ok"
```

## Media normalisation

| input | stored | Telegram (temporary, deleted once the file_id exists) |
|---|---|---|
| still image | WebP q80 | the WebP, `sendPhoto` |
| GIF, any length | animated WebP | H.264 MP4 without audio track, `sendAnimation` |
| video without audio ≤ 20 s | animated WebP (`kind: gif`) | H.264 MP4 without audio track, `sendAnimation` |
| video without audio > 20 s | AV1 MP4 without audio (`kind: video`) | H.264 MP4, `sendVideo` |
| video with audio | AV1 + Opus MP4, faststart (`kind: video`) | H.264 + AAC MP4, faststart, `sendVideo` |

Transparent images stay transparent (WebP keeps alpha). Filenames are always
`<id>.<ext>`. Originals are not kept.
