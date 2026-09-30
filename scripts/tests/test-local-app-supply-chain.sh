#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PYTHONDONTWRITEBYTECODE=1
python3 "${SCRIPT_DIR}/test_local_app_policy.py"
bash "${SCRIPT_DIR}/test-local-app-host.sh"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../lib/runtime_source.py" --root)"
SDK_ROOT="$(python3 "${SCRIPT_DIR}/../lib/mobile_linux_source.py" --root)"
export HARNESS_RUNTIME_TEST_OUTPUT_DIR="${SCRIPT_DIR}/../../build/runtime-resource-tests"
exec bash "${RUNTIME_ROOT}/scripts/tests/test-local-app-supply-chain.sh" --sdk-root "${SDK_ROOT}"
