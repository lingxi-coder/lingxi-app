#!/usr/bin/env bash
set -euo pipefail

VARIANT=""
NODE_MODULES=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:-}"; shift 2 ;;
    --node-modules) NODE_MODULES="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct> --node-modules <dir>" >&2; exit 2 ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ANDROID_DIR}/../.." && pwd)"
RUNTIME_ROOT="$(python3 "${REPO_ROOT}/lingxi-code/scripts/runtime_source.py" --root)"
OUTPUT="${ANDROID_DIR}/app/build/generated/localApps/${VARIANT}/assets/local-app-runtime"

python3 "${RUNTIME_ROOT}/scripts/mobile-linux/stage-local-app-runtime.py" \
  --repo-root "${RUNTIME_ROOT}" \
  --node-modules "${NODE_MODULES}" \
  --output "${OUTPUT}" \
  --platform android \
  --variant "${VARIANT}"
