#!/bin/sh
set -eu
if [ "$#" -ne 2 ]; then
  echo "Usage: sh $0 LINGXI_DATA_CHECKOUT NEW_CONTEXT_DIRECTORY" >&2
  exit 2
fi
firebase="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
core="$1"
context="$2"
pin="$(sed -n "s/^export const LINGXI_DATA_BASELINE = '\([a-f0-9]*\)';/\1/p" "$firebase/src/lingxi-data.mjs")"
[ "${#pin}" -eq 40 ]
git -C "$core" cat-file -e "$pin^{commit}"
mkdir "$context"
context="$(CDPATH='' cd -- "$context" && pwd)"
mkdir "$context/lingxi-data" "$context/deploy" "$context/scripts"
git -C "$core" archive --output="$context/core.tar" "$pin" Cargo.toml Cargo.lock src
tar -xf "$context/core.tar" -C "$context/lingxi-data"
rm "$context/core.tar"
cp "$firebase/package.json" "$firebase/package-lock.json" "$context/"
cp -R "$firebase/src" "$context/"
cp "$firebase/deploy/product-data.Dockerfile" "$context/Dockerfile"
cp "$firebase/deploy/start-product-data.sh" "$context/deploy/"
cp "$firebase/scripts/serve-lingxi-data.mjs" "$firebase/scripts/start-cloud-data-core.mjs" "$context/scripts/"
