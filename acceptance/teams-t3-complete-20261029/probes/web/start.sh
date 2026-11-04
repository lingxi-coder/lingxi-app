#!/bin/sh
set -eu
probe_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$probe_dir/../../../.." && pwd)
web_dir="$repo_dir/apps/web"
cd "$web_dir"
exec "$web_dir/node_modules/.bin/vite" --config "$web_dir/test/teams-t3-probe.vite.config.ts"
