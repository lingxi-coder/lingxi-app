#!/usr/bin/env bash
set -euo pipefail
VARIANT=""
JNI_ROOT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:?missing variant}"; shift 2 ;;
    --jni-root) JNI_ROOT="${2:?missing JNI root}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct> [--jni-root DIR]" >&2; exit 2 ;;
esac
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ANDROID_DIR}/../.." && pwd)"
SDK_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --root)"
ARTIFACT_ROOT="${ANDROID_DIR}/app/build/mobileLinuxNative/${VARIANT}/native-support"
[[ -n "${JNI_ROOT}" ]] || JNI_ROOT="${ANDROID_DIR}/app/src/${VARIANT}/jniLibs"
python3 "${SDK_ROOT}/scripts/checks/verify-android-native.py" --artifact-dir "${ARTIFACT_ROOT}"
SDK_REVISION="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["revision"])')"
exec python3 "${SCRIPT_DIR}/mobile-linux-native.py" verify --source "${ARTIFACT_ROOT}" --jni-root "${JNI_ROOT}" \
  --maven-dir "${ANDROID_DIR}/build/mobileLinuxSdk/maven" --expected-revision "${SDK_REVISION}"
