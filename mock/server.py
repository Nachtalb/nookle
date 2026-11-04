#!/usr/bin/env python3
"""Nookle API mock: in-memory, stdlib only. Implements docs/API.md against samples/."""
import argparse
import base64
import calendar
import email.parser
import email.policy
import hashlib
import json
import os
import re
import secrets
import shutil
import string
import subprocess
import threading
import time
import urllib.parse
from datetime import datetime, timedelta, timezone
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SAMPLES = os.path.join(ROOT, "samples")
PASSWORD = os.environ.get("NOOKLE_PASSWORD", "nookle")
SEED_KEYS = [("ro test", "ro", "nk_" + "rotest" * 5 + "ro"), ("rw test", "rw", "nk_" + "rwtest" * 5 + "rw")]
PROCESS_DELAY = float(os.environ.get("MOCK_PROCESS_DELAY", "0.5"))
NEAR_DISTANCE = 6
LEASE = 300
STYLES = {"anime": "anime", "cartoon": "cartoon", "photo": "photo", "photograph": "photo", "3d": "3d",
          "render": "3d", "drawing": "drawing", "screenshot": "screenshot"}
KINDS = {"image": "image", "images": "image", "gif": "gif", "gifs": "gif", "video": "video", "videos": "video"}
MONTHS = {m.lower(): i for i, m in enumerate(calendar.month_name) if m}
HAVE_FFMPEG = bool(shutil.which("ffmpeg") and shutil.which("ffprobe"))


def now():
    return datetime.now(timezone.utc)


def iso(dt):
    return dt.strftime("%Y-%m-%dT%H:%M:%SZ") if dt else None


def rel_time(v):
    if v is None:
        return None
    m = re.fullmatch(r"-(\d+)([dh])", v)
    if m:
        n = int(m[1])
        return now() - (timedelta(days=n) if m[2] == "d" else timedelta(hours=n))
    return datetime.fromisoformat(v.replace("Z", "+00:00"))


def new_id(n=8):
    return "".join(secrets.choice(string.ascii_letters + string.digits) for _ in range(n))


def load_emoji():
    data = json.load(open(os.path.join(ROOT, "assets", "emoji_keywords_en.json"), encoding="utf-8"))
    inv = {}
    for kw, ems in data["keywords"].items():
        for e in ems:
            inv.setdefault(e.replace("\ufe0f", ""), []).append(kw)
    return inv


EMOJI = load_emoji()
EMOJI_MAXLEN = max(map(len, EMOJI))


def split_emoji(tok):
    """Greedy split of a token made only of emoji; None if it isn't one."""
    tok, out, i = tok.replace("\ufe0f", ""), [], 0
    while i < len(tok):
        for n in range(min(EMOJI_MAXLEN, len(tok) - i), 0, -1):
            if tok[i:i + n] in EMOJI:
                out.append(tok[i:i + n])
                i += n
                break
        else:
            return None
    return out


# ---------- media probing (mock-level, only when ffmpeg exists) ----------

def ffprobe(path):
    if not HAVE_FFMPEG:
        return {}
    r = subprocess.run(["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", path],
                       capture_output=True, text=True)
    return json.loads(r.stdout or "{}")


def frame_png(path):
    if not HAVE_FFMPEG:
        return None
    r = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"],
                       capture_output=True)
    return r.stdout or None


def ahash(path):
    """64-bit average hash of the first frame; stands in for the real pHash."""
    if not HAVE_FFMPEG:
        return None
    r = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-frames:v", "1", "-vf", "scale=8:8,format=gray",
                        "-f", "rawvideo", "-"], capture_output=True)
    px = r.stdout[:64]
    if len(px) != 64:
        return None
    avg = sum(px) / 64
    return sum(1 << i for i, p in enumerate(px) if p > avg)


def sniff(data):
    if data[:8] == b"\x89PNG\r\n\x1a\n" or data[:3] == b"\xff\xd8\xff":
        return "image"
    if data[:6] in (b"GIF87a", b"GIF89a"):
        return "gif"
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "gif" if b"ANMF" in data[:4096] else "image"
    if data[4:8] == b"ftyp" or data[:4] == b"\x1a\x45\xdf\xa3":
        return "video"
    return None


# ---------- store ----------

class Store:
    def __init__(self, base_url):
        self.base_url = base_url.rstrip("/")
        self.lock = threading.RLock()
        self.reset()

    def reset(self):
        with self.lock:
            self.items, self.keys, self.sessions = {}, {}, {}
            self.updir = os.path.join(os.environ.get("TMPDIR", "/tmp"), "nookle-mock-" + new_id(6))
            os.makedirs(self.updir, exist_ok=True)
            for name, scope, key in SEED_KEYS:
                self._add_key(name, scope, key)
            for s in json.load(open(os.path.join(SAMPLES, "manifest.json")))["items"]:
                path = os.path.join(SAMPLES, s["file"])
                it = dict(s, path=path, thumb=os.path.join(SAMPLES, "stored", s["id"] + ".thumb.webp"),
                          added_at=rel_time(s["added_at"]), last_used_at=rel_time(s["last_used_at"]), reviewed_at=None,
                          input_hash=hashlib.sha256(open(os.path.join(SAMPLES, s["input"]), "rb").read()).hexdigest(),
                          phash=ahash(path), ready_at=0, lease=0, error=None,
                          size=os.path.getsize(path))
                self.items[s["id"]] = it

    def _add_key(self, name, scope, key=None):
        key = key or "nk_" + new_id(32)
        kid = new_id(6)
        self.keys[kid] = {"id": kid, "name": name, "scope": scope, "key": key, "created_at": now(), "last_used_at": None}
        return self.keys[kid]

    def key_by_secret(self, secret):
        for k in self.keys.values():
            if secrets.compare_digest(k["key"], secret):
                k["last_used_at"] = now()
                return k
        return None

    def status(self, it):
        if it.get("error"):
            return "failed"
        return "ready" if time.time() >= it["ready_at"] else "processing"

    def public(self, it, authed=True):
        st = self.status(it)
        ext = "mp4" if it["kind"] == "video" else "webp"
        out = {
            "id": it["id"], "kind": it["kind"], "mime": "video/mp4" if ext == "mp4" else "image/webp",
            "url": f"{self.base_url}/{it['id']}.{ext}", "thumb_url": f"{self.base_url}/{it['id']}/thumb.webp",
            "page_url": f"{self.base_url}/{it['id']}", "width": it["width"], "height": it["height"],
            "duration_ms": it["duration_ms"], "has_audio": it["has_audio"], "size": it["size"],
            "style": it["style"], "tags": it["tags"], "text": it["text"], "sources": it["sources"],
            "hits": it["hits"], "added_at": iso(it["added_at"]), "last_used_at": iso(it["last_used_at"]),
            "reviewed_at": iso(it.get("reviewed_at")), "status": st, "analysis": it["analysis"],
        }
        if st == "failed":
            out["error"] = it["error"]
        if authed:
            ttype = {"image": "photo", "gif": "animation", "video": "video"}[it["kind"]]
            out["telegram"] = {"type": ttype, "file_id": "MOCK-" + ttype + "-" + it["id"]} if st == "ready" else None
        return out

    def find_dup(self, data_hash, phash):
        best = None
        for it in self.items.values():
            if it["input_hash"] == data_hash:
                return "exact", 0, it
            if phash is not None and it["phash"] is not None:
                d = bin(phash ^ it["phash"]).count("1")
                if d <= NEAR_DISTANCE and (best is None or d < best[1]):
                    best = ("near", d, it)
        return best

    def ingest(self, data, filename, sources, tags, force):
        kind = sniff(data)
        if not kind:
            raise ApiError(415, "unsupported_media", "not an image, gif or video")
        iid = new_id()
        path = os.path.join(self.updir, iid + os.path.splitext(filename or "")[1].lower())
        with open(path, "wb") as f:
            f.write(data)
        h = hashlib.sha256(data).hexdigest()
        ph = ahash(path)
        with self.lock:
            dup = self.find_dup(h, ph)
            if dup and (dup[0] == "exact" or not force):
                os.unlink(path)
                return None, dup
            probe = ffprobe(path)
            streams = probe.get("streams", [])
            v = next((s for s in streams if s.get("codec_type") == "video"), {})
            has_audio = any(s.get("codec_type") == "audio" for s in streams)
            dur = float(probe.get("format", {}).get("duration") or 0)
            if kind == "video" and not has_audio and dur <= 20:
                kind = "gif"
            it = {"id": iid, "kind": kind, "path": path, "thumb": None, "width": v.get("width"),
                  "height": v.get("height"), "duration_ms": int(dur * 1000) if kind != "image" and dur else None,
                  "has_audio": has_audio, "size": len(data), "style": None, "tags": sorted(set(tags)), "text": "",
                  "sources": [source_obj(s) for s in sources], "hits": 0, "added_at": now(), "last_used_at": None, "reviewed_at": None,
                  "analysis": "pending", "input_hash": h, "phash": ph, "ready_at": time.time() + PROCESS_DELAY,
                  "lease": 0, "error": None}
            self.items[iid] = it
            return it, None


def source_obj(s):
    if isinstance(s, dict):
        return {"url": s["url"], "site": s.get("site") or site_of(s["url"]), "author": s.get("author")}
    return {"url": s, "site": site_of(s), "author": author_of(s)}


def site_of(url):
    host = urllib.parse.urlsplit(url).hostname or ""
    if re.search(r"(^|\.)(x|twitter|fxtwitter|vxtwitter|fixupx)\.com$", host):
        return "x"
    if host in ("t.me", "telegram.me"):
        return "telegram"
    return "web"


def author_of(url):
    p = urllib.parse.urlsplit(url)
    parts = [x for x in p.path.split("/") if x]
    if site_of(url) in ("x", "telegram") and parts and parts[0] not in ("i", "c", "s"):
        return parts[0]
    return None


class ApiError(Exception):
    def __init__(self, status, code, message, **extra):
        super().__init__(message)
        self.status, self.code, self.message, self.extra = status, code, message, extra


# ---------- search ----------

def month_range(y, m):
    start = datetime(y, m, 1, tzinfo=timezone.utc)
    end = datetime(y + (m == 12), m % 12 + 1, 1, tzinfo=timezone.utc)
    return start, end


def parse_query(q):
    out = {"kinds": [], "styles": [], "added_after": None, "added_before": None, "terms": [], "phrases": [], "emoji": []}

    def clamp(a, b):
        if a and (out["added_after"] is None or a > out["added_after"]):
            out["added_after"] = a
        if b and (out["added_before"] is None or b < out["added_before"]):
            out["added_before"] = b

    toks = re.findall(r'"([^"]*)"|(\S+)', q or "")
    i = 0
    while i < len(toks):
        phrase, tok = toks[i]
        i += 1
        if phrase:
            out["phrases"].append(phrase.lower())
            continue
        low = tok.lower()
        if low in KINDS:
            out["kinds"].append(KINDS[low])
        elif low in STYLES:
            out["styles"].append(STYLES[low])
        elif low == "new":
            clamp(now() - timedelta(days=30), None)
        elif low == "old":
            clamp(None, now() - timedelta(days=30))
        elif re.fullmatch(r"(19|20)\d\d", low):
            y = int(low)
            clamp(datetime(y, 1, 1, tzinfo=timezone.utc), datetime(y + 1, 1, 1, tzinfo=timezone.utc))
        elif re.fullmatch(r"(19|20)\d\d-(0[1-9]|1[0-2])", low):
            clamp(*month_range(int(low[:4]), int(low[5:])))
        elif low in MONTHS:
            m, n = MONTHS[low], now()
            if i < len(toks) and re.fullmatch(r"(19|20)\d\d", toks[i][1] or ""):
                y = int(toks[i][1])
                i += 1
            else:
                y = n.year if m <= n.month else n.year - 1
            clamp(*month_range(y, m))
        elif (ems := split_emoji(tok)) is not None:
            out["emoji"] += [{"emoji": e, "keywords": EMOJI[e]} for e in ems]
        else:
            out["terms"].append(low)
    out["kinds"] = sorted(set(out["kinds"]))
    out["styles"] = sorted(set(out["styles"]))
    return out


def matches(it, pq):
    if pq["kinds"] and it["kind"] not in pq["kinds"]:
        return False
    if pq["styles"] and it["style"] not in pq["styles"]:
        return False
    if pq["added_after"] and it["added_at"] < pq["added_after"]:
        return False
    if pq["added_before"] and it["added_at"] >= pq["added_before"]:
        return False
    tags = [t.lower() for t in it["tags"]]
    text = it["text"].lower()
    authors = " ".join((s.get("author") or "").lower() for s in it["sources"])
    hay = " ".join(tags) + " " + text
    for t in pq["terms"]:
        if not (any(t in x for x in tags) or t in text or t in authors):
            return False
    for p in pq["phrases"]:
        if p not in hay:
            return False
    for e in pq["emoji"]:
        words = [k.lower() for k in e["keywords"]]
        if e["emoji"] not in it["tags"] and not any(w in tags or re.search(r"\b" + re.escape(w) + r"\b", text) for w in words):
            return False
    return True


def sort_key(sort):
    epoch = datetime.min.replace(tzinfo=timezone.utc)
    if sort == "recent":
        return lambda it: (it["last_used_at"] or epoch, it["added_at"])
    if sort == "added":
        return lambda it: it["added_at"]
    if sort == "unreviewed":  # only unreviewed items are listed; newest first
        return lambda it: it["added_at"]
    return lambda it: (it["hits"], it["last_used_at"] or epoch, it["added_at"])


# ---------- import ----------

TWEET_RE = re.compile(r"^https?://(?:www\.|mobile\.)?(?:x|twitter|fxtwitter|vxtwitter|fixupx)\.com/([^/]+)/status/(\d+)")


def resolve_import(url):
    """Returns [(bytes, filename)], source. Offline: only fixtures and sample:// URLs."""
    m = TWEET_RE.match(url)
    if not m:
        raise ApiError(422, "unsupported_url", "mock only resolves x.com fixtures in samples/fixtures/fxtwitter")
    fx = os.path.join(SAMPLES, "fixtures", "fxtwitter", m[2] + ".json")
    if not os.path.exists(fx):
        raise ApiError(502, "upstream", f"no fxtwitter fixture for status {m[2]}")
    tweet = json.load(open(fx))["tweet"]
    media = []
    for med in tweet["media"]["all"]:
        p = os.path.join(SAMPLES, med["url"].removeprefix("sample://"))
        media.append((open(p, "rb").read(), os.path.basename(p)))
    src = {"url": tweet["url"], "site": "x", "author": tweet["author"]["screen_name"]}
    return media, src


# ---------- http ----------

ROUTES = []


def route(method, pattern, scope=None):
    def deco(fn):
        ROUTES.append((method, re.compile("^" + pattern + "$"), scope, fn))
        return fn
    return deco


WEB_TYPES = {".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml",
             ".png": "image/png", ".webmanifest": "application/manifest+json", ".json": "application/json"}
SCOPE_RANK = {"ro": 1, "rw": 2, "admin": 3}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "nookle-mock"
    store: Store = None
    web = None

    def log_message(self, fmt, *args):
        if os.environ.get("MOCK_QUIET") != "1":
            super().log_message(fmt, *args)

    # plumbing
    def _send(self, status, body=b"", ctype="application/json", headers=None):
        if isinstance(body, (dict, list)):
            body = json.dumps(body, ensure_ascii=False).encode()
        elif isinstance(body, str):
            body = body.encode()
        self.send_response(status)
        if self.path.startswith("/api/"):
            self.send_header("Access-Control-Allow-Origin", "*")
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        if status != 204:
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD" and status != 204:
            self.wfile.write(body)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def json_body(self):
        raw = self.body()
        try:
            return json.loads(raw or b"{}")
        except ValueError:
            raise ApiError(400, "bad_request", "invalid JSON")

    def auth(self):
        h = self.headers.get("Authorization", "")
        if h.startswith("Bearer "):
            k = self.store.key_by_secret(h[7:].strip())
            return (k["scope"], k) if k else (None, None)
        c = self.headers.get("Cookie", "")
        m = re.search(r"nookle_session=([A-Za-z0-9]+)", c)
        if m and m[1] in self.store.sessions:
            return self.store.sessions[m[1]]
        return None, None

    def do_OPTIONS(self):
        self._send(204, headers={"Access-Control-Allow-Methods": "GET, POST, PATCH, DELETE, OPTIONS",
                                 "Access-Control-Allow-Headers": "Authorization, Content-Type",
                                 "Access-Control-Max-Age": "86400"})

    def dispatch(self):
        path = urllib.parse.urlsplit(self.path).path
        for method, rx, need, fn in ROUTES:
            m = rx.match(path)
            if m and method == ("GET" if self.command == "HEAD" else self.command):
                try:
                    self.scope, self.key = self.auth()
                    if need:
                        if not self.scope:
                            raise ApiError(401, "unauthorized", "login or send a Bearer key")
                        if SCOPE_RANK[self.scope] < SCOPE_RANK[need]:
                            raise ApiError(403, "forbidden", f"needs {need}")
                    with self.store.lock:
                        return fn(self, *m.groups())
                except ApiError as e:
                    return self._send(e.status, {"error": e.code, "message": e.message, **e.extra})
        if self.web and self.command in ("GET", "HEAD"):
            if path == "/emoji-keywords.json":  # the web image copies it in from assets/
                kw = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "emoji_keywords_en.json")
                return self._send(200, open(kw, "rb").read(), "application/json")
            f = os.path.realpath(os.path.join(self.web, path.lstrip("/") or "index.html"))
            if f.startswith(self.web + os.sep) and os.path.isfile(f):
                return self._send(200, open(f, "rb").read(), WEB_TYPES.get(os.path.splitext(f)[1], "application/octet-stream"))
        if any(rx.match(path) for _, rx, _, _ in ROUTES):
            return self._send(405, {"error": "bad_request", "message": "method not allowed"})
        self._send(404, {"error": "not_found", "message": "no such route"})

    do_GET = do_HEAD = do_POST = do_PATCH = do_PUT = do_DELETE = dispatch

    def query(self):
        return {k: v[-1] for k, v in urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query).items()}

    def item(self, iid):
        it = self.store.items.get(iid)
        if not it:
            raise ApiError(404, "not_found", "no such item")
        return it

    def send_file(self, path, ctype):
        size = os.path.getsize(path)
        hdr = {"Cache-Control": "public, max-age=31536000, immutable", "Accept-Ranges": "bytes"}
        rng = re.fullmatch(r"bytes=(\d*)-(\d*)", self.headers.get("Range", ""))
        with open(path, "rb") as f:
            if rng and (rng[1] or rng[2]):
                start = int(rng[1]) if rng[1] else size - int(rng[2])
                end = int(rng[2]) if rng[1] and rng[2] else size - 1
                if start >= size or start > end:
                    return self._send(416, b"", ctype, {"Content-Range": f"bytes */{size}"})
                end = min(end, size - 1)
                f.seek(start)
                hdr["Content-Range"] = f"bytes {start}-{end}/{size}"
                return self._send(206, f.read(end - start + 1), ctype, hdr)
            return self._send(200, f.read(), ctype, hdr)


# ----- routes: auth -----

@route("POST", "/api/login")
def login(h):
    pw = str(h.json_body().get("password", ""))
    if secrets.compare_digest(pw, PASSWORD):
        sess = ("admin", None)
    else:
        k = h.store.key_by_secret(pw)
        if not k:
            raise ApiError(401, "unauthorized", "wrong password or key")
        sess = (k["scope"], k)
    sid = secrets.token_hex(16)
    h.store.sessions[sid] = sess
    h._send(200, {"scope": sess[0]}, headers={"Set-Cookie": f"nookle_session={sid}; Path=/; HttpOnly; SameSite=Lax"})


@route("POST", "/api/logout")
def logout(h):
    m = re.search(r"nookle_session=([A-Za-z0-9]+)", h.headers.get("Cookie", ""))
    if m:
        h.store.sessions.pop(m[1], None)
    h._send(204, headers={"Set-Cookie": "nookle_session=; Path=/; Max-Age=0"})


@route("GET", "/api/me", "ro")
def me(h):
    h._send(200, {"scope": h.scope, "key": {"id": h.key["id"], "name": h.key["name"]} if h.key else None})


def key_out(k):
    return {"id": k["id"], "name": k["name"], "scope": k["scope"], "prefix": k["key"][:7],
            "created_at": iso(k["created_at"]), "last_used_at": iso(k["last_used_at"])}


@route("GET", "/api/keys", "admin")
def keys_list(h):
    h._send(200, {"keys": [key_out(k) for k in h.store.keys.values()]})


@route("POST", "/api/keys", "admin")
def keys_create(h):
    b = h.json_body()
    if b.get("scope") not in ("ro", "rw") or not str(b.get("name", "")).strip():
        raise ApiError(400, "bad_request", "need name and scope ro|rw")
    k = h.store._add_key(b["name"].strip(), b["scope"])
    h._send(201, dict(key_out(k), key=k["key"]))


@route("DELETE", "/api/keys/([A-Za-z0-9]+)", "admin")
def keys_delete(h, kid):
    if not h.store.keys.pop(kid, None):
        raise ApiError(404, "not_found", "no such key")
    h._send(204)


# ----- routes: items -----

@route("GET", "/api/items", "ro")
def items_list(h):
    q = h.query()
    sort = q.get("sort", "used")
    if sort not in ("used", "recent", "added", "unreviewed"):
        raise ApiError(400, "bad_request", "sort must be used|recent|added|unreviewed")
    try:
        limit = max(1, min(200, int(q.get("limit", 50))))
        offset = int(base64.urlsafe_b64decode(q["cursor"]).decode()) if q.get("cursor") else 0
    except (ValueError, UnicodeDecodeError):
        raise ApiError(400, "bad_request", "bad limit or cursor")
    pq = parse_query(q.get("q", ""))
    hits = sorted((it for it in h.store.items.values() if matches(it, pq)
                   and not (sort == "unreviewed" and it.get("reviewed_at"))), key=sort_key(sort), reverse=True)
    page = hits[offset:offset + limit]
    nxt = base64.urlsafe_b64encode(str(offset + limit).encode()).decode() if offset + limit < len(hits) else None
    pq_out = dict(pq, added_after=iso(pq["added_after"]), added_before=iso(pq["added_before"]))
    hl = {}
    for it in page:
        tags = [t for t in it["tags"] if any(term in t for term in pq["terms"])]
        text = [term for term in pq["terms"] if term in it["text"].lower()]
        if tags or text:
            hl[it["id"]] = {"tags": tags, "text": text}
    h._send(200, {"items": [h.store.public(it) for it in page], "next_cursor": nxt, "query": pq_out, "highlights": hl})


@route("GET", "/api/items/([A-Za-z0-9]+)", "ro")
def items_get(h, iid):
    h._send(200, h.store.public(h.item(iid)))


def parse_multipart(h):
    ctype = h.headers.get("Content-Type", "")
    if not ctype.startswith("multipart/form-data"):
        raise ApiError(400, "bad_request", "expected multipart/form-data")
    msg = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(
        b"Content-Type: " + ctype.encode() + b"\r\n\r\n" + h.body())
    fields, files = {}, []
    for part in msg.iter_parts():
        name = part.get_param("name", header="content-disposition")
        if part.get_filename() is not None or name == "file":
            files.append((name, part.get_filename(), part.get_payload(decode=True)))
        else:
            fields.setdefault(name, []).append(part.get_payload(decode=True).decode())
    return fields, files


def dup_error(dup, pub):
    kind, dist, it = dup
    return ApiError(409, "duplicate", f"{kind} duplicate of {it['id']}", match=kind, distance=dist, existing=pub(it))


@route("POST", "/api/items", "rw")
def items_upload(h):
    fields, files = parse_multipart(h)
    files = [f for f in files if f[0] == "file"]
    if len(files) != 1:
        raise ApiError(400, "bad_request", "send exactly one 'file' part")
    tags = [t.strip().lower() for t in ",".join(fields.get("tags", [])).split(",") if t.strip()]
    force = fields.get("force", ["0"])[-1] in ("1", "true")
    it, dup = h.store.ingest(files[0][2], files[0][1], fields.get("source", []), tags, force)
    if dup:
        raise dup_error(dup, h.store.public)
    h._send(201, h.store.public(it))


@route("POST", "/api/items/import", "rw")
def items_import(h):
    b = h.json_body()
    url = str(b.get("url", ""))
    media, src = resolve_import(url)
    want = b.get("media") or list(range(1, len(media) + 1))
    if any(not isinstance(i, int) or not 1 <= i <= len(media) for i in want):
        raise ApiError(400, "bad_request", f"media indexes must be 1..{len(media)}")
    tags = [str(t).lower() for t in b.get("tags", [])]
    results = []
    for i in want:
        data, name = media[i - 1]
        it, dup = h.store.ingest(data, name, [src], tags, bool(b.get("force")))
        if dup:
            results.append({"index": i, "status": "duplicate", "match": dup[0], "existing": h.store.public(dup[2])})
        else:
            results.append({"index": i, "status": "created", "item": h.store.public(it)})
    h._send(200, {"results": results})


@route("PATCH", "/api/items/([A-Za-z0-9]+)", "rw")
def items_patch(h, iid):
    it, b = h.item(iid), h.json_body()
    if "tags" in b:
        it["tags"] = sorted({str(t).strip().lower() for t in b["tags"] if str(t).strip()})
    if "text" in b:
        it["text"] = str(b["text"])
    if "style" in b:
        if b["style"] is not None and b["style"] not in set(STYLES.values()) | {"other"}:
            raise ApiError(400, "bad_request", "unknown style")
        it["style"] = b["style"]
    if "sources" in b:
        it["sources"] = [source_obj(s) for s in b["sources"]]
    if "reviewed" in b:
        it["reviewed_at"] = now() if b["reviewed"] else None
    elif {"tags", "text", "style", "sources"} & b.keys():
        it["reviewed_at"] = now()
    h._send(200, h.store.public(it))


@route("DELETE", "/api/items/([A-Za-z0-9]+)", "rw")
def items_delete(h, iid):
    h.item(iid)
    del h.store.items[iid]
    h._send(204)


@route("POST", "/api/items/([A-Za-z0-9]+)/hit", "ro")
def items_hit(h, iid):
    it = h.item(iid)
    it["hits"] += 1
    it["last_used_at"] = now()
    h._send(204)


@route("GET", "/api/tags", "ro")
def tags(h):
    q = h.query()
    pre = q.get("q", "").lower()
    limit = max(1, min(200, int(q.get("limit", 20))))
    counts = {}
    for it in h.store.items.values():
        for t in it["tags"]:
            if t.startswith(pre):
                counts[t] = counts.get(t, 0) + 1
    top = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[:limit]
    h._send(200, {"tags": [{"tag": t, "count": c} for t, c in top]})


@route("GET", "/api/blocked-tags", "ro")
def blocked_get(h):
    h._send(200, {"tags": sorted(getattr(h.store, "blocked", []))})


@route("PUT", "/api/blocked-tags", "rw")
def blocked_put(h):
    tags = sorted({t.strip().lower() for t in h.json_body().get("tags", []) if t.strip()})
    added = set(tags) - set(getattr(h.store, "blocked", []))
    h.store.blocked = tags
    for it in h.store.items.values():
        it["tags"] = [t for t in it["tags"] if t not in added]
    h._send(200, {"tags": tags})


# ----- routes: analysis -----

@route("POST", "/api/analysis/claim", "rw")
def analysis_claim(h):
    t = time.time()
    todo = [it for it in h.store.items.values()
            if it["analysis"] == "pending" and h.store.status(it) == "ready" and it["lease"] < t]
    if not todo:
        return h._send(204)
    it = min(todo, key=lambda x: x["added_at"])
    it["lease"] = t + LEASE
    h._send(200, {"item": h.store.public(it), "frame_url": f"{h.store.base_url}/_frame/{it['id']}.png"})


@route("POST", "/api/items/([A-Za-z0-9]+)/analysis", "rw")
def analysis_submit(h, iid):
    it, b = h.item(iid), h.json_body()
    if "error" in b:
        it["analysis"] = "failed"
    else:
        style = b.get("style")
        if style not in set(STYLES.values()) | {"other"}:
            raise ApiError(400, "bad_request", "unknown style")
        it["tags"] = sorted(set(it["tags"]) | {str(x).strip().lower() for x in b.get("tags", []) if str(x).strip()})
        it["text"] = str(b.get("text", ""))
        it["style"] = style
        it["analysis"] = "done"
    it["lease"] = 0
    h._send(200, h.store.public(it))


# ----- routes: public -----

@route("GET", "/healthz")
def healthz(h):
    h._send(200, "ok", "text/plain")


@route("GET", r"/([A-Za-z0-9]{8})\.(webp|mp4)")
def asset(h, iid, ext):
    it = h.item(iid)
    want = "mp4" if it["kind"] == "video" else "webp"
    if ext != want or h.store.status(it) != "ready":
        raise ApiError(404, "not_found", "no such asset")
    # mock serves uploads as-is (no transcoding); content-type follows the stored kind anyway
    h.send_file(it["path"], "video/mp4" if want == "mp4" else "image/webp")


@route("GET", r"/([A-Za-z0-9]{8})/thumb\.webp")
def thumb(h, iid):
    it = h.item(iid)
    if it["thumb"]:
        return h.send_file(it["thumb"], "image/webp")
    png = frame_png(it["path"])
    if not png:
        raise ApiError(404, "not_found", "no thumbnail (mock needs ffmpeg)")
    h._send(200, png, "image/png", {"Cache-Control": "public, max-age=31536000, immutable"})


@route("GET", r"/_frame/([A-Za-z0-9]{8})\.png")
def frame(h, iid):
    png = frame_png(h.item(iid)["path"])
    if not png:
        raise ApiError(404, "not_found", "no frame (mock needs ffmpeg)")
    h._send(200, png, "image/png")


@route("GET", r"/([A-Za-z0-9]{8})")
def page(h, iid):
    it = h.store.public(h.item(iid), authed=False)
    esc = lambda s: (s or "").replace("&", "&amp;").replace('"', "&quot;").replace("<", "&lt;")
    title = esc(", ".join(it["tags"][:5]) or it["id"])
    if it["kind"] == "video":
        og = f'<meta property="og:type" content="video.other"><meta property="og:video" content="{it["url"]}"><meta property="og:video:type" content="video/mp4"><meta property="og:image" content="{it["thumb_url"]}">'
        media = f'<video src="{it["url"]}" controls autoplay loop playsinline></video>'
    else:
        og = f'<meta property="og:type" content="website"><meta property="og:image" content="{it["url"]}">'
        media = f'<img src="{it["url"]}" alt="{esc(it["text"])}">'
    html = f"""<!doctype html><html><head><meta charset="utf-8"><title>{title}</title>
<meta property="og:title" content="{title}">{og}<meta name="twitter:card" content="summary_large_image">
<style>body{{margin:0;background:#111;display:grid;place-items:center;min-height:100vh}}img,video{{max-width:100vw;max-height:100vh}}</style>
</head><body>{media}</body></html>"""
    h._send(200, html, "text/html; charset=utf-8")


@route("POST", "/_mock/reset")
def mock_reset(h):
    h.store.reset()
    h._send(204)


def make_server(host="127.0.0.1", port=8787, base_url=None):
    srv = ThreadingHTTPServer((host, port), Handler)
    Handler.store = Store(base_url or f"http://{host}:{srv.server_address[1]}")
    return srv


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8787)
    ap.add_argument("--base-url", help="public URL used in item urls (default http://host:port)")
    ap.add_argument("--web", help="serve static files from this dir for unmatched GETs")
    a = ap.parse_args()
    Handler.web = a.web and os.path.realpath(a.web)
    s = make_server(a.host, a.port, a.base_url)
    print(f"nookle mock on {Handler.store.base_url}  password={PASSWORD!r}  keys: "
          + ", ".join(f"{sc}={k}" for _, sc, k in SEED_KEYS) + ("" if HAVE_FFMPEG else "  (no ffmpeg: no near-dup/probe)"))
    s.serve_forever()
