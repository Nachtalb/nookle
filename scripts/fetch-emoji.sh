#!/bin/sh
# Fetches the Apple emoji images (emoji-datasource-apple, the data version emoji-js 3.9 uses)
# into web/emoji/apple/64. Too big for git; the web image runs this at build time.
set -eu
V=16.0.0
DEST=${1:-web}/emoji/apple
mkdir -p "$DEST"
curl -fsSL "https://registry.npmjs.org/emoji-datasource-apple/-/emoji-datasource-apple-$V.tgz" |
  tar xz -C "$DEST" --strip-components=3 package/img/apple/64
curl -fsSL "https://registry.npmjs.org/emoji-datasource-apple/-/emoji-datasource-apple-$V.tgz" |
  tar xzO package/LICENSE > "$DEST/LICENSE"
