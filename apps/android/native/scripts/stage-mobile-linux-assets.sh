#!/usr/bin/env bash
set -euo pipefail

VARIANT=""
INPUT_DIR=""
APK_DIR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:-}"; shift 2 ;;
    --input) INPUT_DIR="${2:-}"; shift 2 ;;
    --apk-dir) APK_DIR="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct> --input <release-evidence-dir> --apk-dir <apk-closure-dir>" >&2; exit 2 ;;
esac
[[ -d "${INPUT_DIR}" ]] || { echo "input directory not found: ${INPUT_DIR}" >&2; exit 1; }
[[ -d "${APK_DIR}" ]] || { echo "APK closure directory not found: ${APK_DIR}" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ANDROID_DIR}/../../.." && pwd)"
LOCAL_APP_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/local_app_source.py" --root)"
SDK_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --root)"
NATIVE_ROOT="${ANDROID_DIR}/app/build/mobileLinuxNative/${VARIANT}/native-support"
OUTPUT="${ANDROID_DIR}/app/build/generated/mobileLinux/${VARIANT}/assets/mobile-linux"

bash "${SCRIPT_DIR}/verify-local-app-supply-chain.sh" --release --apk-dir "${APK_DIR}"

"${SCRIPT_DIR}/verify-mobile-linux-native.sh" --variant "${VARIANT}"

exec python3 "${SCRIPT_DIR}/stage-mobile-linux-assets.py" \
  --input "${INPUT_DIR}" --output "${OUTPUT}" --sdk-root "${SDK_ROOT}" \
  --local-app-root "${LOCAL_APP_ROOT}" --native-root "${NATIVE_ROOT}"
