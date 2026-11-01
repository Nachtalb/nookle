"""Self-check for the mock (and the contract in docs/API.md). Run: python3 -m unittest mock/test_mock.py"""
import json
import os
import sys
import threading
import time
import unittest
import urllib.error
import urllib.request
import uuid

sys.path.insert(0, os.path.dirname(__file__))
os.environ.setdefault("MOCK_QUIET", "1")
os.environ["MOCK_PROCESS_DELAY"] = "0.2"
import server  # noqa: E402

RO, RW = server.SEED_KEYS[0][2], server.SEED_KEYS[1][2]
INPUTS = os.path.join(server.SAMPLES, "inputs")


class MockTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = server.make_server(port=0)
        cls.base = server.Handler.store.base_url
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()

    def setUp(self):
        self.req("POST", "/_mock/reset")

    def req(self, method, path, body=None, key=RW, headers=None, raw=False):
        h = dict(headers or {})
        if key:
            h["Authorization"] = "Bearer " + key
        data = body
        if isinstance(body, dict):
            data = json.dumps(body).encode()
            h["Content-Type"] = "application/json"
        r = urllib.request.Request(self.base + path, data=data, method=method, headers=h)
        try:
            with urllib.request.urlopen(r) as resp:
                out = resp.read()
                return resp.status, (out if raw else json.loads(out) if out else None), resp.headers
        except urllib.error.HTTPError as e:
            out = e.read()
            return e.code, (json.loads(out) if out else None), e.headers

    def upload(self, name, key=RW, **fields):
        b = uuid.uuid4().hex
        parts = [f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode() for k, v in fields.items()]
        with open(os.path.join(INPUTS, name), "rb") as f:
            data = f.read()
        parts.append(f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n'
                     f'Content-Type: application/octet-stream\r\n\r\n'.encode() + data + b"\r\n")
        body = b"".join(parts) + f"--{b}--\r\n".encode()
        return self.req("POST", "/api/items", body, key, {"Content-Type": f"multipart/form-data; boundary={b}"})

    def ids(self, q="", **params):
        params["q"] = q
        qs = urllib.parse.urlencode(params)
        st, body, _ = self.req("GET", "/api/items?" + qs, key=RO)
        self.assertEqual(st, 200, body)
        return [i["id"] for i in body["items"]]

    def test_auth(self):
        self.assertEqual(self.req("GET", "/api/items", key=None)[0], 401)
        self.assertEqual(self.req("GET", "/api/keys", key=RW)[0], 403)
        self.assertEqual(self.req("DELETE", "/api/items/bUiLdGrn", key=RO)[0], 403)
        st, body, hdr = self.req("POST", "/api/login", {"password": RO}, key=None)
        self.assertEqual((st, body["scope"]), (200, "ro"))
        cookie = hdr["Set-Cookie"].split(";")[0]
        self.assertEqual(self.req("GET", "/api/me", key=None, headers={"Cookie": cookie})[1]["scope"], "ro")
        st, body, hdr = self.req("POST", "/api/login", {"password": server.PASSWORD}, key=None)
        admin = {"Cookie": hdr["Set-Cookie"].split(";")[0]}
        st, k, _ = self.req("POST", "/api/keys", {"name": "irc", "scope": "ro"}, key=None, headers=admin)
        self.assertEqual(st, 201)
        self.assertEqual(self.req("GET", "/api/items", key=k["key"])[0], 200)
        self.assertEqual(self.req("DELETE", f"/api/keys/{k['id']}", key=None, headers=admin)[0], 204)
        self.assertEqual(self.req("GET", "/api/items", key=k["key"])[0], 401)

    def test_sorts(self):
        self.assertEqual(self.ids()[:2], ["bUiLdGrn", "dAnCe123"])
        self.assertEqual(self.ids(sort="added")[0], "sIlEnt25")
        self.assertEqual(self.ids(sort="recent")[0], "dAnCe123")
        self.req("POST", "/api/items/pH0t0aaa/hit", key=RO)
        self.assertEqual(self.ids(sort="recent")[0], "pH0t0aaa")

    def test_paging(self):
        st, body, _ = self.req("GET", "/api/items?limit=3", key=RO)
        seen = [i["id"] for i in body["items"]]
        while body["next_cursor"]:
            body = self.req("GET", "/api/items?limit=3&cursor=" + body["next_cursor"], key=RO)[1]
            seen += [i["id"] for i in body["items"]]
        self.assertEqual(len(seen), 7)
        self.assertEqual(len(set(seen)), 7)

    def test_query_language(self):
        self.assertEqual(set(self.ids("video")), {"sIlEnt25", "sOuNd006"})
        self.assertEqual(set(self.ids("gif")), {"dAnCe123", "sIlEnt05"})
        self.assertEqual(set(self.ids("new")), {"bUiLdGrn", "dAnCe123", "sIlEnt25"})
        self.assertEqual(set(self.ids("new video")), {"sIlEnt25"})
        self.assertEqual(set(self.ids("old gif")), {"sIlEnt05"})
        self.assertEqual(set(self.ids("2024")), {"sOuNd006"})
        self.assertEqual(set(self.ids("2025-03")), {"tRaNsPrt"})
        self.assertEqual(set(self.ids("march 2025")), {"tRaNsPrt"})
        self.assertEqual(set(self.ids("december 2024 video")), {"sOuNd006"})
        self.assertEqual(set(self.ids("anime")), {"dAnCe123"})
        self.assertEqual(set(self.ids("photograph")), {"pH0t0aaa"})
        self.assertEqual(set(self.ids("prog")), {"bUiLdGrn"})
        self.assertEqual(set(self.ids("finally green")), {"bUiLdGrn"})
        self.assertEqual(set(self.ids('"new"')), set())
        self.assertEqual(set(self.ids("nachtalb")), {"bUiLdGrn"})
        self.assertEqual(set(self.ids("👍")), {"tRaNsPrt", "dAnCe123"})  # ok, yes
        self.assertEqual(set(self.ids("😂")), {"dAnCe123"})  # happy
        self.assertEqual(set(self.ids("🦖")), set())
        self.assertEqual(set(self.ids("🥳")), {"dAnCe123"})  # celebration
        body = self.req("GET", "/api/items?q=" + urllib.parse.quote("new video 👍"), key=RO)[1]
        self.assertEqual(body["query"]["kinds"], ["video"])
        self.assertTrue(body["query"]["added_after"])
        self.assertIn("ok", body["query"]["emoji"][0]["keywords"])

    def test_upload_and_dups(self):
        st, it, _ = self.upload("x_photo.jpg", source="https://x.com/someone/status/42", tags="Bars, tv")
        self.assertEqual(st, 201, it)
        self.assertEqual((it["status"], it["kind"], it["tags"]), ("processing", "image", ["bars", "tv"]))
        self.assertEqual(it["sources"][0], {"url": "https://x.com/someone/status/42", "site": "x", "author": "someone"})
        time.sleep(0.3)
        it = self.req("GET", f"/api/items/{it['id']}")[1]
        self.assertEqual(it["status"], "ready")
        self.assertEqual(it["telegram"]["type"], "photo")
        self.assertEqual(self.req("GET", f"/{it['id']}.webp", key=None, raw=True)[0], 200)

        st, err, _ = self.upload("build_green.png")
        self.assertEqual((st, err["match"], err["existing"]["id"]), (409, "exact", "bUiLdGrn"))
        st, err, _ = self.upload("build_green_near_dup.jpg")
        self.assertEqual((st, err["match"], err["existing"]["id"]), (409, "near", "bUiLdGrn"))
        self.assertEqual(self.upload("build_green_near_dup.jpg", force="1")[0], 201)
        self.assertEqual(self.upload("build_green.png", force="1")[0], 409)

    def test_kind_detection(self):
        self.assertEqual(self.upload("x_video.mp4")[1]["kind"], "video")  # has audio

    def test_import(self):
        st, body, _ = self.req("POST", "/api/items/import", {"url": "https://x.com/nachtalb/status/1000000000000000001"})
        self.assertEqual(st, 200, body)
        r = body["results"]
        self.assertEqual([x["status"] for x in r], ["created", "created", "duplicate"])
        self.assertEqual(r[2]["existing"]["id"], "dAnCe123")
        self.assertEqual(r[0]["item"]["sources"][0]["author"], "nachtalb")
        st, body, _ = self.req("POST", "/api/items/import", {"url": "https://fxtwitter.com/nachtalb/status/1000000000000000001", "media": [1]})
        self.assertEqual(body["results"][0]["status"], "duplicate")
        self.assertEqual(self.req("POST", "/api/items/import", {"url": "https://example.com/a"})[0], 422)

    def test_analysis_flow(self):
        st, c, _ = self.req("POST", "/api/analysis/claim")
        self.assertEqual((st, c["item"]["id"]), (200, "sIlEnt25"))
        st, frame, hdr = self.req("GET", c["frame_url"].removeprefix(self.base), key=None, raw=True)
        self.assertEqual((st, frame[:4]), (200, b"\x89PNG"))
        self.assertEqual(self.req("POST", "/api/analysis/claim")[0], 204)  # leased
        st, it, _ = self.req("POST", "/api/items/sIlEnt25/analysis", {"tags": ["Pattern"], "text": "", "style": "other"})
        self.assertEqual((it["analysis"], it["tags"], it["style"]), ("done", ["pattern"], "other"))

    def test_public(self):
        st, data, hdr = self.req("GET", "/sOuNd006.mp4", key=None, raw=True, headers={"Range": "bytes=0-99"})
        self.assertEqual((st, len(data), hdr["Content-Range"].split("/")[0]), (206, 100, "bytes 0-99"))
        self.assertEqual(self.req("GET", "/sOuNd006.webp", key=None, raw=True)[0], 404)
        st, html, _ = self.req("GET", "/sOuNd006", key=None, raw=True)
        self.assertIn(b'og:video" content="' + self.base.encode() + b'/sOuNd006.mp4', html)
        self.assertEqual(self.req("GET", "/bUiLdGrn/thumb.webp", key=None, raw=True)[0], 200)
        self.assertNotIn("telegram", json.loads(json.dumps(server.Handler.store.public(server.Handler.store.items["bUiLdGrn"], authed=False))))
        st, _, hdr = self.req("OPTIONS", "/api/items", key=None, raw=True)
        self.assertEqual((st, hdr["Access-Control-Allow-Origin"]), (204, "*"))


import urllib.parse  # noqa: E402

if __name__ == "__main__":
    unittest.main()
