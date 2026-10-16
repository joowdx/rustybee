#!/usr/bin/env bash
# Prints the next Android release version (spec 001).
#
# The last clean vX.Y.Z tag is bumped by the Conventional Commits that touched
# the app since it: a breaking change (`type!:` or `BREAKING CHANGE`) bumps
# major, a `feat` bumps minor, anything else bumps patch. VERSION is a floor,
# so raising it in a PR forces a jump; with no tag yet, VERSION is released.
set -euo pipefail

paths=(android protocol)
floor=$(cat VERSION)
last=$(git tag --list 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)

if [ -z "$last" ]; then
  echo "$floor"
  exit 0
fi

log=$(git log --format='%s%n%b%n--' "$last..HEAD" -- "${paths[@]}")
IFS=. read -r major minor patch <<< "${last#v}"
if grep -qE '^[a-z]+(\([^)]*\))?!: |^[[:space:]]*BREAKING[ -]CHANGE: ' <<< "$log"; then
  next="$((major + 1)).0.0"
elif grep -qE '^feat(\([^)]*\))?: ' <<< "$log"; then
  next="$major.$((minor + 1)).0"
else
  next="$major.$minor.$((patch + 1))"
fi

printf '%s\n%s\n' "$next" "$floor" | sort -V | tail -1
