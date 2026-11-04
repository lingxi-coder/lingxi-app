#!/bin/sh
set -eu
cd "$(dirname "$0")"

expected=c08d65885ee38876c4f86fa503fb49d7b507c2b62552df7c70b2fce627e06381
actual=$(shasum -a 256 opaque-debug-0.3.1.crate | cut -d ' ' -f 1)
test "$actual" = "$expected"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/opaque-debug-audit.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM
tar -xzf opaque-debug-0.3.1.crate -C "$tmp_dir"
rustc +1.94.0 --edition=2018 --crate-name opaque_debug --crate-type lib \
  "$tmp_dir/opaque-debug-0.3.1/src/lib.rs" --out-dir "$tmp_dir"
rustc +1.94.0 --edition=2018 --test \
  --extern "opaque_debug=$tmp_dir/libopaque_debug.rlib" \
  "$tmp_dir/opaque-debug-0.3.1/tests/mod.rs" -o "$tmp_dir/upstream-tests"
"$tmp_dir/upstream-tests"
rustdoc +1.94.0 --edition=2018 --test \
  --extern "opaque_debug=$tmp_dir/libopaque_debug.rlib" \
  "$tmp_dir/opaque-debug-0.3.1/src/lib.rs"
rustc +1.94.0 --edition=2021 --crate-name opaque_debug_probe \
  --extern "opaque_debug=$tmp_dir/libopaque_debug.rlib" probe.rs -o "$tmp_dir/probe"
"$tmp_dir/probe"
