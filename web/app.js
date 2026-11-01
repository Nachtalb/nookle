import { chips, parseTags, parseLines, fmtSize, fmtDuration, fmtDate, rankSuggestions, splitEmoji, markSegments } from './lib.js';
import { emojify, emojiNames, emojiWords, colonsToEmoji } from './emoji.js';

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

// Toasts stack: each one stays its own time; the same message again just restarts its timer.
// With onTap the toast is a button (shown longer) that runs onTap and closes.
function toast(msg, { onTap, ms } = {}) {
  const box = $('#toast');
  // An open modal dialog sits above any z-index and makes the rest of the page inert, so the
  // toasts live in the topmost open dialog. Moving elsewhere drops the old ones: they were
  // about whatever that dialog showed.
  const host = [...document.querySelectorAll('dialog[open]')].pop() || document.body;
  if (box.parentNode !== host) {
    box.replaceChildren();
    host.append(box);
  }
  let t = [...box.children].find((c) => c.textContent === msg);
  if (!t) {
    t = onTap
      ? el('button', { type: 'button', className: 'toast action', textContent: msg })
      : el('div', { className: 'toast', textContent: msg });
    if (onTap) t.onclick = () => { t.remove(); onTap(); };
    box.append(t);
    while (box.children.length > 4) box.firstChild.remove();
  }
  clearTimeout(t._timer);
  t._timer = setTimeout(() => t.remove(), ms ?? (onTap ? 5000 : 2500));
}

// After removing a tag by hand: offer to block it, which also sweeps it from every item.
async function offerBlock(tag) {
  const r = await api('/blocked-tags').catch(() => null);
  if (!r || r.tags.includes(tag)) return;
  toast(`🚫 Block “${tag}” everywhere?`, {
    onTap: async () => {
      try {
        await api('/blocked-tags', { method: 'PUT', json: { tags: [...r.tags, tag] } });
        toast(`Blocked “${tag}”, removed from all items`);
        search();
      } catch (err) { toast(err.message); }
    },
  });
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

// Per item id, what matched the current search (tags and text pieces), for highlighting.
const highlights = new Map();

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
  if (!p.has('cursor')) highlights.clear();
  for (const [id, h] of Object.entries(r.highlights || {})) highlights.set(id, h);
  $('#chips').replaceChildren(...chips(r.query).map((c) => emojify(el('span', { className: 'chip ' + c.type }), c.label)));
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
  // after cards.set: watchBusy observes the card it finds there, so the poller sees it on screen
  if (busy) watchBusy(it.id);
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
// Busy cards poll together (only the ones on screen) and pause their animation off screen,
// so a page full of uploads stays smooth.
const busyVisible = new Set();
const busyObserver = new IntersectionObserver((entries) => {
  for (const e of entries) {
    const id = e.target._item?.id;
    e.target.classList.toggle('idle', !e.isIntersecting);
    if (e.isIntersecting) busyVisible.add(id); else busyVisible.delete(id);
  }
}, { rootMargin: '200px' });

function watchBusy(id) {
  busyIds.add(id);
  const c = cards.get(id);
  if (c) { c.classList.add('idle'); busyObserver.observe(c); }
  busyTimer ??= setInterval(async () => {
    for (const id of [...busyIds]) {
      if (!cards.get(id)?.isConnected) { busyIds.delete(id); busyVisible.delete(id); }
    }
    if (!busyIds.size) { clearInterval(busyTimer); busyTimer = null; return; }
    const ids = [...busyIds].filter((id) => busyVisible.has(id) || current?.id === id);
    const fresh = await Promise.all(ids.map((id) => api('/items/' + id).catch(() => null)));
    fresh.forEach((it, i) => {
      const c = cards.get(ids[i]);
      if (!it || !c || busyLabel(it) === busyLabel(c._item)) return;
      busyIds.delete(it.id);
      busyVisible.delete(it.id);
      busyObserver.unobserve(c);
      c.replaceWith(card(it));
      if (current?.id === it.id) showItem(it);
    });
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

// Desktop (mouse and keyboard): opening an item puts the cursor in its tag field, so tagging
// runs as type, Esc, arrow to the next, type. Not on touch, where it would pop up the keyboard.
const desktop = matchMedia('(hover: hover) and (pointer: fine)');

function showItem(it) {
  updateSteps(it.id);
  const opened = current?.id !== it.id || !$('#item').open;
  current = it;
  if (opened && desktop.matches) queueMicrotask(() => $('#item-info .tag-input')?.focus());
  setEditing(false);
  showReviewed(it);
  $('#item-title').textContent = it.id;
  const m = it.status !== 'ready' ? el('p', { className: 'muted', textContent: it.status === 'failed' ? 'Failed: ' + it.error : 'Processing…' })
    : it.mime === 'video/mp4' ? el('video', { src: it.url, controls: true, loop: true, playsInline: true, autoplay: true })
      : el('img', { src: it.url, alt: it.text });
  $('#item-media').replaceChildren(m);
  const row = (k, v, cls = '') => v || v === 0 ? [el('dt', { className: cls, textContent: k }), el('dd', { className: cls }, v)] : [];
  const hl = (route().q && highlights.get(it.id)) || { tags: [], text: [] };
  const tags = tagEditor(it.tags, {
    hits: hl.tags,
    onClick: (t) => { nav({ q: t, item: null }, true); applyRoute(); },
    onChange: canWrite() ? (next) => saveTags(it.id, next) : null,
    onRemove: canWrite() ? offerBlock : null,
  });
  const sources = it.sources.length ? el('div', {}, ...it.sources.map((s) =>
    el('div', {}, el('a', { href: s.url, target: '_blank', rel: 'noopener', textContent: s.author ? `${s.site}: @${s.author}` : s.url })))) : null;
  $('#item-info').replaceChildren(
    tags,
    it.text ? markedText(it.text, hl.text) : '',
    el('dl', {},
      ...row('Kind', [it.kind, it.width && `${it.width}×${it.height}`, fmtDuration(it.duration_ms), it.has_audio ? 'sound' : '', fmtSize(it.size)].filter(Boolean).join(' · ')),
      ...row('Style', it.style || '—', 'ro'),
      ...row('Sources', sources, 'ro'),
      ...row('Hits', String(it.hits)),
      ...row('Added', fmtDate(it.added_at)),
      ...row('Last used', fmtDate(it.last_used_at)),
      ...row('Reviewed', fmtDate(it.reviewed_at)),
      ...row('Status', `${it.status} · analysis ${it.analysis}`)));
}

// The item text with the pieces that matched the search wrapped in <mark>.
function markedText(text, pieces) {
  const p = el('p', { className: 'text', title: canWrite() ? 'Double-click to edit' : '' });
  if (canWrite()) p.ondblclick = () => { nav({ edit: '1' }, true); applyRoute(); };
  for (const s of markSegments(text, pieces)) p.append(s.hit ? emojify(el('mark'), s.text) : emojify(el('span'), s.text));
  return p;
}

// Set when an item is edited from search results: on closing it the search runs again, so items
// that no longer match (a removed tag was the hit) drop out.
let staleResults = false;

async function saveTags(id, tags) {
  try {
    const it = await api('/items/' + id, { method: 'PATCH', json: { tags } });
    staleResults = true;
    cards.get(it.id)?.replaceWith(card(it));
    if (current?.id === it.id) { current = it; showReviewed(it); return it.tags; }
  } catch (err) {
    toast(err.message);
    if (current?.id === id) showItem(current);
  }
}

// Chips with ✕ to remove and an input to add (Enter, comma or the ➤ button inside the field,
// which saves switching to the letter keyboard after typing emoji). An autocomplete list offers
// known tags and emoji by name; picking an entry adds it at once.
// onChange(tags) may return the saved list to show; without it the editor is read-only.
function tagEditor(initial, { onClick, onChange, onRemove, hits = [] }) {
  let tags = [...initial];
  const box = el('div', { className: 'chips tag-editor' });
  const input = el('input', { className: 'tag-input', placeholder: 'add tag…', enterKeyHint: 'done', autocomplete: 'off' });
  const submit = el('button', { type: 'button', className: 'tag-submit', title: 'Add', textContent: '➤' });
  const list = el('ul', { className: 'suggest', hidden: true });
  const field = el('span', { className: 'tag-add' }, input, submit, list);
  let options = [], active = -1;
  const set = async (next) => {
    tags = next;
    render();
    const saved = await onChange(next);
    if (saved) { tags = saved; render(); }
  };
  const add = (raw) => {
    const fresh = parseTags(raw).filter((t) => !tags.includes(t));
    input.value = '';
    close();
    if (fresh.length) set([...tags, ...fresh]);
    input.focus();
  };
  const close = () => { list.hidden = true; options = []; active = -1; };
  const show = (opts) => {
    options = opts;
    active = -1;
    list.replaceChildren(...opts.map((o, i) => {
      const li = el('li', {}, emojify(el('span', { className: 'label' }), o.label), el('span', { className: 'hint', textContent: o.hint }));
      // pointerdown keeps the focus (and the phone keyboard) in the input
      li.addEventListener('pointerdown', (e) => e.preventDefault());
      li.onclick = () => add(o.value);
      li.onmouseenter = () => mark(i);
      return li;
    }));
    list.hidden = !opts.length;
  };
  const mark = (i) => {
    active = i;
    [...list.children].forEach((li, j) => li.classList.toggle('active', j === i));
  };
  let timer, seq = 0;
  const lookup = () => {
    clearTimeout(timer);
    const q = input.value;
    if (!q.trim()) return close();
    const mine = ++seq;
    timer = setTimeout(async () => {
      const r = await api('/tags?' + new URLSearchParams({ q: q.trim().toLowerCase(), limit: 8 })).catch(() => ({ tags: [] }));
      if (mine !== seq) return;
      const words = await emojiWords(api);
      if (mine !== seq) return;
      show(rankSuggestions(q, r.tags.filter((t) => !tags.includes(t.tag)), [...emojiNames, ...words]));
    }, 120);
  };
  const remove = (t) => { set(tags.filter((y) => y !== t)); onRemove?.(t); };
  const labels = () => [...box.querySelectorAll('.chip-label')];
  // Keyboard: Left from an empty field selects the last chip; Left/Right walk the chips,
  // Backspace/Delete remove the selected one, Esc or typing goes back to the field.
  const pick = (i) => {
    const ls = labels();
    if (i >= ls.length || i < 0 && !ls.length) return input.focus();
    ls[Math.max(0, i)].focus();
  };
  box.addEventListener('keydown', (e) => {
    const label = e.target.closest?.('.chip-label');
    if (!label || !onChange) return;
    const i = labels().indexOf(label), t = tags[i];
    if (e.key === 'ArrowLeft') pick(i - 1);
    else if (e.key === 'ArrowRight') pick(i + 1);
    else if (e.key === 'Backspace' || e.key === 'Delete') {
      remove(t);
      pick(e.key === 'Backspace' ? i - 1 : i);
    } else if (e.key === 'Escape') input.focus();
    else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) return input.focus();
    else return;
    // keep the item view's arrow keys (prev/next item) and Esc (close) out of it
    e.preventDefault();
    e.stopPropagation();
  });
  // Only the chips are rebuilt; the input stays in place, since re-inserting it would blur it
  // (and close the phone keyboard) every time a tag is added.
  function render() {
    // a selected chip stays selected (by position) when the saved list comes back
    const sel = labels().indexOf(document.activeElement);
    for (const c of [...box.children]) if (c !== field) c.remove();
    const chipEls = tags.map((t) => {
      const chip = el('span', { className: 'chip term' + (hits.includes(t) ? ' hit' : '') });
      const label = emojify(el('button', { type: 'button', className: 'chip-label', title: t }), t);
      if (onClick) label.onclick = () => onClick(t);
      chip.append(label);
      if (onChange) {
        const x = el('button', { type: 'button', className: 'chip-x', textContent: '✕', title: 'Remove ' + t });
        x.onclick = () => remove(t);
        chip.append(x);
      }
      return chip;
    });
    if (onChange) {
      if (!field.isConnected || field.parentNode !== box) box.append(field);
      field.before(...chipEls);
    } else box.append(...chipEls);
    if (sel >= 0) pick(Math.min(sel, tags.length - 1));
  }
  if (onChange) {
    submit.addEventListener('pointerdown', (e) => e.preventDefault());
    submit.onclick = () => (active >= 0 ? add(options[active].value) : add(input.value));
    input.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown' && options.length) { e.preventDefault(); mark((active + 1) % options.length); }
      else if (e.key === 'ArrowUp' && options.length) { e.preventDefault(); mark((active - 1 + options.length) % options.length); }
      else if (e.key === 'Escape' && !list.hidden) { e.preventDefault(); e.stopPropagation(); close(); }
      // Esc leaves the field first (arrow keys then step through items); a second Esc closes
      else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); input.blur(); }
      else if (e.key === 'Enter') { e.preventDefault(); active >= 0 ? add(options[active].value) : add(input.value); }
      else if (e.key === 'Backspace' && !input.value && tags.length) set(tags.slice(0, -1));
      else if (e.key === 'ArrowLeft' && !input.value && tags.length) { e.preventDefault(); e.stopPropagation(); pick(tags.length - 1); }
    });
    input.addEventListener('input', () => {
      if (input.value.includes(',')) return add(input.value);
      // An emoji is always a tag of its own: add it the moment it is typed, `:us:` codes too.
      const { emoji, rest } = splitEmoji(colonsToEmoji(input.value));
      if (emoji.length) {
        add(emoji.join(','));
        input.value = rest;
      }
      lookup();
    });
    input.addEventListener('blur', () => setTimeout(close, 150));
  }
  render();
  box.tags = () => tags;
  return box;
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

function used(it) {
  api(`/items/${it.id}/hit`, { method: 'POST' }).then(() => { it.hits++; it.last_used_at = new Date().toISOString(); if (current?.id === it.id) showItem(it); }).catch(() => {});
}

// The share sheet needs a recent tap; a slow download can outlive it, so the file is kept
// and a second tap shares it at once.
const shareFiles = new Map();
$('#item-share-file').onclick = async () => {
  const it = current;
  try {
    let file = shareFiles.get(it.id);
    if (!file) {
      toast('Preparing…');
      const blob = await (await fetch(it.url)).blob();
      file = new File([blob], it.url.split('/').pop(), { type: it.mime });
      shareFiles.set(it.id, file);
    }
    if (!navigator.canShare?.({ files: [file] })) {
      const a = el('a', { href: URL.createObjectURL(file), download: file.name });
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 10000);
      toast('Sharing files is not supported here, downloaded instead');
    } else {
      await navigator.share({ files: [file] });
    }
    used(it);
  } catch (err) {
    if (err.name === 'NotAllowedError') toast('Ready, tap again to share');
    else if (err.name !== 'AbortError') toast(err.message);
  }
};

$('#item-share-link').onclick = async () => {
  const it = current;
  try {
    if (navigator.share) await navigator.share({ url: it.url });
    else { await copy(it.url); toast('Link copied'); }
    used(it);
  } catch (err) {
    if (err.name !== 'AbortError') toast(err.message);
  }
};

// Edit mode is its own history entry (`&edit=1`), so Back leaves edit mode instead of the item.
// Editing happens in place: the form takes the spot of the text, and the read-only text, style
// and sources hide meanwhile. Outside edit mode the form waits at the end of the dialog, so
// rebuilding the item info never throws it away.
function setEditing(on) {
  const f = $('#edit-form'), info = $('#item-info');
  f.hidden = !on;
  info.classList.toggle('editing', on);
  $('#item-edit').classList.toggle('active', on);
  $('#item-edit').setAttribute('aria-pressed', String(on));
  if (on) (info.querySelector('.tag-editor') || info.firstChild)?.after(f);
  else if (f.parentNode !== $('#item')) $('#item').append(f);
}
function openEdit() {
  const f = $('#edit-form'), it = current;
  if (!it || !f.hidden) return;
  f.text.value = it.text;
  f.style.value = it.style || '';
  f.sources.value = it.sources.map((s) => s.url).join('\n');
  setEditing(true);
  fitText();
  f.text.focus();
  f.text.setSelectionRange(f.text.value.length, f.text.value.length);
}
// The text box grows with its content, so edit mode looks like the text it replaces.
function fitText() {
  const t = $('#edit-form').text;
  t.style.height = 'auto';
  t.style.height = t.scrollHeight + 2 + 'px';
}
$('#edit-form').text.addEventListener('input', fitText);
function leaveEdit() {
  if (route().edit && history.state?.pushed) history.back();
  else { nav({ edit: null }); setEditing(false); }
}
// The pencil toggles: a second tap leaves edit mode without saving.
$('#item-edit').onclick = () => {
  if (!$('#edit-form').hidden) return leaveEdit();
  nav({ edit: '1' }, true);
  applyRoute();
};
$('#edit-cancel').onclick = leaveEdit;

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
    staleResults = true;
    toast('Saved');
    leaveEdit();
  } catch (err) {
    toast(err.message);
  }
};

// ---------- stepping through the current results ----------
// Prev/next follow the grid order; stepping replaces the history entry, so Back still closes.

const gridIds = () => [...$('#grid').children].map((c) => c._item?.id).filter(Boolean);

function updateSteps(id) {
  const ids = gridIds(), i = ids.indexOf(id);
  $('#item-prev').hidden = i <= 0;
  $('#item-next').hidden = i < 0 || (i === ids.length - 1 && !cursor);
}

async function step(dir) {
  if (!current) return;
  let ids = gridIds(), i = ids.indexOf(current.id);
  if (i < 0) return;
  if (dir > 0 && i === ids.length - 1 && cursor) {
    await loadMore();
    ids = gridIds();
  }
  const next = ids[i + dir];
  if (!next) return;
  cards.get(next)?.scrollIntoView({ block: 'nearest' });
  nav({ item: next, edit: null });
  applyRoute();
}

$('#item-prev').onclick = () => step(-1);
$('#item-next').onclick = () => step(1);
document.addEventListener('keydown', (e) => {
  if (!$('#item').open || e.target.closest?.('input, textarea, select')) return;
  if (e.key === 'ArrowLeft') step(-1);
  else if (e.key === 'ArrowRight') step(1);
});
let touch = null;
$('.media-wrap').addEventListener('touchstart', (e) => { touch = e.touches[0]; }, { passive: true });
$('.media-wrap').addEventListener('touchend', (e) => {
  const t = e.changedTouches[0];
  if (!touch) return;
  const dx = t.clientX - touch.clientX, dy = t.clientY - touch.clientY;
  touch = null;
  if (Math.abs(dx) > 60 && Math.abs(dx) > 2 * Math.abs(dy)) step(dx < 0 ? 1 : -1);
});

// Reviewed mark: set by any hand edit, or toggled here; "Reviewed" sort lists marked items first.
function showReviewed(it) {
  const b = $('#item-reviewed'), on = !!it.reviewed_at;
  b.classList.toggle('active', on);
  b.setAttribute('aria-pressed', String(on));
  b.title = b.ariaLabel = on ? 'Reviewed — tap to unmark' : 'Mark as reviewed';
}
$('#item-reviewed').onclick = async () => {
  const it = current;
  try {
    const next = await api('/items/' + it.id, { method: 'PATCH', json: { reviewed: !it.reviewed_at } });
    staleResults = true;
    cards.get(next.id)?.replaceWith(card(next));
    if (current?.id === next.id) { current = next; showReviewed(next); }
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
    // Fixed two-line rows (thumb | name over state | actions) so they keep their height as the
    // state changes.
    const thumb = el('span', { className: 'q-thumb' });
    if (f.type.startsWith('image/')) thumb.append(el('img', { src: URL.createObjectURL(f), alt: '' }));
    else thumb.textContent = f.type.startsWith('video/') ? '▶' : '…';
    const li = el('li', { className: 'q-row' }, thumb,
      el('span', { className: 'q-main' }, el('span', { className: 'name', textContent: f.name || 'pasted' }), el('span', { className: 'state muted', textContent: 'queued' })),
      el('span', { className: 'q-extra' }));
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

// Uploads run in the background, two at a time: closing the dialog doesn't stop them, and
// conversion and tagging start on the server as each file lands. Finished uploads show up in
// the gallery as busy cards right away.
$('#up-go').onclick = async () => {
  const jobs = pending;
  pending = [];
  $('#up-go').disabled = true;
  const source = $('#up-source').value.trim(), tags = upTags.tags().join(',');
  const next = async () => {
    for (let j; (j = jobs.shift());) await upload(j, source, tags, false);
  };
  const total = jobs.length;
  await Promise.all([next(), next()]);
  if (!$('#upload').open) toast(`Uploaded ${total} file${total === 1 ? '' : 's'}`);
};

// New uploads go to the top of the gallery: at once while it's showing, or when the dialog closes.
const landed = [];
function showUploaded(it) {
  if ($('#upload').open) return landed.push(it);
  if (cards.has(it.id) || route().q) return;
  $('#grid').prepend(card(it));
  $('#grid-status').textContent = '';
}

async function upload(j, source, tags, force) {
  const fd = new FormData();
  fd.append('file', j.file, j.file.name || 'pasted');
  if (source) fd.append('source', source);
  if (tags) fd.append('tags', tags);
  if (force) fd.append('force', '1');
  setState(j.li, 'uploading…');
  try {
    const it = await api('/items', { method: 'POST', body: fd });
    showUploaded(it);
    track(j.li, it);
  } catch (err) {
    if (err.status === 409) showDup(j.li, err.body, () => upload(j, source, tags, true));
    else setState(j.li, err.message, 'error');
  }
}

function setState(li, text, cls = 'muted', ...extra) {
  if (!li.classList.contains('q-row')) {
    li.querySelector('.state').replaceWith(el('span', { className: 'state ' + cls, textContent: text }, ...extra));
    return;
  }
  // item links sit inline on the state line; real actions (Add anyway) go on the right
  const links = extra.filter((x) => x.classList?.contains('mini'));
  li.querySelector('.state').replaceWith(el('span', { className: 'state ' + cls, title: text.trim() }, text, ...links));
  li.querySelector('.q-extra').replaceChildren(...extra.filter((x) => !links.includes(x)));
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
  else {
    setState(li, 'ready ', 'ok', itemLink(it));
    const th = li.querySelector('.q-thumb');
    if (th) th.replaceChildren(el('img', { src: it.thumb_url, alt: '' }));
  }
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
  added = false;
  for (const it of landed.splice(0).reverse()) showUploaded(it);
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

async function loadBlocked() {
  try {
    const r = await api('/blocked-tags');
    $('#blocked').replaceChildren(tagEditor(r.tags, {
      onChange: async (tags) => {
        try {
          const saved = (await api('/blocked-tags', { method: 'PUT', json: { tags } })).tags;
          search();
          return saved;
        } catch (err) { toast(err.message); }
      },
    }));
  } catch (err) {
    toast(err.message);
  }
}

// ---------- routing ----------
// The URL holds the search (?q=&sort=) and the open overlay (&item=<id> or &view=add|keys),
// so items can be linked and Back closes what was opened.

function route() {
  const p = new URLSearchParams(location.search);
  return { q: p.get('q') || '', sort: p.get('sort') || 'used', item: p.get('item'), view: p.get('view'), edit: p.get('edit') };
}

function nav(patch, push = false) {
  const r = { ...route(), ...patch };
  const p = new URLSearchParams();
  if (r.q) p.set('q', r.q);
  if (r.sort && r.sort !== 'used') p.set('sort', r.sort);
  if (r.item) {
    p.set('item', r.item);
    if (r.edit) p.set('edit', r.edit);
  } else if (r.view) p.set('view', r.view);
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
  if (r.item) loadItem(r.item).then(() => (r.edit ? openEdit() : setEditing(false)));
  else if ($('#item').open) $('#item').close();
  for (const [view, d] of Object.entries(dialogs)) {
    if (!r.item && r.view === view) {
      if (!d.open) {
        if (view === 'keys') { $('#new-key').hidden = true; loadKeys(); loadBlocked(); }
        d.showModal();
      }
    } else if (d.open) d.close();
  }
}

addEventListener('popstate', applyRoute);
for (const d of document.querySelectorAll('dialog')) d.addEventListener('close', () => { if ($('#toast').parentNode === d) $('#toast').replaceChildren(); });

// ---------- boot ----------

for (const b of document.querySelectorAll('dialog .close')) b.onclick = () => b.closest('dialog').close();
for (const d of document.querySelectorAll('dialog')) {
  d.onclick = (e) => {
    const r = d.getBoundingClientRect();
    if (e.target === d && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom)) d.close();
  };
}
$('#item').addEventListener('close', () => {
  $('#item-media').replaceChildren();
  current = null;
  if (staleResults) {
    staleResults = false;
    // after the close handler below has dropped ?item= from the URL
    if (route().q) queueMicrotask(search);
  }
});
for (const d of [$('#item'), ...Object.values(dialogs)]) d.addEventListener('close', () => {
  const r = route();
  if ((d === $('#item') && r.item) || (d !== $('#item') && r.view && dialogs[r.view] === d)) closeOverlay();
});

if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js');

api('/me').then((me) => showApp(me.scope)).catch(() => showLogin());
