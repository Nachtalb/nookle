# Nookle

Self-hosted meme collection: search by keywords, meme text and emoji, a
Telegram inline bot, an x.com userscript and public asset links at
`https://i.naa.gg`.

- `docs/API.md`: the HTTP API every part talks to
- `crates/nookle-core`: API types shared by the backend and all Rust clients
- `crates/nookle-api`: the backend (API, assets, view pages, ingest)
- `crates/nookle-media`: ffmpeg normalisation, Telegram copies, thumbnails, hashes
- `crates/nookle-fetch`: resolves and downloads third-party URLs (x.com via fxtwitter)
- `crates/nookle-tagger`: analysis worker for the desktop VLM
- `crates/nookle-bot`: Telegram bot (@nooklebot)
- `web/`: static PWA, `userscript/`: x.com button
- `mock/`: Python mock of that API for integration tests
- `samples/`: sample uploads, their normalised versions and fixtures
- `assets/emoji_keywords_en.json`: Telegram's English emoji keywords
  (`messages.getEmojiKeywords`)

## Tagger

`nookle-tagger` claims pending items one at a time and tags them with the
llama-server VLM (`/v1/chat/completions`, model `vlm`). Env: `NOOKLE_API` (base
URL), `NOOKLE_KEY` (rw key), `VLM_URL` (e.g. `http://vlm:8080` in the
moss-image network), `POLL_SECS` (idle wait, default 30).
