#!/bin/bash

# Rust's --remap-path-prefix does not rewrite C/C++ __FILE__ strings, including
# assertion diagnostics embedded by cryptographic dependencies.
configure_native_path_remapping() {
  local native_path
  for native_path in "$@"; do
    if [[ "$native_path" == *[[:space:]]* ]]; then
      printf 'Native path remapping requires source and cache directories without whitespace: %s\n' "$native_path" >&2
      return 1
    fi
  done
  local native_flags="-ffile-prefix-map=$1=. -ffile-prefix-map=$2=/cargo-home -ffile-prefix-map=$3=/rustup -ffile-prefix-map=$4=/cargo-target"
  export CFLAGS="${CFLAGS:+${CFLAGS} }${native_flags}"
  export CXXFLAGS="${CXXFLAGS:+${CXXFLAGS} }${native_flags}"
}
