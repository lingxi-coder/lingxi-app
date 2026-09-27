#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bash "${SCRIPT_DIR}/test-local-app-host.sh"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../runtime_source.py" --root)"
export HARNESS_RUNTIME_TEST_OUTPUT_DIR="${SCRIPT_DIR}/../../../build/runtime-resource-tests"
export PYTHONDONTWRITEBYTECODE=1
exec bash "${RUNTIME_ROOT}/scripts/mobile-linux/test-local-app-supply-chain.sh"
