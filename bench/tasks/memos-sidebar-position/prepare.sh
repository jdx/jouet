#!/bin/sh
# Populate the optional task's isolated pnpm and Chromium caches.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source_repo="$HOME/src/memos-bench-history"
cache="$HOME/.cache/jouet-bench"
pnpm_root="$cache/memos-pnpm"
store="$cache/memos-store"
browsers="$cache/memos-playwright"
revision=6529609283f6beb3db44e9077332fe3e6a2fcc89
if [ ! -d "$source_repo/.git" ]; then
  printf 'Clone usememos/memos to %s first.\n' "$source_repo" >&2
  exit 1
fi
mkdir -p "$cache"
npm install --prefix "$pnpm_root" --no-save --ignore-scripts pnpm@11.0.1
pnpm="$pnpm_root/node_modules/.bin/pnpm"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/jouet-memos-prepare.XXXXXXXX")
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
git clone --quiet --local --no-hardlinks --no-checkout "$source_repo" "$scratch/work"
git -C "$scratch/work" checkout --quiet --detach "$revision"
git -C "$scratch/work" apply --unidiff-zero "$root/checker-tools.patch"
CI=1 "$pnpm" --dir "$scratch/work/web" --store-dir "$store" install --frozen-lockfile
PLAYWRIGHT_BROWSERS_PATH="$browsers" node "$scratch/work/web/node_modules/playwright/cli.js" install --only-shell chromium
printf 'Memos benchmark dependencies prepared in %s\n' "$cache"
