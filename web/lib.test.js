import test from 'node:test';
import assert from 'node:assert/strict';
import { chips, parseTags, parseLines, fmtSize, fmtDuration } from './lib.js';

test('chips', () => {
  const q = {
    kinds: ['video'], styles: ['anime'], added_after: '2024-03-01T00:00:00Z', added_before: '2024-04-01T00:00:00Z',
    terms: ['cat'], phrases: ['may'], emoji: [{ emoji: '😂', keywords: ['lol', 'laugh'] }],
  };
  assert.deepEqual(chips(q).map((c) => c.label), ['video', 'anime', '2024-03-01 – 2024-04-01', '😂 lol, laugh', '“may”', 'cat']);
  assert.deepEqual(chips({ added_after: '2026-10-02T00:00:00Z' }).map((c) => c.label), ['after 2026-10-02']);
  assert.deepEqual(chips({ added_before: '2026-10-02T00:00:00Z' }).map((c) => c.label), ['before 2026-10-02']);
  assert.deepEqual(chips(null), []);
});

test('parsers and formatters', () => {
  assert.deepEqual(parseTags(' Cat, dog,,cat '), ['cat', 'dog']);
  assert.deepEqual(parseLines('a\n\n b \n'), ['a', 'b']);
  assert.equal(fmtSize(500), '500 B');
  assert.equal(fmtSize(11700), '11.4 KB');
  assert.equal(fmtDuration(65000), '1:05');
  assert.equal(fmtDuration(null), '');
});
