#!/usr/bin/env bash
# Regenerates every sample. inputs/ = what clients upload, stored/ = what the backend keeps.
set -euo pipefail
export SVT_LOG=1
cd "$(dirname "$0")"
rm -rf inputs stored && mkdir -p inputs stored
FONT=$(fc-match -f '%{file}' 'DejaVu Sans:bold' 2>/dev/null || fc-match -f '%{file}' 'sans:bold')
ff() { ffmpeg -hide_banner -loglevel error -y "$@"; }
caption() { echo "drawtext=fontfile=$FONT:text='$1':fontsize=$2:fontcolor=white:borderw=3:bordercolor=black:x=(w-tw)/2:y=$3"; }

# --- inputs ---
ff -f lavfi -i "gradients=s=640x480:c0=0x3050a0:c1=0xe07030:seed=7" -frames:v 1 \
   -vf "$(caption 'WHEN THE BUILD' 48 40),$(caption 'IS FINALLY GREEN' 48 400)" inputs/build_green.png
ff -i inputs/build_green.png -vf scale=480:-1 -q:v 6 inputs/build_green_near_dup.jpg
ff -f lavfi -i "testsrc2=s=640x480" -frames:v 1 -vf "$(caption 'just a photo' 36 420)" -q:v 4 inputs/photo.jpg
ff -f lavfi -i "color=c=black@0.0:s=320x320,format=rgba" -frames:v 1 \
   -vf "drawbox=x=60:y=60:w=200:h=200:color=yellow@1:t=fill:replace=1,$(caption 'OK' 64 120)" inputs/transparent.png
ff -f lavfi -i "testsrc=s=320x240:r=12:d=2" -vf "$(caption 'yes yes yes' 28 200),split[a][b];[a]palettegen[p];[b][p]paletteuse" -loop 0 inputs/dance.gif
ff -f lavfi -i "mandelbrot=s=360x200:r=15" -t 4 -vf "$(caption 'silent loop' 32 220)" -an \
   -c:v libx264 -pix_fmt yuv420p -movflags +faststart inputs/silent_short.mp4
ff -f lavfi -i "testsrc=s=320x180:r=10" -t 25 -an \
   -c:v libx264 -pix_fmt yuv420p -movflags +faststart inputs/silent_long.mp4
ff -f lavfi -i "testsrc2=s=480x270:r=24:d=6" -f lavfi -i "sine=f=440:d=6" \
   -vf "$(caption 'this one has sound' 28 220)" -c:v libx264 -pix_fmt yuv420p -c:a aac -b:a 64k \
   -movflags +faststart -shortest inputs/with_sound.mp4
# only referenced by the fxtwitter fixtures, never seeded
ff -f lavfi -i "smptebars=s=480x360" -frames:v 1 -vf "$(caption 'imported from x' 36 300)" -q:v 4 inputs/x_photo.jpg
ff -f lavfi -i "rgbtestsrc=s=320x240:r=15:d=3" -f lavfi -i "sine=f=880:d=3" -c:v libx264 -pix_fmt yuv420p \
   -c:a aac -b:a 48k -movflags +faststart -shortest inputs/x_video.mp4

# --- stored (backend normalisation output) ---
webp()      { ff -i "$1" -c:v libwebp -quality 80 "$2"; }
anim_webp() { ff -i "$1" -an -c:v libwebp_anim -quality 75 -loop 0 "$2"; }
av1()       { ff -i "$1" -c:v libsvtav1 -crf 38 -preset 8 -pix_fmt yuv420p "${@:3}" -movflags +faststart "$2"; }
thumb()     { ff -i "$1" -frames:v 1 -vf "scale='min(320,iw)':-2" -c:v libwebp -quality 70 "$2"; }

webp inputs/build_green.png stored/bUiLdGrn.webp
webp inputs/photo.jpg stored/pH0t0aaa.webp
webp inputs/transparent.png stored/tRaNsPrt.webp
anim_webp inputs/dance.gif stored/dAnCe123.webp
anim_webp inputs/silent_short.mp4 stored/sIlEnt05.webp
av1 inputs/silent_long.mp4 stored/sIlEnt25.mp4 -an
av1 inputs/with_sound.mp4 stored/sOuNd006.mp4 -c:a libopus -b:a 64k
for f in stored/*.webp stored/*.mp4; do id=$(basename "${f%.*}"); thumb "$f" "stored/$id.thumb.webp"; done
ls -la inputs stored
