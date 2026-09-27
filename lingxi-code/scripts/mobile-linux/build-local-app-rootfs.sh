#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../runtime_source.py" --root)"
HOST_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
exec bash "${RUNTIME_ROOT}/scripts/mobile-linux/build-local-app-rootfs.sh" --output-dir "${HOST_ROOT}/clients/ios/build/local-app-rootfs" --cache-dir "${HOST_ROOT}/clients/ios/build/local-app-cache" "$@"
