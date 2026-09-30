#!/usr/bin/env bash
# Build LingXi's FFI plus the Cargo-locked SDK's native-support-only payload.
set -euo pipefail
VARIANT=""
SKIP_JNI=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:?missing variant}"; shift 2 ;;
    --skip-jni) SKIP_JNI=true; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct> [--skip-jni]" >&2; exit 2 ;;
esac
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ANDROID_DIR}/../../.." && pwd)"
SDK_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --root)"
BUILD_ROOT="${ANDROID_DIR}/app/build/mobileLinuxNative/${VARIANT}"
# Both distributions consume one Maven coordinate for the SDK helpers. Build
# those helpers once per invocation and stage the exact same bytes for each
# variant; their Rust FFI libraries remain variant-specific below.
ARTIFACT_ROOT="${ANDROID_DIR}/app/build/mobileLinuxNative/shared/native-support"
JNI_ROOT="${LINGXI_ANDROID_JNILIBS_DIR:-${ANDROID_DIR}/app/src/${VARIANT}/jniLibs}"
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  NDK_BASE="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-${HOME}/Library/Android/sdk}}/ndk"
  [[ -d "${NDK_BASE}" ]] || { echo "Android NDK missing; set ANDROID_NDK_HOME" >&2; exit 1; }
  NDK_VERSION="$(ls -1 "${NDK_BASE}" | sort -V | tail -1)"
  [[ -n "${NDK_VERSION}" ]] || { echo "Android NDK missing" >&2; exit 1; }
  export ANDROID_NDK_HOME="${NDK_BASE}/${NDK_VERSION}"
fi
ARGS=(--ndk "${ANDROID_NDK_HOME}" --output-dir "${ARTIFACT_ROOT}"
  --cache-dir "${ANDROID_DIR}/app/build/mobileLinuxNative/cache" --abi all)
if [[ -n "${PROOT_SOURCE:-}" ]]; then ARGS+=(--proot-source "${PROOT_SOURCE}"); fi
bash "${SDK_ROOT}/scripts/build/build-android-native.sh" "${ARGS[@]}"
python3 "${SDK_ROOT}/scripts/checks/verify-android-native.py" --artifact-dir "${ARTIFACT_ROOT}"
for distribution in play direct; do
  distribution_root="${ANDROID_DIR}/app/build/mobileLinuxNative/${distribution}/native-support"
  rm -rf "${distribution_root}"
  mkdir -p "$(dirname "${distribution_root}")"
  cp -R "${ARTIFACT_ROOT}" "${distribution_root}"
  python3 "${SDK_ROOT}/scripts/checks/verify-android-native.py" --artifact-dir "${distribution_root}"
done
if [[ "${SKIP_JNI}" == false ]]; then
  "${SCRIPT_DIR}/build-jni.sh" --variant "${VARIANT}"
fi
# Publish metadata-backed AARs outside the immutable SDK checkout. Their native
# helpers are packaged once through Gradle; LingXi owns only libandroid_aar.so.
SDK_VERSION="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["sdk_version"])' "${REPO_ROOT}/docs/mobile-linux/mobile-linux-native-pins.json")"
python3 "${SDK_ROOT}/scripts/release/publish-android.py" --native-artifacts "${ARTIFACT_ROOT}" \
  --maven-dir "${ANDROID_DIR}/build/mobileLinuxSdk/maven" \
  --build-dir "${ANDROID_DIR}/build/mobileLinuxSdk/gradle" --version "${SDK_VERSION}" --native-only
# Delete only obsolete generated helper copies after validated AAR publication.
for abi in arm64-v8a x86_64; do
  for name in libproot.so libproot-loader.so libmobile_linux_policy_launcher.so libpty_bridge.so libmksh.so libtoybox.so; do
    rm -f "${JNI_ROOT}/${abi}/${name}"
  done
done
python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --json | python3 -c 'import json,sys; d=json.load(sys.stdin); json.dump({k:d[k] for k in ("source","revision")},sys.stdout)' > "${BUILD_ROOT}/sdk-source.json"
"${SCRIPT_DIR}/verify-mobile-linux-native.sh" --variant "${VARIANT}" --jni-root "${JNI_ROOT}"
