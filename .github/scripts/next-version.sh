#!/usr/bin/env bash
# Prints the next release version of one component (spec 001).
#
#   next-version.sh [android|server] [--if-changed]
#
# Each component has its own tags: the app `vX.Y.Z`, the server image
# `server-vX.Y.Z`. The last clean tag of the component is bumped by the
# Conventional Commits that touched its paths since it: a breaking change
# (`type!:` or `BREAKING CHANGE`) bumps major, a `feat` bumps minor, anything
# else bumps patch. VERSION is a shared floor, so raising it in a PR forces
# both components to jump; with no tag yet, VERSION is released.
#
# With --if-changed, prints nothing when no commit touched the component's
# paths since its last tag.
set -euo pipefail

component=${1:-android}
case "$component" in
  android) prefix=v; paths=(android protocol) ;;
  server) prefix=server-v; paths=(server deploy/Dockerfile deploy/Dockerfile.dockerignore VERSION) ;;
  *) echo "unknown component: $component" >&2; exit 2 ;;
esac

floor=$(cat VERSION)
last=$(git tag --list "${prefix}*" | grep -E "^${prefix}[0-9]+\.[0-9]+\.[0-9]+\$" | sort -V | tail -1 || true)

if [ -z "$last" ]; then
  echo "$floor"
  exit 0
fi

log=$(git log --format='%s%n%b%n--' "$last..HEAD" -- "${paths[@]}")
if [ -z "$log" ] && [ "${2:-}" = --if-changed ]; then
  exit 0
fi
IFS=. read -r major minor patch <<< "${last#"$prefix"}"
if grep -qE '^[a-z]+(\([^)]*\))?!: |^[[:space:]]*BREAKING[ -]CHANGE: ' <<< "$log"; then
  next="$((major + 1)).0.0"
elif grep -qE '^feat(\([^)]*\))?: ' <<< "$log"; then
  next="$major.$((minor + 1)).0"
else
  next="$major.$minor.$((patch + 1))"
fi

printf '%s\n%s\n' "$next" "$floor" | sort -V | tail -1
