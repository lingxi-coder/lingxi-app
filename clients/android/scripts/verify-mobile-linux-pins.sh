#!/usr/bin/env bash
# Android validation deliberately does not require the iOS source tree.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
SDK_ROOT="$(python3 "${REPO_ROOT}/lingxi-code/scripts/mobile_linux_source.py" --root)"
exec python3 "${SDK_ROOT}/scripts/verify-android-native.py" --source-only
