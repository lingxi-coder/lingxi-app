#!/usr/bin/env bash
set -euo pipefail

VARIANT=""
SKIP_JNI=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:-}"; shift 2 ;;
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
REPO_ROOT="$(cd "${ANDROID_DIR}/../.." && pwd)"
OPENMINIS="${OPENMINIS_SOURCE:-${REPO_ROOT}/docs/superpowers/references/OpenMinis}"
PROOT="${PROOT_SOURCE:-${OPENMINIS}/deps/proot}"
JNI_ROOT="${ANDROID_DIR}/app/src/${VARIANT}/jniLibs"
BUILD_ROOT="${ANDROID_DIR}/app/build/mobileLinuxNative/${VARIANT}"
ANDROID_API=26

"${SCRIPT_DIR}/verify-mobile-linux-pins.sh"
if [[ "${SKIP_JNI}" == false ]]; then
  "${SCRIPT_DIR}/build-jni.sh" --variant "${VARIANT}"
fi

if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-${HOME}/Library/Android/sdk}}"
  NDK_PATH="$(find "${SDK_ROOT}/ndk" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null | sort -V | tail -1)"
  [[ -n "${NDK_PATH}" ]] || { echo "Android NDK not found; set ANDROID_NDK_HOME" >&2; exit 1; }
  ANDROID_NDK_HOME="${NDK_PATH}"
fi
export ANDROID_NDK_HOME

case "$(uname -s)" in
  Darwin) HOST_TAG="darwin-x86_64" ;;
  Linux) HOST_TAG="linux-x86_64" ;;
  *) echo "unsupported build host: $(uname -s)" >&2; exit 1 ;;
esac
TOOLCHAIN="${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/${HOST_TAG}"
TOOLCHAIN_BIN="${TOOLCHAIN}/bin"
[[ -x "${TOOLCHAIN_BIN}/llvm-strip" ]] || { echo "invalid NDK toolchain: ${TOOLCHAIN}" >&2; exit 1; }
mkdir -p "${BUILD_ROOT}/host-tools"
ln -sf "${TOOLCHAIN_BIN}/llvm-readelf" "${BUILD_ROOT}/host-tools/readelf"
export PATH="${BUILD_ROOT}/host-tools:${TOOLCHAIN_BIN}:${PATH}"

CMAKE="${CMAKE:-}"
if [[ -z "${CMAKE}" ]]; then
  CMAKE="$(find "${ANDROID_SDK_ROOT:-${ANDROID_HOME:-${HOME}/Library/Android/sdk}}/cmake" -type f -path '*/bin/cmake' 2>/dev/null | sort -V | tail -1)"
fi
[[ -x "${CMAKE}" ]] || { echo "CMake not found; set CMAKE" >&2; exit 1; }

machine_jobs() {
  sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4
}

build_proot() {
  local abi="$1"
  local triple="$2"
  local work="${BUILD_ROOT}/proot-${abi}"
  local proot_work="${work}/source"
  local talloc_work="${work}/talloc"
  local cc="${TOOLCHAIN_BIN}/${triple}${ANDROID_API}-clang"

  "${CMAKE}" -E remove_directory "${work}"
  mkdir -p "${proot_work}" "${talloc_work}"
  git -C "${PROOT}" archive --format=tar HEAD | tar -xf - -C "${proot_work}"
  git -C "${OPENMINIS}" show HEAD:deps/talloc/talloc.c > "${talloc_work}/talloc.c"
  git -C "${OPENMINIS}" show HEAD:deps/talloc/talloc.h > "${talloc_work}/talloc.h"
  git -C "${OPENMINIS}" show HEAD:deps/talloc/replace.h > "${talloc_work}/replace.h"

  "${cc}" -c "${talloc_work}/talloc.c" \
    -o "${talloc_work}/talloc.o" \
    -I"${talloc_work}" -fPIC -O2 -std=gnu99 \
    -DHAVE_STDARG_H=1 -DHAVE_VA_COPY=1 -DHAVE_UNISTD_H=1 -DHAVE_INTPTR_T=1
  "${TOOLCHAIN_BIN}/llvm-ar" rcs "${talloc_work}/libtalloc.a" "${talloc_work}/talloc.o"

  (
    cd "${proot_work}/src"
    make \
      CC="${cc}" \
      STRIP="${TOOLCHAIN_BIN}/llvm-strip" \
      OBJCOPY="${TOOLCHAIN_BIN}/llvm-objcopy" \
      OBJDUMP="${TOOLCHAIN_BIN}/llvm-objdump" \
      PROOT_UNBUNDLE_LOADER="/proc/self/fd" \
      CPPFLAGS="-D_FILE_OFFSET_BITS=64 -D_GNU_SOURCE -I. -DARG_MAX=131072 -I${talloc_work}" \
      CFLAGS="-O2 -Wall -Wextra -fPIE -DPROOT_UNBUNDLE_LOADER=\\\"/proc/self/fd\\\"" \
      LDFLAGS="-Wl,-z,noexecstack -pie ${talloc_work}/libtalloc.a" \
      -j"$(machine_jobs)"
  )

  mkdir -p "${JNI_ROOT}/${abi}"
  cp "${proot_work}/src/proot" "${JNI_ROOT}/${abi}/libproot.so"
  cp "${proot_work}/src/loader/loader" "${JNI_ROOT}/${abi}/libproot-loader.so"
  "${TOOLCHAIN_BIN}/llvm-strip" "${JNI_ROOT}/${abi}/libproot.so"
}

build_pty() {
  local abi="$1"
  local work="${BUILD_ROOT}/pty-${abi}"
  local source="${work}/source"
  local output="${work}/output"

  "${CMAKE}" -E remove_directory "${work}"
  mkdir -p "${source}" "${output}" "${JNI_ROOT}/${abi}"
  git -C "${OPENMINIS}" show HEAD:src/android/app/src/main/cpp/pty_bridge.c > "${source}/pty_bridge.c"
  {
    echo 'cmake_minimum_required(VERSION 3.22.1)'
    echo 'project(lingxi_pty_bridge C)'
    echo 'add_library(pty_bridge SHARED pty_bridge.c)'
    echo 'find_library(log-lib log)'
    printf '%s\n' "target_link_libraries(pty_bridge \${log-lib})"
  } > "${source}/CMakeLists.txt"
  "${CMAKE}" \
    -S "${source}" \
    -B "${output}" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_TOOLCHAIN_FILE="${ANDROID_NDK_HOME}/build/cmake/android.toolchain.cmake" \
    -DANDROID_ABI="${abi}" \
    -DANDROID_PLATFORM="android-${ANDROID_API}"
  "${CMAKE}" --build "${output}" --parallel "$(machine_jobs)"
  cp "${output}/libpty_bridge.so" "${JNI_ROOT}/${abi}/libpty_bridge.so"
}

build_proot "arm64-v8a" "aarch64-linux-android"
build_proot "x86_64" "x86_64-linux-android"
build_pty "arm64-v8a"
build_pty "x86_64"
"${SCRIPT_DIR}/verify-mobile-linux-native.sh" --variant "${VARIANT}"
