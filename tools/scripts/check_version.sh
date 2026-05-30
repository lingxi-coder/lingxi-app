#!/usr/bin/env bash
# tools/scripts/check_version.sh
# Asserts every Cargo.toml in the lingxi-code workspace carries
# `version = "0.5.0"`. M4-09 Task 6 gate.
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION="0.5.0"
mismatches=()
while IFS= read -r f; do
    # Skip the workspace root Cargo.toml (no [package] version field).
    if grep -q '^\[workspace\]' "$f"; then
        continue
    fi
    if ! grep -q "^version = \"${VERSION}\"\$" "$f"; then
        mismatches+=("$f")
    fi
done < <(find lingxi-code -name "Cargo.toml" -not -path "*/target/*")

if (( ${#mismatches[@]} )); then
    echo "VERSION MISMATCH — these files do not carry version = \"${VERSION}\":"
    printf '  - %s\n' "${mismatches[@]}"
    exit 1
fi
echo "OK: all Cargo.toml files at ${VERSION}"
