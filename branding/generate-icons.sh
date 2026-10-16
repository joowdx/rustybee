#!/usr/bin/env bash
# Regenerates every rustybee icon. The colour icons come from branding/bee.svg,
# the sticker bee traced from bee-source.jpg; the monochrome themed and
# notification icons come from branding/bee.png (cut by branding/extract-bee.py).
# Needs: python3 with numpy, scipy and pillow; ImageMagick (magick); rsvg-convert; cwebp.
#   python3 -m venv .venv && .venv/bin/pip install numpy scipy pillow
#   PYTHON=.venv/bin/python branding/generate-icons.sh
set -euo pipefail

cd "$(dirname "$0")/.."
PY=${PYTHON:-python3}
B=branding
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

compose() { "$PY" $B/compose.py "$@"; }
webp() { cwebp -quiet -lossless "$1" -o "$2"; }

# masters
compose $B/icon-master.png
"$PY" $B/masks.py $B/icon-master.png $B/icon
# standalone bee on transparent, filling the canvas (favicon, anywhere the bee
# sits on its own)
compose $B/icon-bee.png --mode fg --width 0.96

# server icon (container registries, dashboards): the bee on a honeycomb cell
# filled with the icon background, with nothing touching the bee
compose "$T/bee.png" --mode fg --width 0.5 --size 1024
compose "$T/hexbg.png" --mode bg --size 1024
hex=$("$PY" -c "import math;print(' '.join(f'{512+430*math.cos(math.radians(a)):.0f},{512+430*math.sin(math.radians(a)):.0f}' for a in range(-90,270,60)))")
magick "$T/hexbg.png" \( -size 1024x1024 xc:black -fill white -draw "polygon $hex" \) \
  -alpha off -compose copy-opacity -composite -compose over -fill none \
  -stroke white -strokewidth 110 -draw "stroke-linejoin round polygon $hex" \
  -stroke '#4a2a14' -strokewidth 66 -draw "stroke-linejoin round polygon $hex" \
  \( "$T/bee.png" \) -gravity center -geometry -8+8 -composite \
  -trim +repage -background none -extent 1024x1024 $B/icon-server.png

# web ui: embedded by the server binary, so both stay small enough for the
# 40 KB asset budget (AC-010-05). transparent bee for the tab and the page
# logo, the full square for iOS, which applies its own mask
S=server/static
magick $B/icon-bee.png -define icon:auto-resize=32,16 $S/favicon.ico
magick $B/icon-master.png -resize 180x180 -strip -colors 128 PNG8:$S/apple-touch-icon.png
# header and login logo (40px at 2x)
magick $B/icon-bee.png -resize 80x80 -strip -colors 128 PNG8:$S/bee.png

# android adaptive layers (108dp canvas, 72dp visible, 66dp safe zone). minSdk 36
# always uses the adaptive icon, so there are no legacy square or round mipmaps
compose "$T/fg.png" --mode fg --width 0.44
compose "$T/bg.png" --mode bg --hex-scale 0.67
compose "$T/mono.png" --mode mono --width 0.44
compose "$T/stat.png" --mode mono --width 0.92 --size 512

R=android/app/src/main/res
for pair in mdpi:1 hdpi:1.5 xhdpi:2 xxhdpi:3 xxxhdpi:4; do
  d=${pair%%:*}; k=${pair##*:}
  layer=$(awk "BEGIN{print int(108*$k)}")
  stat=$(awk "BEGIN{print int(24*$k)}")
  mkdir -p $R/mipmap-$d $R/drawable-$d
  for layer_name in fg:foreground bg:background mono:monochrome; do
    magick "$T/${layer_name%%:*}.png" -resize ${layer}x${layer} "$T/a.png"
    webp "$T/a.png" $R/mipmap-$d/ic_launcher_${layer_name##*:}.webp
  done
  magick "$T/stat.png" -resize ${stat}x${stat} -strip $R/drawable-$d/ic_stat_bee.png
done

echo "icons regenerated"
