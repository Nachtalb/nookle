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
  applyRoute();
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

let gen = 0, cursor = null, loading = false, searched = false;
const cards = new Map();

async function search() {
  nav({ q: $('#q').value.trim(), sort: sortValue() });
  searched = true;
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
  const p = new URLSearchParams({ q: $('#q').value.trim(), sort: sortValue(), limit: 40 });
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

const sortValue = () => document.querySelector('[name=sort]:checked').value;

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
  const busy = busyLabel(it);
  if (busy) {
    // Plex-style: dimmed poster, light sweep, spinner + label until the server is done with it.
    c.classList.add('busy');
    if (it.status === 'processing') c.querySelector('img').remove();
    c.append(el('span', { className: 'busy-veil' }, el('span', { className: 'spinner' }), el('span', { textContent: busy })));
    watchBusy(it.id);
  }
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

function busyLabel(it) {
  if (it.status === 'processing') return 'Converting…';
  if (it.status === 'ready' && it.analysis === 'pending') return 'Tagging…';
  return null;
}

// Busy cards are re-fetched every few seconds and swapped for the finished card.
const busyIds = new Set();
let busyTimer = null;
function watchBusy(id) {
  busyIds.add(id);
  busyTimer ??= setInterval(async () => {
    for (const id of [...busyIds]) {
      const c = cards.get(id);
      if (!c?.isConnected) { busyIds.delete(id); continue; }
      const it = await api('/items/' + id).catch(() => null);
      if (!it || busyLabel(it) === busyLabel(c._item)) continue;
      busyIds.delete(id);
      c.replaceWith(card(it));
      if (current?.id === id) showItem(it);
    }
    if (!busyIds.size) { clearInterval(busyTimer); busyTimer = null; }
  }, 4000);
}

let searchTimer;
$('#q').oninput = () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(search, 300);
};
// A handler that returns false cancels the key, which ate Backspace on mobile keyboards.
$('#q').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') { clearTimeout(searchTimer); search(); }
});
$('#sort').onchange = search;

// ---------- item view ----------

let current = null;

function openItem(id) {
  nav({ item: id, view: null }, true);
  applyRoute();
}

async function loadItem(id) {
  if (current?.id === id && $('#item').open) return;
  try {
    const it = await api('/items/' + id);
    if (route().item !== id) return;
    showItem(it);
    if (!$('#item').open) $('#item').showModal();
  } catch (err) {
    toast(err.message);
    closeOverlay();
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
  const tags = tagEditor(it.tags, {
    onClick: (t) => { nav({ q: t, item: null }); applyRoute(); },
    onChange: canWrite() ? (next) => saveTags(it.id, next) : null,
  });
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

async function saveTags(id, tags) {
  try {
    const it = await api('/items/' + id, { method: 'PATCH', json: { tags } });
    cards.get(it.id)?.replaceWith(card(it));
    if (current?.id === it.id) { current = it; return it.tags; }
  } catch (err) {
    toast(err.message);
    if (current?.id === id) showItem(current);
  }
}

// Chips with ✕ to remove and an input to add (Enter or comma), suggesting known tags.
// onChange(tags) may return the saved list to show; without it the editor is read-only.
function tagEditor(initial, { onClick, onChange }) {
  let tags = [...initial];
  const box = el('div', { className: 'chips tag-editor' });
  const input = el('input', { className: 'tag-input', placeholder: 'add tag…', enterKeyHint: 'done', autocomplete: 'off' });
  input.setAttribute('list', 'tag-suggest');
  const set = async (next) => {
    tags = next;
    render();
    const saved = await onChange(next);
    if (saved) { tags = saved; render(); }
  };
  const add = (raw) => {
    const fresh = parseTags(raw).filter((t) => !tags.includes(t));
    if (fresh.length) set([...tags, ...fresh]);
  };
  function render() {
    box.replaceChildren(...tags.map((t) => {
      const chip = el('span', { className: 'chip term' });
      const label = el('button', { type: 'button', className: 'chip-label', textContent: t });
      if (onClick) label.onclick = () => onClick(t);
      chip.append(label);
      if (onChange) {
        const x = el('button', { type: 'button', className: 'chip-x', textContent: '✕', title: 'Remove ' + t });
        x.onclick = () => set(tags.filter((y) => y !== t));
        chip.append(x);
      }
      return chip;
    }), onChange ? input : '');
  }
  if (onChange) {
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); add(input.value); input.value = ''; }
      else if (e.key === 'Backspace' && !input.value && tags.length) set(tags.slice(0, -1));
    });
    input.addEventListener('input', () => {
      if (input.value.includes(',')) { add(input.value); input.value = ''; return; }
      suggest(input.value);
    });
    input.addEventListener('change', () => { if (input.value.trim()) { add(input.value); input.value = ''; } });
  }
  render();
  box.tags = () => tags;
  return box;
}

let suggestTimer;
function suggest(prefix) {
  clearTimeout(suggestTimer);
  if (!prefix.trim()) return;
  suggestTimer = setTimeout(async () => {
    const r = await api('/tags?' + new URLSearchParams({ q: prefix.trim(), limit: 8 })).catch(() => null);
    if (r) $('#tag-suggest').replaceChildren(...r.tags.map((t) => el('option', { value: t.tag })));
  }, 150);
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
  f.text.value = it.text;
  f.style.value = it.style || '';
  f.sources.value = it.sources.map((s) => s.url).join('\n');
  f.hidden = false;
  f.text.focus();
};
$('#edit-cancel').onclick = () => ($('#edit-form').hidden = true);

$('#edit-form').onsubmit = async (e) => {
  e.preventDefault();
  const f = e.target;
  try {
    const it = await api('/items/' + current.id, {
      method: 'PATCH',
      json: { text: f.text.value, style: f.style.value || null, sources: parseLines(f.sources.value) },
    });
    showItem(it);
    cards.get(it.id)?.replaceWith(card(it));
    toast('Saved');
  } catch (err) {
    toast(err.message);
  }
};

$('#item-retag').onclick = async () => {
  try {
    showItem(await api('/items/' + current.id + '/reanalyse', { method: 'POST' }));
    toast('Queued for the tagger');
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
    closeOverlay();
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
  if (!$('#upload').open) openView('add');
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
  const source = $('#up-source').value.trim(), tags = upTags.tags().join(',');
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
  const tags = upTags.tags();
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

const upTags = tagEditor([], { onChange: () => {} });
$('#up-tags').replaceWith(upTags);

$('#open-upload').onclick = () => openView('add');
$('#upload').addEventListener('close', () => {
  if (added) { added = false; search(); }
});

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

$('#open-keys').onclick = () => openView('keys');

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

// ---------- routing ----------
// The URL holds the search (?q=&sort=) and the open overlay (&item=<id> or &view=add|keys),
// so items can be linked and Back closes what was opened.

function route() {
  const p = new URLSearchParams(location.search);
  return { q: p.get('q') || '', sort: p.get('sort') || 'used', item: p.get('item'), view: p.get('view') };
}

function nav(patch, push = false) {
  const r = { ...route(), ...patch };
  const p = new URLSearchParams();
  if (r.q) p.set('q', r.q);
  if (r.sort && r.sort !== 'used') p.set('sort', r.sort);
  if (r.item) p.set('item', r.item);
  else if (r.view) p.set('view', r.view);
  const url = '/' + (p.size ? '?' + p : '');
  if (url === location.pathname + location.search) return;
  // `pushed` marks overlay entries we created, so closing can go Back instead of stacking.
  if (push) history.pushState({ pushed: true }, '', url);
  else history.replaceState(history.state, '', url);
}

function openView(view) {
  nav({ view, item: null }, !route().item && !route().view);
  applyRoute();
}

function closeOverlay() {
  const r = route();
  if (!r.item && !r.view) return;
  if (history.state?.pushed) history.back();
  else { nav({ item: null, view: null }); applyRoute(); }
}

const dialogs = { add: $('#upload'), keys: $('#keys') };

function applyRoute() {
  if ($('#app').hidden) return;
  const r = route();
  if (!searched || $('#q').value.trim() !== r.q || sortValue() !== r.sort) {
    $('#q').value = r.q;
    const radio = document.querySelector(`[name=sort][value="${r.sort}"]`) || document.querySelector('[name=sort][value=used]');
    radio.checked = true;
    search();
  }
  if (r.item) loadItem(r.item);
  else if ($('#item').open) $('#item').close();
  for (const [view, d] of Object.entries(dialogs)) {
    if (!r.item && r.view === view) {
      if (!d.open) {
        if (view === 'keys') { $('#new-key').hidden = true; loadKeys(); }
        d.showModal();
      }
    } else if (d.open) d.close();
  }
}

addEventListener('popstate', applyRoute);

// ---------- boot ----------

for (const b of document.querySelectorAll('dialog .close')) b.onclick = () => b.closest('dialog').close();
for (const d of document.querySelectorAll('dialog')) {
  d.onclick = (e) => {
    const r = d.getBoundingClientRect();
    if (e.target === d && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom)) d.close();
  };
}
$('#item').addEventListener('close', () => { $('#item-media').replaceChildren(); current = null; });
for (const d of [$('#item'), ...Object.values(dialogs)]) d.addEventListener('close', () => {
  const r = route();
  if ((d === $('#item') && r.item) || (d !== $('#item') && r.view && dialogs[r.view] === d)) closeOverlay();
});

if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js');

api('/me').then((me) => showApp(me.scope)).catch(() => showLogin());
