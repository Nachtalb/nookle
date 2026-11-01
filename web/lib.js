export function chips(q) {
  if (!q) return [];
  const out = [];
  for (const k of q.kinds || []) out.push({ type: 'kind', label: k });
  for (const s of q.styles || []) out.push({ type: 'style', label: s });
  const a = q.added_after && day(q.added_after), b = q.added_before && day(q.added_before);
  if (a && b) out.push({ type: 'date', label: `${a} – ${b}` });
  else if (a) out.push({ type: 'date', label: `after ${a}` });
  else if (b) out.push({ type: 'date', label: `before ${b}` });
  for (const e of q.emoji || []) out.push({ type: 'emoji', label: `${e.emoji} ${e.keywords.slice(0, 6).join(', ')}${e.keywords.length > 6 ? '…' : ''}` });
  for (const p of q.phrases || []) out.push({ type: 'phrase', label: `“${p}”` });
  for (const t of q.terms || []) out.push({ type: 'term', label: t });
  return out;
}

const day = (iso) => iso.slice(0, 10);

export function parseTags(s) {
  return [...new Set(s.split(',').map((t) => t.trim().toLowerCase()).filter(Boolean))];
}

export function parseLines(s) {
  return s.split('\n').map((l) => l.trim()).filter(Boolean);
}

export function fmtSize(n) {
  if (n == null) return '';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${i ? n.toFixed(1) : n} ${u[i]}`;
}

export function fmtDuration(ms) {
  if (!ms) return '';
  const s = Math.round(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

export function fmtDate(iso) {
  return iso ? new Date(iso).toLocaleString() : 'never';
}

/**
 * Autocomplete entries for a tag being typed: up to limit-3 known tags (by use), then emoji whose short
 * name starts with, then contains, the text (`:smi` or `smi` -> 😏 smirk). `value` is what gets added.
 */
export function rankSuggestions(input, tags, emoji, limit = 8) {
  const q = input.trim().toLowerCase().replace(/^:/, '').replace(/:$/, '');
  if (!q) return [];
  const out = tags.slice(0, Math.max(1, limit - 3)).map((t) => ({ value: t.tag, label: t.tag, hint: String(t.count) }));
  const seen = new Set(out.map((o) => o.value));
  const starts = [], within = [];
  for (const e of emoji) {
    const words = e.name.replace(/_/g, ' ');
    if (e.name.startsWith(q) || words.startsWith(q)) starts.push(e);
    else if (e.name.includes(q)) within.push(e);
  }
  for (const e of [...starts.sort((a, b) => a.name.length - b.name.length), ...within]) {
    if (out.length >= limit) break;
    if (seen.has(e.char)) continue;
    seen.add(e.char);
    out.push({ value: e.char, label: e.char, hint: ':' + e.name + ':' });
  }
  return out.slice(0, limit);
}

const segmenter = new Intl.Segmenter('en', { granularity: 'grapheme' });
const EMOJI = /\p{Extended_Pictographic}|\p{Regional_Indicator}/u;

/** Pulls whole emoji (ZWJ sequences, skin tones, flags) out of typed text; `rest` keeps the words. */
export function splitEmoji(text) {
  const emoji = [];
  let rest = '';
  for (const { segment } of segmenter.segment(text)) {
    if (EMOJI.test(segment)) emoji.push(segment);
    else rest += segment;
  }
  return { emoji, rest: emoji.length ? rest.replace(/\s+/g, ' ').trimStart() : rest };
}

/** Splits `text` into [{text, hit}] runs, `hit` where one of `pieces` occurs (case-insensitive). */
export function markSegments(text, pieces = []) {
  const lower = text.toLowerCase();
  const hit = new Array(text.length).fill(false);
  for (const p of pieces) {
    if (!p) continue;
    for (let i = lower.indexOf(p.toLowerCase()); i >= 0; i = lower.indexOf(p.toLowerCase(), i + 1)) {
      hit.fill(true, i, i + p.length);
    }
  }
  const out = [];
  for (let i = 0; i < text.length; i++) {
    const last = out[out.length - 1];
    if (last && last.hit === hit[i]) last.text += text[i];
    else out.push({ text: text[i], hit: hit[i] });
  }
  return out;
}
