// Apple emoji everywhere (vendor/emoji.min.js = iamcal's emoji-js, images from
// emoji-datasource-apple 16, the data version emoji-js 3.9 is built on), so tags look the same
// on every device. Text inputs can't show images and keep the system font.
/* global EmojiConvertor */

const img = new EmojiConvertor();
img.img_set = 'apple';
img.img_sets.apple.path = '/emoji/apple/64/';
img.use_sheet = false;
img.allow_native = false;
img.include_title = true;
img.init_env();
img.replace_mode = 'img';

const uni = new EmojiConvertor();
uni.init_env();
uni.allow_native = true;
uni.replace_mode = 'unified';
uni.init_colons();

const esc = (s) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

/** Sets `node`'s content to `text` with every emoji drawn as an Apple image. */
export function emojify(node, text) {
  node.innerHTML = img.replace_unified(esc(text));
  return node;
}

/** Short names (`smirk`, `joy`, …) with their emoji, for the tag autocomplete. */
export const emojiNames = Object.keys(uni.map.colons)
  .filter((n) => !n.includes('::'))
  .map((name) => ({ name, char: uni.replace_colons(`:${name}:`) }))
  .filter((e) => e.char && !e.char.startsWith(':'));

/** Replaces `:short_name:` codes (`:us:`, `:+1:`) with their emoji; unknown codes stay. */
export function colonsToEmoji(text) {
  return text.includes(':') ? uni.replace_colons(text) : text;
}
