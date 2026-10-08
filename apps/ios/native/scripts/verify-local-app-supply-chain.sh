#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

python3 "${REPO_ROOT}/scripts/local-apps/verify-local-app-host.py" --repo-root "${REPO_ROOT}"
LOCAL_APP_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/local_app_source.py" --root)"
SDK_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --root)"
python3 "${LOCAL_APP_ROOT}/scripts/runtime/verify-local-app-supply-chain.py" \
  --repo-root "${LOCAL_APP_ROOT}" --sdk-root "${SDK_ROOT}" \
  "$@"
