#!/usr/bin/env bash
# Download the external CPU test suites into $GBAA_DATA (default
# ~/.local/share/gbaa). Each suite is a shallow checkout pinned to a commit,
# so reruns are no-ops and every machine tests against the same data.
# To move a suite forward, change its commit here. The script owns these
# directories: a changed, missing or extra file triggers a re-checkout, and
# extra files are deleted.
set -euo pipefail

data="${GBAA_DATA:-$HOME/.local/share/gbaa}"
if [[ "$data" != /* ]]; then
  echo "GBAA_DATA must be an absolute path, got: $data" >&2
  exit 1
fi
mkdir -p "$data"

# name  url  commit
suites=(
  "SingleStepTests-ARM7TDMI https://github.com/SingleStepTests/ARM7TDMI e3097d88d428752736b949d94c094d5da50f0db6"
  "gba-tests https://github.com/jsmolka/gba-tests a7113b67e63f83a9b321696ddd7042ccfad6c881"
  "armwrestler https://github.com/destoer/armwrestler-gba-fixed 802e55ad61b421f4bbbc6f74f96aa4e5df4d630f"
)

for suite in "${suites[@]}"; do
  read -r name url commit <<<"$suite"
  dest="$data/$name"
  if [[ -d "$dest/.git" ]] \
    && [[ "$(git -C "$dest" rev-parse HEAD 2>/dev/null)" == "$commit" ]] \
    && [[ -z "$(git -C "$dest" status --porcelain --ignored)" ]]; then
    echo "$name: up to date ($commit)"
    continue
  fi
  echo "$name: fetching $url at $commit"
  mkdir -p "$dest"
  git -C "$dest" init -q
  git -C "$dest" remote remove origin 2>/dev/null || true
  git -C "$dest" remote add origin "$url"
  git -C "$dest" fetch -q --depth 1 origin "$commit"
  git -C "$dest" checkout -q --force --detach FETCH_HEAD
  git -C "$dest" clean -q -fdx
done

echo "test data in $data"
