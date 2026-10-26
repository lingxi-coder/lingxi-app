#!/usr/bin/env bash
set -euo pipefail
VARIANT=""
JNI_ROOT=""
ABIS="arm64-v8a,x86_64"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:?missing variant}"; shift 2 ;;
    --abis) ABIS="${2:?missing ABI list}"; shift 2 ;;
    --jni-root) JNI_ROOT="${2:?missing JNI root}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct> [--jni-root DIR] [--abis arm64-v8a,x86_64]" >&2; exit 2 ;;
esac
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ANDROID_DIR}/../../.." && pwd)"
SDK_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --root)"
ARTIFACT_ROOT="${ANDROID_DIR}/app/build/mobileLinuxNative/${VARIANT}/native-support"
[[ -n "${JNI_ROOT}" ]] || JNI_ROOT="${ANDROID_DIR}/app/src/${VARIANT}/jniLibs"
python3 "${SDK_ROOT}/scripts/checks/verify-android-native.py" --artifact-dir "${ARTIFACT_ROOT}"
SDK_IDENTITY="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(json.dumps({k:d[k] for k in ("source","revision")}))')"
SDK_SOURCE="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["source"])' <<<"${SDK_IDENTITY}")"
SDK_REVISION="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["revision"])' <<<"${SDK_IDENTITY}")"
exec python3 "${SCRIPT_DIR}/mobile-linux-native.py" verify --source "${ARTIFACT_ROOT}" --jni-root "${JNI_ROOT}" \
  --maven-dir "${ANDROID_DIR}/build/mobileLinuxSdk/maven" --expected-source "${SDK_SOURCE}" \
  --expected-revision "${SDK_REVISION}" --abis "${ABIS}"
