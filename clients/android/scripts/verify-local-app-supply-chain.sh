#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

python3 "${REPO_ROOT}/scripts/local-apps/verify-local-app-host.py" --repo-root "${REPO_ROOT}"
RUNTIME_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/runtime_source.py" --root)"
python3 "${RUNTIME_ROOT}/scripts/local-apps/verify-local-app-supply-chain.py" \
  --repo-root "${RUNTIME_ROOT}" \
  "$@"
