#!/usr/bin/env bash
# Builds the whole trailer into a folder: ./make.sh out-dir
# Renders every frame at 60fps, synthesizes the soundtrack, then writes the
# master (trailer.mp4) and a smaller copy for the web (trailer-web.mp4), which
# has to stay under 15 MB to go on the player page.
# Needs the web dependencies installed, ffmpeg, Chromium and python3 with scipy.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p "${1:?usage: ./make.sh out-dir}"
out="$(realpath "$1")"
../node_modules/.bin/vite build --logLevel error
node render.mjs "$out/video.mp4" 60 4
python3 soundtrack.py "$out/music.wav"
ffmpeg -v error -y -i "$out/video.mp4" -i "$out/music.wav" -map 0:v -map 1:a -c:v copy -c:a aac -b:a 256k -ar 48000 -movflags +faststart -shortest "$out/trailer.mp4"
ffmpeg -v error -y -i "$out/video.mp4" -i "$out/music.wav" -map 0:v -map 1:a -c:v libx264 -preset veryslow -crf 22 -tune animation -pix_fmt yuv420p -profile:v high -level 4.2 \
  -color_primaries bt709 -color_trc bt709 -colorspace bt709 -c:a aac -b:a 160k -ar 48000 -movflags +faststart -shortest "$out/trailer-web.mp4"
