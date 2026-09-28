#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../lib/runtime_source.py" --root)"
SDK_ROOT="$(python3 "${SCRIPT_DIR}/../lib/mobile_linux_source.py" --root)"
HOST_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
ARCH=""
ARGS=("$@")
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:?missing architecture}"; shift 2 ;;
    *) shift ;;
  esac
done
exec bash "${RUNTIME_ROOT}/scripts/local-apps/build-local-app-node-modules.sh" \
  --sdk-root "${SDK_ROOT}" \
  --rootfs "${HOST_ROOT}/clients/ios/build/local-app-rootfs/${ARCH}/rootfs.tar.gz" \
  --output-dir "${HOST_ROOT}/clients/ios/build/local-app-node-modules/${ARCH}" \
  --cache-dir "${HOST_ROOT}/clients/ios/build/local-app-cache" "${ARGS[@]}"
