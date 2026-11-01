import { chips, parseTags, parseLines, fmtSize, fmtDuration, fmtDate } from './lib.js';

const $ = (s) => document.querySelector(s);
const el = (tag, props = {}, ...kids) => {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...kids.filter((k) => k != null));
  return e;
};

class ApiError extends Error {
  constructor(status, body) {
    super(body.message || `HTTP ${status}`);
    this.status = status;
    this.body = body;
  }
}

async function api(path, opts = {}) {
  if (opts.json) {
    opts.body = JSON.stringify(opts.json);
    opts.headers = { 'Content-Type': 'application/json' };
  }
  const r = await fetch('/api' + path, { credentials: 'same-origin', ...opts });
  if (r.status === 204) return null;
  const body = await r.json().catch(() => ({}));
  if (r.status === 401 && path !== '/login') showLogin();
  if (!r.ok) throw new ApiError(r.status, body);
  return body;
}

let toastTimer;
function toast(msg) {
  const t = $('#toast');
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 2500);
}

// ---------- auth ----------

function showLogin() {
  $('#app').hidden = true;
  $('#login').hidden = false;
  document.querySelectorAll('dialog[open]').forEach((d) => d.close());
}

function showApp(scope) {
  document.body.dataset.scope = scope;
  $('#scope').textContent = scope;
  $('#login').hidden = true;
  $('#app').hidden = false;
  search();
}

$('#login-form').onsubmit = async (e) => {
  e.preventDefault();
  const f = e.target;
  try {
    const { scope } = await api('/login', { method: 'POST', json: { password: f.password.value } });
    f.reset();
    $('#login-error').textContent = '';
    toast(`Logged in as ${scope}`);
    showApp(scope);
  } catch (err) {
    $('#login-error').textContent = err.message;
  }
};

$('#logout').onclick = async () => {
  await api('/logout', { method: 'POST' }).catch(() => {});
  showLogin();
};

const canWrite = () => ['rw', 'admin'].includes(document.body.dataset.scope);

// ---------- search grid ----------

let gen = 0, cursor = null, loading = false;
const cards = new Map();

async function search() {
  gen++;
  cursor = undefined;
  cards.clear();
  $('#grid').replaceChildren();
  await loadMore();
}

async function loadMore() {
  if (loading || cursor === null) return;
  loading = true;
  const my = gen;
  const p = new URLSearchParams({ q: $('#q').value.trim(), sort: document.querySelector('[name=sort]:checked').value, limit: 40 });
  if (cursor) p.set('cursor', cursor);
  $('#grid-status').textContent = 'Loading…';
  let r, error;
  try { r = await api('/items?' + p); } catch (err) { error = err; }
  loading = false;
  if (my !== gen) return loadMore();
  if (error) return ($('#grid-status').textContent = error.message);
  cursor = r.next_cursor;
  $('#chips').replaceChildren(...chips(r.query).map((c) => el('span', { className: 'chip ' + c.type, textContent: c.label })));
  for (const it of r.items) $('#grid').append(card(it));
  $('#grid-status').textContent = cards.size ? '' : 'Nothing found';
  if (cursor && isVisible($('#sentinel'))) loadMore();
}

const isVisible = (e) => e.getBoundingClientRect().top < innerHeight + 600;

new IntersectionObserver((es) => es[0].isIntersecting && loadMore(), { rootMargin: '600px' }).observe($('#sentinel'));

const noHover = matchMedia('(hover: none)').matches;
const animObserver = new IntersectionObserver((es) => {
  for (const e of es) animate(e.target, e.isIntersecting);
}, { threshold: 0.6 });

function animate(c, on) {
  const it = c._item, img = c.querySelector('img');
  if (it.kind === 'gif') img.src = on ? it.url : it.thumb_url;
  if (it.kind === 'video') {
    c.querySelector('video')?.remove();
    if (on) c.prepend(el('video', { src: it.url, muted: true, autoplay: true, loop: true, playsInline: true }));
  }
}

function card(it) {
  const c = el('button', { className: 'card', title: it.tags.join(', ') },
    el('img', { src: it.thumb_url, alt: it.text || it.tags.join(', '), loading: 'lazy' }),
    it.kind !== 'image' ? el('span', { className: 'kind', textContent: it.kind === 'gif' ? 'GIF' : '▶ ' + fmtDuration(it.duration_ms) }) : null);
  c._item = it;
  if (it.width && it.height) c.style.aspectRatio = `${it.width} / ${it.height}`;
  c.onclick = () => openItem(it.id);
  if (it.kind !== 'image') {
    if (noHover) animObserver.observe(c);
    else {
      c.onmouseenter = () => animate(c, true);
      c.onmouseleave = () => animate(c, false);
    }
  }
  cards.set(it.id, c);
  return c;
}

let searchTimer;
$('#q').oninput = () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(search, 300);
};
$('#q').onkeydown = (e) => e.key === 'Enter' && (clearTimeout(searchTimer), search());
$('#sort').onchange = search;

// ---------- item view ----------

let current = null;

async function openItem(id) {
  try {
    showItem(await api('/items/' + id));
    if (!$('#item').open) $('#item').showModal();
  } catch (err) {
    toast(err.message);
  }
}

function showItem(it) {
  current = it;
  $('#edit-form').hidden = true;
  $('#item-title').textContent = it.id;
  const m = it.status !== 'ready' ? el('p', { className: 'muted', textContent: it.status === 'failed' ? 'Failed: ' + it.error : 'Processing…' })
    : it.mime === 'video/mp4' ? el('video', { src: it.url, controls: true, loop: true, playsInline: true, autoplay: true })
      : el('img', { src: it.url, alt: it.text });
  $('#item-media').replaceChildren(m);
  $('#item-page').href = it.page_url;
  const row = (k, v) => v || v === 0 ? [el('dt', { textContent: k }), el('dd', {}, v)] : [];
  const tags = el('div', { className: 'chips' }, ...it.tags.map((t) => {
    const b = el('button', { className: 'chip term', textContent: t, type: 'button' });
    b.onclick = () => { $('#q').value = t; $('#item').close(); search(); };
    return b;
  }));
  const sources = it.sources.length ? el('div', {}, ...it.sources.map((s) =>
    el('div', {}, el('a', { href: s.url, target: '_blank', rel: 'noopener', textContent: s.author ? `${s.site}: @${s.author}` : s.url })))) : null;
  $('#item-info').replaceChildren(
    tags,
    it.text ? el('p', { className: 'text', textContent: it.text }) : '',
    el('dl', {},
      ...row('Kind', [it.kind, it.width && `${it.width}×${it.height}`, fmtDuration(it.duration_ms), it.has_audio ? 'sound' : '', fmtSize(it.size)].filter(Boolean).join(' · ')),
      ...row('Style', it.style || '—'),
      ...row('Sources', sources),
      ...row('Hits', String(it.hits)),
      ...row('Added', fmtDate(it.added_at)),
      ...row('Last used', fmtDate(it.last_used_at)),
      ...row('Status', `${it.status} · analysis ${it.analysis}`)));
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const t = el('textarea', { value: text });
    (document.querySelector('dialog[open]') || document.body).append(t);
    t.select();
    document.execCommand('copy');
    t.remove();
  }
}

$('#item-copy').onclick = async () => {
  const it = current;
  await copy(it.url);
  toast('Link copied');
  api(`/items/${it.id}/hit`, { method: 'POST' }).then(() => { it.hits++; it.last_used_at = new Date().toISOString(); showItem(it); }).catch(() => {});
};

$('#item-edit').onclick = () => {
  const f = $('#edit-form'), it = current;
  f.tags.value = it.tags.join(', ');
  f.text.value = it.text;
  f.style.value = it.style || '';
  f.sources.value = it.sources.map((s) => s.url).join('\n');
  f.hidden = false;
  f.tags.focus();
};
$('#edit-cancel').onclick = () => ($('#edit-form').hidden = true);

$('#edit-form').onsubmit = async (e) => {
  e.preventDefault();
  const f = e.target;
  try {
    const it = await api('/items/' + current.id, {
      method: 'PATCH',
      json: { tags: parseTags(f.tags.value), text: f.text.value, style: f.style.value || null, sources: parseLines(f.sources.value) },
    });
    showItem(it);
    cards.get(it.id)?.replaceWith(card(it));
    toast('Saved');
  } catch (err) {
    toast(err.message);
  }
};

$('#item-delete').onclick = async () => {
  if (!confirm('Delete this item for good?')) return;
  try {
    await api('/items/' + current.id, { method: 'DELETE' });
    cards.get(current.id)?.remove();
    cards.delete(current.id);
    $('#item').close();
    toast('Deleted');
  } catch (err) {
    toast(err.message);
  }
};

// ---------- upload & import ----------

let pending = [], added = false;

function addFiles(files) {
  if (!canWrite()) return;
  for (const f of files) {
    const li = el('li', {}, el('span', { className: 'name', textContent: f.name || 'pasted' }), el('span', { className: 'state muted', textContent: 'queued' }));
    pending.push({ file: f, li });
    $('#up-queue').append(li);
  }
  $('#up-go').disabled = !pending.length;
  if (!$('#upload').open) $('#upload').showModal();
}

$('#files').onchange = (e) => { addFiles(e.target.files); e.target.value = ''; };
const drop = $('#drop');
drop.ondragover = (e) => { e.preventDefault(); drop.classList.add('over'); };
drop.ondragleave = () => drop.classList.remove('over');
drop.ondrop = (e) => { e.preventDefault(); drop.classList.remove('over'); addFiles(e.dataTransfer.files); };
document.addEventListener('dragover', (e) => canWrite() && e.preventDefault());
document.addEventListener('drop', (e) => { if (canWrite() && !drop.contains(e.target)) { e.preventDefault(); addFiles(e.dataTransfer.files); } });
document.addEventListener('paste', (e) => {
  const files = [...e.clipboardData.files];
  if (files.length) { e.preventDefault(); addFiles(files); }
});

$('#up-go').onclick = async () => {
  const jobs = pending;
  pending = [];
  $('#up-go').disabled = true;
  const source = $('#up-source').value.trim(), tags = $('#up-tags').value.trim();
  for (const j of jobs) await upload(j, source, tags, false);
};

async function upload(j, source, tags, force) {
  const fd = new FormData();
  fd.append('file', j.file, j.file.name || 'pasted');
  if (source) fd.append('source', source);
  if (tags) fd.append('tags', tags);
  if (force) fd.append('force', '1');
  setState(j.li, 'uploading…');
  try {
    track(j.li, await api('/items', { method: 'POST', body: fd }));
  } catch (err) {
    if (err.status === 409) showDup(j.li, err.body, () => upload(j, source, tags, true));
    else setState(j.li, err.message, 'error');
  }
}

function setState(li, text, cls = 'muted', ...extra) {
  li.querySelector('.state').replaceWith(el('span', { className: 'state ' + cls, textContent: text }, ...extra));
}

const itemLink = (it) => {
  const b = el('button', { className: 'mini', type: 'button' }, el('img', { src: it.thumb_url, alt: '' }), it.id);
  b.onclick = () => openItem(it.id);
  return b;
};

async function track(li, it) {
  added = true;
  while (it.status === 'processing') {
    setState(li, 'processing…');
    await new Promise((r) => setTimeout(r, 1000));
    try { it = await api('/items/' + it.id); } catch (err) { return setState(li, err.message, 'error'); }
  }
  if (it.status === 'failed') setState(li, 'failed: ' + it.error, 'error');
  else setState(li, 'ready ', 'ok', itemLink(it));
}

function showDup(li, d, force) {
  const extra = [itemLink(d.existing)];
  if (d.match === 'near') {
    const b = el('button', { type: 'button', textContent: 'Add anyway' });
    b.onclick = force;
    extra.push(b);
  }
  setState(li, `${d.match} duplicate${d.match === 'near' ? ` (distance ${d.distance})` : ''} of `, 'warn', ...extra);
}

$('#import-form').onsubmit = async (e) => {
  e.preventDefault();
  await runImport(e.target.url.value.trim(), undefined, false);
  e.target.reset();
};

async function runImport(url, media, force) {
  const tags = parseTags($('#up-tags').value);
  const head = el('li', {}, el('span', { className: 'name', textContent: url + (media ? ` #${media[0]}` : '') }), el('span', { className: 'state muted', textContent: 'importing…' }));
  $('#import-queue').append(head);
  try {
    const { results } = await api('/items/import', { method: 'POST', json: { url, media, tags, force } });
    head.remove();
    for (const r of results) {
      const li = el('li', {}, el('span', { className: 'name', textContent: `${url} #${r.index}` }), el('span', { className: 'state' }));
      $('#import-queue').append(li);
      if (r.status === 'created') track(li, r.item);
      else showDup(li, r, () => { li.remove(); runImport(url, [r.index], true); });
    }
  } catch (err) {
    setState(head, err.message, 'error');
  }
}

$('#open-upload').onclick = () => $('#upload').showModal();
$('#upload').onclose = () => {
  if (added) { added = false; search(); }
};

// ---------- keys ----------

async function loadKeys() {
  try {
    const { keys } = await api('/keys');
    $('#key-list').replaceChildren(...keys.map((k) => {
      const del = el('button', { className: 'danger', textContent: 'Delete' });
      del.onclick = async () => {
        if (!confirm(`Delete key "${k.name}"? Clients using it stop working.`)) return;
        await api('/keys/' + k.id, { method: 'DELETE' }).catch((err) => toast(err.message));
        loadKeys();
      };
      return el('li', {},
        el('div', {}, el('strong', { textContent: k.name }), ' ', el('span', { className: 'badge', textContent: k.scope }), ' ', el('code', { textContent: k.prefix + '…' }),
          el('div', { className: 'muted small', textContent: `created ${fmtDate(k.created_at)} · last used ${fmtDate(k.last_used_at)}` })),
        del);
    }));
  } catch (err) {
    toast(err.message);
  }
}

$('#open-keys').onclick = () => {
  $('#new-key').hidden = true;
  $('#keys').showModal();
  loadKeys();
};

$('#key-form').onsubmit = async (e) => {
  e.preventDefault();
  const f = e.target;
  try {
    const k = await api('/keys', { method: 'POST', json: { name: f.name.value.trim(), scope: f.scope.value } });
    $('#new-key-value').value = k.key;
    $('#new-key').hidden = false;
    f.reset();
    loadKeys();
  } catch (err) {
    toast(err.message);
  }
};

$('#new-key-copy').onclick = async () => {
  await copy($('#new-key-value').value);
  toast('Key copied');
};

// ---------- boot ----------

for (const b of document.querySelectorAll('dialog .close')) b.onclick = () => b.closest('dialog').close();
for (const d of document.querySelectorAll('dialog')) {
  d.onclick = (e) => {
    const r = d.getBoundingClientRect();
    if (e.target === d && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom)) d.close();
  };
}
$('#item').addEventListener('close', () => $('#item-media').replaceChildren());


api('/me').then((me) => showApp(me.scope)).catch(() => showLogin());
