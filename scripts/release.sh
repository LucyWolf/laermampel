#!/usr/bin/env bash
# Neue Version veröffentlichen: zählt die letzte Stelle hoch (0.3.1, 0.3.2 …),
# nach .99 geht es mit der nächsten mittleren Stelle weiter (0.3.99 → 0.4.0).
# Aufruf: scripts/release.sh
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Es gibt noch nicht committete Änderungen, bitte erst committen." >&2
  exit 1
fi

# Alte Releases wegräumen, bevor das neue dazukommt: es sollen immer nur zwei stehen
# (Installer sind groß). Die Git-Tags bleiben, daraus lässt sich jede Version neu bauen.
# Hier wird auf eins heruntergeräumt, weil gleich eines dazukommt.
if command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then
  gh release list --limit 200 --json tagName,createdAt --jq 'sort_by(.createdAt) | reverse | .[1:] | .[].tagName' \
    | while read -r alt; do
        gh release delete "$alt" --yes >/dev/null 2>&1 && echo "altes Release $alt gelöscht"
      done
fi

current=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
IFS=. read -r major minor patch <<<"$current"
if (( patch >= 99 )); then
  minor=$((minor + 1))
  patch=0
else
  patch=$((patch + 1))
fi
next="$major.$minor.$patch"

sed -i "0,/^version = \"$current\"/s//version = \"$next\"/" Cargo.toml
# Die eigene Version steht auch in Cargo.lock.
sed -i "/^name = \"laermampel\"$/{n;s/^version = \"$current\"/version = \"$next\"/}" Cargo.lock

git add Cargo.toml Cargo.lock
git commit -q -m "Version $next"
git tag "v$next"
git push -q
git push -q origin "v$next"
echo "v$current → v$next veröffentlicht, GitHub baut jetzt Installer und Release."
