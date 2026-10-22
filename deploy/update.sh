#!/usr/bin/env bash
# Puts a Plain Web release in place (ТЗ-Plain-Web §8):
#   deploy/update.sh web-0.4
# Downloads the tag's archive from GitHub unless that release is already
# here, checks its sha256, and points /opt/plain-web/current at it; the
# service, if it is set up, restarts on it. Going back is the same command
# with the previous tag — no download, the folder is still there.
set -euo pipefail

TAG=${1:?usage: update.sh <tag, e.g. web-0.4>}
# Releases come from this fork, unless /etc/plain-web/update.env (root's
# only) names another repository — PLAIN_REPO — and, for a private one, a
# read-only token, PLAIN_TOKEN.
if [ -r /etc/plain-web/update.env ]; then . /etc/plain-web/update.env; fi
REPO=${PLAIN_REPO:-ekaterina-ya/plain}
ROOT=/opt/plain-web
NAME="plain-web-$TAG-linux-x64"
DIR="$ROOT/releases/$TAG"

# One file of the release. A private repository's goes through the API; the
# token is handed to curl on stdin, never on its command line.
fetch() {
  if [ -z "${PLAIN_TOKEN:-}" ]; then
    curl -fsSL -o "$2" "https://github.com/$REPO/releases/download/$TAG/$1"
    return
  fi
  local auth id
  auth=$(printf 'header = "Authorization: Bearer %s"' "$PLAIN_TOKEN")
  id=$(curl -fsSL -K - "https://api.github.com/repos/$REPO/releases/tags/$TAG" <<<"$auth" \
    | jq -r --arg name "$1" '.assets[] | select(.name == $name) | .id')
  [ -n "$id" ] || { echo "$TAG: no $1 in $REPO"; return 1; }
  curl -fsSL -K - -H 'Accept: application/octet-stream' -o "$2" \
    "https://api.github.com/repos/$REPO/releases/assets/$id" <<<"$auth"
}

mkdir -p "$ROOT/releases"
if [ ! -x "$DIR/plain-web" ]; then
  work=$(mktemp -d "$ROOT/releases/.new.XXXX")
  trap 'rm -rf "$work"' EXIT
  fetch "$NAME.tar.gz" "$work/$NAME.tar.gz"
  fetch "$NAME.tar.gz.sha256" "$work/$NAME.tar.gz.sha256"
  (cd "$work" && sha256sum -c "$NAME.tar.gz.sha256")
  tar xzf "$work/$NAME.tar.gz" --no-same-owner -C "$work"
  rm -rf "$DIR"
  mv "$work/$NAME" "$DIR"
fi

# The health check asks the server by its own address — the unit's last
# --listen, else the default. Worked out before anything is switched.
listen=$(systemctl cat plain-web 2>/dev/null | grep '^ExecStart=.' | tail -n 1 \
  | grep -o -- '--listen [^ ]*' | cut -d' ' -f2 || true)
listen=${listen:-127.0.0.1:8642}

# A new link, not new files under the running binary.
prev=$(readlink "$ROOT/current" || true)
ln -sfn "releases/$TAG" "$ROOT/current"
echo "current → $TAG"

if systemctl is-enabled --quiet plain-web 2>/dev/null; then
  systemctl restart plain-web
  # A large library takes a moment to watch before the port opens.
  for _ in $(seq 15); do
    if curl -fsS "http://$listen/robots.txt" >/dev/null 2>&1; then
      echo "plain-web answers"
      exit 0
    fi
    sleep 1
  done
  echo "plain-web does not answer: journalctl -u plain-web"
  [ -n "$prev" ] && echo "back to the previous release: $0 ${prev#releases/}"
  exit 1
fi
