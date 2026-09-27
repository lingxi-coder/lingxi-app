#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../runtime_source.py" --root)"
HOST_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
export HARNESS_RUNTIME_BUILD_ROOT="${HOST_ROOT}/clients/ios/build"
exec bash "${RUNTIME_ROOT}/scripts/mobile-linux/build-local-app-node-modules.sh" "$@"
