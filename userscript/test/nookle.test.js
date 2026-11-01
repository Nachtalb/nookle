const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const path = require("node:path");
const { statusUrl, summarize, errorText } = require("../nookle.user.js");

const hrefs = (file) =>
  [...fs.readFileSync(path.join(__dirname, file), "utf8").matchAll(/<a [^>]*?href="([^"]+)"/g)].map((m) => m[1].replaceAll("&amp;", "&"));

test("statusUrl from saved x.com articles", () => {
  assert.equal(statusUrl(hrefs("timeline-article.html")), "https://x.com/NASA/status/2116263878807998722");
  assert.equal(statusUrl(hrefs("detail-article.html")), "https://x.com/NASA/status/2116263878807998722");
  assert.equal(statusUrl(hrefs("reply-article.html")), "https://x.com/Grazi/status/2116264206127231118");
});

test("statusUrl prefers the article's own post over a quoted one", () => {
  const quoteFirst = ["/q/status/111/photo/1", "/me/status/222", "/compose/post?in_reply_to=222"];
  assert.equal(statusUrl(quoteFirst), "https://x.com/me/status/222");
  assert.equal(statusUrl(["https://twitter.com/me/status/9"]), "https://x.com/me/status/9");
  assert.equal(statusUrl(["/compose/post?in_reply_to=7"]), "https://x.com/i/status/7");
  assert.equal(statusUrl(["/NASA", "https://t.co/x"]), null);
});

test("summarize the mock's import response", () => {
  const body = JSON.parse(fs.readFileSync(path.join(__dirname, "import-response.json"), "utf8"));
  const lines = summarize(body);
  assert.deepEqual(
    lines.map((l) => l.text),
    ["Added 2", "1 already in Nookle"],
  );
  assert.deepEqual(lines[1].links, [{ label: "#3", href: body.results[2].existing.page_url }]);
  assert.equal(lines[0].links.length, 2);
  assert.equal(summarize({ results: [] })[0].text, "No media found in this post");
});

test("errorText", () => {
  const e = (msg) => JSON.stringify({ error: "x", message: msg });
  assert.match(errorText(401, e("login"), "b"), /Bad API key/);
  assert.equal(errorText(422, e("nope"), "b"), "Nookle can't import this URL: nope");
  assert.equal(errorText(502, e("fx down"), "b"), "Nookle couldn't download the post: fx down");
  assert.equal(errorText(0, "", "https://n.naa.gg"), "Can't reach https://n.naa.gg");
  assert.equal(errorText(500, "<html>", "b"), "Nookle error 500");
});
