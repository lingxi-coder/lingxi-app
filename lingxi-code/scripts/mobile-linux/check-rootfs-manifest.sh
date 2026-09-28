#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../mobile_linux_source.py" --root)"
exec bash "${RUNTIME_ROOT}/scripts/mobile-linux/check-rootfs-manifest.sh" "$@"
