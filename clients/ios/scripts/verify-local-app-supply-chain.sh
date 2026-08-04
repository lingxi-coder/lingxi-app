#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

python3 "${REPO_ROOT}/lingxi-code/scripts/mobile-linux/verify-local-app-supply-chain.py" \
  --repo-root "${REPO_ROOT}" \
  "$@"
