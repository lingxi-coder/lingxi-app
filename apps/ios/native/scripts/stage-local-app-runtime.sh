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
  store|full) ;;
  *) echo "usage: $0 --variant <store|full> --node-modules <dir>" >&2; exit 2 ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${IOS_DIR}/../../.." && pwd)"
RUNTIME_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/runtime_source.py" --root)"
OUTPUT="${IOS_DIR}/build/local-app-runtime/${VARIANT}"

python3 "${RUNTIME_ROOT}/scripts/local-apps/stage-local-app-runtime.py" \
  --repo-root "${RUNTIME_ROOT}" \
  --node-modules "${NODE_MODULES}" \
  --output "${OUTPUT}" \
  --platform ios \
  --variant "${VARIANT}"
