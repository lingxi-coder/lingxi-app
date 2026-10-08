#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PYTHONDONTWRITEBYTECODE=1
python3 "${SCRIPT_DIR}/test_local_app_policy.py"
bash "${SCRIPT_DIR}/test-local-app-host.sh"
RUNTIME_ROOT="$(python3 "${SCRIPT_DIR}/../lib/runtime_source.py" --root)"
LOCAL_APP_ROOT="$(python3 "${SCRIPT_DIR}/../lib/local_app_source.py" --root)"
SDK_ROOT="$(python3 "${SCRIPT_DIR}/../lib/mobile_linux_source.py" --root)"
export HARNESS_RUNTIME_TEST_OUTPUT_DIR="${SCRIPT_DIR}/../../build/runtime-resource-tests"
# The runtime seed's own verifier and staging suite belongs to the Local App; the SDK's release-archive
# cases are driven through the Harness's delegates.
bash "${LOCAL_APP_ROOT}/scripts/tests/test-local-app-supply-chain.sh" --sdk-root "${SDK_ROOT}"
exec bash "${RUNTIME_ROOT}/scripts/tests/test-rootfs-release-evidence.sh" --sdk-root "${SDK_ROOT}"
