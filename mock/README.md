# Nookle API mock

Stdlib-only Python stand-in for the real backend, so every client can write
integration tests before the Rust core exists. It implements `docs/API.md`.

```sh
python3 mock/server.py                 # http://127.0.0.1:8787
python3 mock/server.py --port 0        # random port
python3 mock/server.py --web web       # also serve the PWA from web/
python3 -m unittest mock/test_mock.py  # contract self-check
```

Login password `nookle` (env `NOOKLE_PASSWORD`). Seeded keys:

- ro: `nk_rotestrotestrotestrotestrotestro`
- rw: `nk_rwtestrwtestrwtestrwtestrwtestrw`

Seed data comes from `samples/manifest.json` (7 items covering every kind,
style, `new`/`old` and month/year filters). `POST /_mock/reset` restores it.
`MOCK_PROCESS_DELAY` (seconds, default 0.5) is how long uploads stay
`processing`. `MOCK_QUIET=1` silences the request log.

Imports work offline: x.com post URLs resolve from
`samples/fixtures/fxtwitter/<status id>.json`, whose media URLs point at
`sample://inputs/...`. Status `1000000000000000001` has a photo, a video and a
GIF that is already in the collection.

Differences from the real backend:

- No transcoding. Uploads are served back byte for byte.
- Duplicate checks use SHA-256 and an 8×8 average hash, not BLAKE3 and pHash.
- Probing, near-duplicates, thumbnails and analysis frames need `ffmpeg` and
  `ffprobe` on `PATH`. Without them those parts are skipped.
- Telegram file IDs are fake (`MOCK-<type>-<id>`).
- Everything lives in memory.

`samples/make.sh` regenerates all sample media (needs ffmpeg with libwebp,
libsvtav1, libopus, libx264).
