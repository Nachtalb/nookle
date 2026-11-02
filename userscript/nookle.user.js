// ==UserScript==
// @name         Nookle: add x.com posts
// @namespace    https://n.naa.gg
// @version      0.2.0
// @description  Adds an "Add to Nookle" button to x.com posts with media
// @homepageURL  https://github.com/Nachtalb/nookle/tree/main/userscript
// @supportURL   https://github.com/Nachtalb/nookle/issues
// @updateURL    https://raw.githubusercontent.com/Nachtalb/nookle/main/userscript/nookle.user.js
// @downloadURL  https://raw.githubusercontent.com/Nachtalb/nookle/main/userscript/nookle.user.js
// @match        https://x.com/*
// @match        https://twitter.com/*
// @match        https://mobile.x.com/*
// @match        https://mobile.twitter.com/*
// @grant        GM_xmlhttpRequest
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_registerMenuCommand
// @connect      n.naa.gg
// @connect      localhost
// @connect      127.0.0.1
// @run-at       document-idle
// ==/UserScript==

const POST_RE = /^(?:https?:\/\/(?:www\.|mobile\.)?(?:x|twitter)\.com)?\/(\w+)\/status\/(\d+)/;

// hrefs of an <article> in document order -> canonical post URL or null.
// The reply link (in_reply_to=<id>) belongs to the article's own action bar,
// so it wins over status links that may come from a quoted post.
function statusUrl(hrefs) {
  const posts = hrefs.map((h) => POST_RE.exec(h)).filter(Boolean);
  const reply = hrefs.map((h) => /[?&]in_reply_to=(\d+)/.exec(h)).find(Boolean);
  const id = reply ? reply[1] : posts[0]?.[2];
  if (!id) return null;
  const user = posts.find((m) => m[2] === id && m[1] !== "i")?.[1] ?? "i";
  return `https://x.com/${user}/status/${id}`;
}

// Import response -> toast lines [{text, links: [{label, href}]}]
function summarize(body) {
  const results = body?.results ?? [];
  const created = results.filter((r) => r.status === "created");
  const dupes = results.filter((r) => r.status === "duplicate");
  const links = (rs, key) => rs.map((r) => ({ label: `#${r.index}`, href: r[key]?.page_url })).filter((l) => l.href);
  const lines = [];
  if (created.length) lines.push({ text: `Added ${created.length}`, links: links(created, "item") });
  if (dupes.length) lines.push({ text: `${dupes.length} already in Nookle`, links: links(dupes, "existing") });
  return lines.length ? lines : [{ text: "No media found in this post", links: [] }];
}

// HTTP status + response text -> readable error
function errorText(status, text, base) {
  let msg = "";
  try {
    msg = JSON.parse(text).message || "";
  } catch {}
  const why = msg ? `: ${msg}` : "";
  if (!status) return `Can't reach ${base}`;
  if (status === 401) return "Bad API key (401). Set it in the userscript menu.";
  if (status === 403) return "This key can't import, it needs rw scope (403)";
  if (status === 422) return `Nookle can't import this URL${why}`;
  if (status === 502) return `Nookle couldn't download the post${why}`;
  return `Nookle error ${status}${why}`;
}

if (typeof module === "object") {
  module.exports = { statusUrl, summarize, errorText };
} else {
  main();
}

function main() {
  const ICON_ADD = "M19 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2zm0 16H5V5h14v14zM11 7h2v4h4v2h-4v4h-2v-4H7v-2h4V7z";
  const ICON_DONE = "M9 16.2 4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4L9 16.2z";
  const MEDIA = 'img[src*="pbs.twimg.com/media/"], img[src*="video_thumb/"], video';
  const done = new Set(); // post URLs added this session
  const busy = new Set();

  const base = () => GM_getValue("base", "https://n.naa.gg").replace(/\/+$/, "");
  const askKey = () => {
    const k = prompt("Nookle rw API key (nk_...)", GM_getValue("key", ""));
    if (k !== null) GM_setValue("key", k.trim());
    return k?.trim() || "";
  };
  GM_registerMenuCommand("Set Nookle API key", askKey);
  GM_registerMenuCommand("Set Nookle API URL", () => {
    const u = prompt("Nookle API URL", base());
    if (u !== null) GM_setValue("base", u.trim() || "https://n.naa.gg");
  });

  const style = document.createElement("style");
  style.textContent = `
.nookle-btn{display:inline-flex;align-items:center;justify-content:center;height:34px;min-width:34px;padding:0 8px;border:0;border-radius:9999px;background:none;color:var(--nk,inherit);cursor:pointer}
.nookle-btn:hover{color:#1d9bf0;background:rgba(29,155,240,.1)}
.nookle-btn svg{width:18.75px;height:18.75px;fill:currentColor}
.nookle-btn.done{color:#00ba7c}
.nookle-btn.busy{opacity:.4;cursor:progress}
#nookle-toast{position:fixed;left:50%;bottom:80px;transform:translateX(-50%);z-index:2147483647;max-width:90vw;padding:10px 14px;border-radius:8px;background:#1d9bf0;color:#fff;font:15px/1.4 system-ui,sans-serif;box-shadow:0 2px 12px rgba(0,0,0,.3)}
#nookle-toast.err{background:#f4212e}
#nookle-toast a{color:#fff;text-decoration:underline;margin-left:6px}`;
  document.head.append(style);

  let toastTimer;
  function toast(lines, err) {
    let el = document.getElementById("nookle-toast");
    if (!el) {
      el = document.createElement("div");
      el.id = "nookle-toast";
      document.body.append(el);
    }
    el.className = err ? "err" : "";
    el.replaceChildren(
      ...lines.map(({ text, links = [] }) => {
        const div = document.createElement("div");
        div.textContent = text;
        for (const l of links) {
          const a = document.createElement("a");
          a.href = l.href;
          a.target = "_blank";
          a.rel = "noopener";
          a.textContent = l.label;
          div.append(a);
        }
        return div;
      }),
    );
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.remove(), err ? 10000 : 6000);
  }

  function paint(btn) {
    const url = btn.dataset.url;
    btn.classList.toggle("done", done.has(url));
    btn.classList.toggle("busy", busy.has(url));
    btn.title = done.has(url) ? "In Nookle" : "Add to Nookle";
    btn.setAttribute("aria-label", btn.title);
    btn.firstChild.firstChild.setAttribute("d", done.has(url) ? ICON_DONE : ICON_ADD);
  }

  function add(btn) {
    const url = btn.dataset.url;
    if (busy.has(url)) return;
    const key = GM_getValue("key", "") || askKey();
    if (!key) return;
    busy.add(url);
    document.querySelectorAll(".nookle-btn").forEach((b) => b.dataset.url === url && paint(b));
    toast([{ text: "Adding to Nookle…" }]);
    const finish = (status, text) => {
      busy.delete(url);
      if (status === 200) {
        done.add(url);
        let body;
        try {
          body = JSON.parse(text);
        } catch {}
        toast(summarize(body));
      } else {
        toast([{ text: errorText(status, text, base()) }], true);
      }
      document.querySelectorAll(".nookle-btn").forEach((b) => b.dataset.url === url && paint(b));
    };
    GM_xmlhttpRequest({
      method: "POST",
      url: base() + "/api/items/import",
      headers: { Authorization: "Bearer " + key, "Content-Type": "application/json" },
      data: JSON.stringify({ url }),
      timeout: 300000,
      onload: (r) => finish(r.status, r.responseText),
      onerror: () => finish(0, ""),
      ontimeout: () => finish(0, ""),
    });
  }

  function makeButton() {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "nookle-btn";
    btn.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path/></svg>';
    for (const ev of ["pointerdown", "pointerup", "mousedown", "mouseup", "touchstart", "touchend"])
      btn.addEventListener(ev, (e) => e.stopPropagation());
    btn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      add(btn);
    });
    return btn;
  }

  // Quoted posts are a role=link card inside the article (the post itself may sit in one too).
  const ownMedia = (art) =>
    [...art.querySelectorAll(MEDIA)].some((m) => {
      const card = m.closest('[role="link"]:not(a)');
      return m.closest("article") === art && !(card && art.contains(card));
    });

  function scan() {
    for (const art of document.querySelectorAll("article")) {
      const groups = [...art.querySelectorAll('[role="group"]')].filter(
        (g) => g.closest("article") === art && g.querySelector("[data-engagement-action], [data-testid=like], [data-testid=unlike], [data-testid=reply]"),
      );
      const group = groups.at(-1);
      let btn = art.querySelector(".nookle-btn");
      if (!group || !ownMedia(art)) {
        btn?.remove();
        continue;
      }
      const url = statusUrl([...art.querySelectorAll("a[href]")].map((a) => a.getAttribute("href")));
      if (!url) continue;
      if (!btn) {
        btn = makeButton();
        btn.style.setProperty("--nk", getComputedStyle(group.querySelector("a, button") || group).color);
        group.insertBefore(btn, group.lastElementChild);
      }
      if (btn.dataset.url !== url || !btn.title) {
        btn.dataset.url = url;
        paint(btn);
      }
    }
  }

  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      scan();
    });
  }).observe(document.body, { childList: true, subtree: true });
  setInterval(scan, 800);
  scan();
}
