#!/bin/sh
# Appends ?v=<content hash> to the shell's CSS/JS URLs so every deploy gets fresh URLs
# that no browser or Cloudflare cache has seen.
set -eu
cd "${1:-web}"
V=$(cat index.html style.css *.js vendor/*.js | sha256sum | cut -c1-10)
sed -i -E "s#(href|src)=\"/(style\.css|app\.js|vendor/emoji\.min\.js)\"#\1=\"/\2?v=$V\"#g" index.html
sed -i -E "s#from '\./(lib|emoji)\.js'#from './\1.js?v=$V'#g" app.js
echo "assets v=$V"
