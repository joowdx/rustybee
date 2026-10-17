#!/usr/bin/env bash
# AC-011-01: the foss release APK is under 3 MB and has no Firebase, AppCompat or
# Compose classes. Usage: ci/check-apk.sh <apk>. Needs ANDROID_HOME (for dexdump).
set -euo pipefail

apk=${1:?usage: check-apk.sh <apk>}
max=$((3 * 1024 * 1024))

size=$(wc -c < "$apk" | tr -d ' ')
if [ "$size" -ge "$max" ]; then
  echo "::error::$apk is $size bytes, over the 3 MB limit (AC-011-01)"
  exit 1
fi

dexdump="$(find "$ANDROID_HOME/build-tools" -name dexdump -type f | sort -V | tail -1)"
[ -x "$dexdump" ] || { echo "::error::dexdump not found under $ANDROID_HOME/build-tools"; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
unzip -q -o "$apk" 'classes*.dex' -d "$work"

banned='^L(com/google/firebase|com/google/android/gms|androidx/appcompat|androidx/compose)/'
classes="$work/classes.txt"
for dex in "$work"/classes*.dex; do
  "$dexdump" "$dex" | sed -n "s/^ *Class descriptor *: *'\(.*\)'.*/\1/p"
done > "$classes"

[ -s "$classes" ] || { echo "::error::no classes read from $apk"; exit 1; }
if grep -E "$banned" "$classes" > "$work/hits.txt"; then
  echo "::error::$apk contains banned classes (AC-011-01):"
  head -20 "$work/hits.txt"
  exit 1
fi

echo "AC-011-01 ok: $apk is $size bytes, $(wc -l < "$classes" | tr -d ' ') classes, none banned"
