#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${IOS_DIR}/../.." && pwd)"
VARIANT="store"
OUTPUT=""

usage() {
  cat <<'EOF'
Usage: build-local-app-runtime.sh [--variant <store|full>] [--output <dir>]
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "${VARIANT}" in
  store|full) ;;
  *) echo "--variant must be store or full" >&2; exit 2 ;;
esac

if [[ -z "${OUTPUT}" ]]; then
  OUTPUT="${IOS_DIR}/build/local-app-runtime/${VARIANT}"
fi

"${REPO_ROOT}/lingxi-code/scripts/mobile-linux/build-local-app-runtime.sh" \
  --platform ios \
  --variant "${VARIANT}" \
  --output "${OUTPUT}"
