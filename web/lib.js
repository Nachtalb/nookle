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
