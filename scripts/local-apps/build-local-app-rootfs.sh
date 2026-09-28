#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../lib/runtime_source.py" --root)"
SDK_ROOT="$(python3 "${SCRIPT_DIR}/../lib/mobile_linux_source.py" --root)"
HOST_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
exec bash "${RUNTIME_ROOT}/scripts/local-apps/build-local-app-rootfs.sh" --sdk-root "${SDK_ROOT}" --output-dir "${HOST_ROOT}/clients/ios/build/local-app-rootfs" --cache-dir "${HOST_ROOT}/clients/ios/build/local-app-cache" "$@"
