#!/usr/bin/env bash
#
# T2.4 — build-jni.sh  (Android counterpart of clients/ios/scripts/build-xcframework.sh)
#
# Reproduces the GITIGNORED Android integration artifacts the Gradle project links
# against, so a fresh checkout can build the app:
#
#   1. cargo-ndk cross-compiles the `android-aar` crate's `cdylib`
#      (`libandroid_aar.so`) for every target ABI into
#        clients/android/app/src/<play|direct>/jniLibs/<abi>/libandroid_aar.so
#      ABIs: arm64-v8a (aarch64-linux-android) + x86_64 (x86_64-linux-android).
#   2. Generates the Kotlin UniFFI bindings from the built `.so` in `--library`
#      mode into
#        clients/android/app/src/main/java/  (package com.lingxi.code.bindings)
#      using apps/android-aar/uniffi.toml for the package name.
#
# Bindgen: we DO NOT use the stock `uniffi-bindgen` — its 0.28.3 `--library`
# pipeline PANICS on our surface (cross-crate `ClientError` throw type recorded as
# `Type::External`). We reuse the SAME offline bindgen bin the iOS script uses
# (`ios-framework --features cli --bin uniffi-bindgen`), which re-tags that one
# external throw type so 0.28.3 can render it; T2.4 taught it `--language kotlin`.
#
# Idempotent: regenerated dirs are cleaned first; safe to re-run. Prints output
# paths on success. NO secrets baked in (the LLM API key is read at runtime).

set -euo pipefail

VARIANT="play"
if [[ "${1:-}" == "--variant" ]]; then
  VARIANT="${2:-}"
  shift 2
fi
case "${VARIANT}" in
  play|direct) ;;
  *) echo "ERROR: --variant must be play or direct" >&2; exit 2 ;;
esac

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"                  # clients/android
REPO_ROOT="$(cd "${ANDROID_DIR}/../.." && pwd)"               # worktree root
CARGO_DIR="${REPO_ROOT}/lingxi-code"                          # Rust workspace
CARGO_TARGET_DIR="${CARGO_DIR}/target"

CRATE="android-aar"
LIB_STEM="android_aar"                # cargo turns the `-` into `_`
SONAME="lib${LIB_STEM}.so"
HOST_DYLIB="lib${LIB_STEM}.dylib"     # macOS host cdylib (bindgen introspection)

UNIFFI_CONFIG="${CARGO_DIR}/apps/${CRATE}/uniffi.toml"

# Build into a gitignored staging directory, then atomically replace only this
# script's three owned files after every ABI and binding step succeeds. A Rust
# compile failure must not erase the last known-good app binaries.
FINAL_JNILIBS_DIR="${ANDROID_DIR}/app/src/${VARIANT}/jniLibs"
JNILIBS_DIR="${ANDROID_DIR}/app/build/nativeStaging/${VARIANT}/jniLibs"
KOTLIN_OUT="${ANDROID_DIR}/app/src/main/java"   # bindgen writes <pkg-path>/*.kt under here

PROFILE="release"
PROFILE_DIR="release"

# Rust target triple → Android ABI directory name. (Portable to bash 3.2 — macOS
# ships no associative arrays, so map via a case function instead of `declare -A`.)
TARGETS=("aarch64-linux-android" "x86_64-linux-android")
abi_of() {
  case "$1" in
    aarch64-linux-android) echo "arm64-v8a" ;;
    armv7-linux-androideabi) echo "armeabi-v7a" ;;
    x86_64-linux-android) echo "x86_64" ;;
    i686-linux-android) echo "x86" ;;
    *) echo "ERROR: no ABI mapping for target $1" >&2; return 1 ;;
  esac
}

log() { printf '\033[1;34m[build-jni]\033[0m %s\n' "$*"; }

# ---------------------------------------------------------------------------
# 0. Preflight — tools, NDK, Rust targets
# ---------------------------------------------------------------------------
for tool in cargo rustc cargo-ndk; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "ERROR: required tool not found: ${tool}" >&2; exit 1; }
done

# Resolve ANDROID_NDK_HOME: honor an existing env, else pick the newest NDK under
# the SDK's ndk/ dir (highest version-sorted directory name).
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  NDK_BASE="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-${HOME}/Library/Android/sdk}}/ndk"
  [[ -d "${NDK_BASE}" ]] || { echo "ERROR: no NDK dir at ${NDK_BASE}; set ANDROID_NDK_HOME" >&2; exit 1; }
  NEWEST_NDK="$(ls -1 "${NDK_BASE}" 2>/dev/null | sort -V | tail -1)"
  [[ -n "${NEWEST_NDK}" ]] || { echo "ERROR: no NDK versions under ${NDK_BASE}" >&2; exit 1; }
  export ANDROID_NDK_HOME="${NDK_BASE}/${NEWEST_NDK}"
fi
[[ -d "${ANDROID_NDK_HOME}" ]] || { echo "ERROR: ANDROID_NDK_HOME does not exist: ${ANDROID_NDK_HOME}" >&2; exit 1; }
log "Using NDK: ${ANDROID_NDK_HOME}"

# Ensure the Android std targets are installed for the active toolchain.
ACTIVE_TOOLCHAIN="$(cd "${CARGO_DIR}" && rustup show active-toolchain 2>/dev/null | awk '{print $1}')"
if [[ -n "${ACTIVE_TOOLCHAIN}" ]]; then
  INSTALLED_TARGETS="$(rustup target list --toolchain "${ACTIVE_TOOLCHAIN}" --installed 2>/dev/null || true)"
  MISSING=()
  for t in "${TARGETS[@]}"; do
    if [[ -n "${INSTALLED_TARGETS}" ]] && ! grep -qx "${t}" <<<"${INSTALLED_TARGETS}"; then
      MISSING+=("${t}")
    fi
  done
  if [[ ${#MISSING[@]} -gt 0 ]]; then
    log "Installing missing Rust std targets for ${ACTIVE_TOOLCHAIN}: ${MISSING[*]}"
    rustup target add --toolchain "${ACTIVE_TOOLCHAIN}" "${MISSING[@]}"
  fi
fi

# ---------------------------------------------------------------------------
# 0b. Vendored sherpa-onnx AAR (offline voice runtime) — gitignored, fetched
#     from GitHub Releases v1.13.2 into app/libs/ (consumed via the flatDir repo
#     in settings.gradle.kts). ~38 MB; skipped if already present.
# ---------------------------------------------------------------------------
SHERPA_AAR_VER="1.13.2"
SHERPA_AAR="${ANDROID_DIR}/app/libs/sherpa-onnx-static-link-onnxruntime-${SHERPA_AAR_VER}.aar"
if [[ ! -f "${SHERPA_AAR}" ]]; then
  log "Downloading sherpa-onnx AAR v${SHERPA_AAR_VER} → ${SHERPA_AAR}"
  mkdir -p "$(dirname "${SHERPA_AAR}")"
  curl -fsSL "https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_AAR_VER}/sherpa-onnx-static-link-onnxruntime-${SHERPA_AAR_VER}.aar" -o "${SHERPA_AAR}" \
    || { echo "ERROR: failed to fetch sherpa-onnx AAR" >&2; exit 1; }
  log "  $(du -h "${SHERPA_AAR}" | awk '{print $1}')"
else
  log "sherpa-onnx AAR present: ${SHERPA_AAR}"
fi

# ---------------------------------------------------------------------------
# 1. cargo-ndk cross-compile the cdylib into jniLibs/<abi>/
# ---------------------------------------------------------------------------
# `cargo ndk -t <abi> -o <jniLibs>` places each built `.so` under
# <jniLibs>/<abi>/. We pass the Gradle ABI names; cargo-ndk maps them to triples.
log "Cross-compiling ${CRATE} cdylib (${PROFILE}) for: ${TARGETS[*]/#/}"
if [[ -d "${JNILIBS_DIR}" ]]; then
  find "${JNILIBS_DIR}" -mindepth 1 -delete
fi
mkdir -p "${JNILIBS_DIR}"

NDK_ABI_ARGS=()
for t in "${TARGETS[@]}"; do
  NDK_ABI_ARGS+=(-t "$(abi_of "${t}")")
done

# `cargo ndk` shells out to `cargo metadata` in the CURRENT directory BEFORE it
# honors `--manifest-path`, so run it from the workspace root or it fails with
# "could not find Cargo.toml" when invoked from elsewhere (e.g. the repo root).
if [[ "${VARIANT}" == "direct" ]]; then
  ( cd "${CARGO_DIR}" && cargo ndk "${NDK_ABI_ARGS[@]}" -o "${JNILIBS_DIR}" \
      build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" \
      --features android-computer-use --"${PROFILE}" )
else
  ( cd "${CARGO_DIR}" && cargo ndk "${NDK_ABI_ARGS[@]}" -o "${JNILIBS_DIR}" \
      build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${CRATE}" \
      --"${PROFILE}" )
fi

# Verify every expected ABI `.so` landed.
for t in "${TARGETS[@]}"; do
  abi="$(abi_of "${t}")"
  so="${JNILIBS_DIR}/${abi}/${SONAME}"
  [[ -f "${so}" ]] || { echo "ERROR: ${SONAME} not produced for ${abi}: ${so}" >&2; exit 1; }
  log "  ${abi}/${SONAME} ($(du -h "${so}" | awk '{print $1}'))"
done

# ---------------------------------------------------------------------------
# 1b. Bundled shell binaries — mksh + toybox as lib*.so (P5a)
# ---------------------------------------------------------------------------
# The `platform-android-shellbin` build crate NDK-compiles mksh + toybox into
# free-standing ELF executables. P5 ships them inside the APK as
# jniLibs/<abi>/libmksh.so + libtoybox.so so that, with useLegacyPackaging=true
# (set in app/build.gradle.kts), the package manager EXTRACTS them into
# nativeLibraryDir as real executable files — the only place Android 10+ W^X
# permits `execve` of app-shipped binaries. These are gitignored build
# artifacts, exactly like libandroid_aar.so.
#
# The build crate has no `links` key, so the binary paths aren't exposed as
# DEP_ env vars. We locate them by globbing the per-target build-script OUT_DIR
# (target/<triple>/<profile>/build/platform-android-shellbin-*/out/{mksh,toybox})
# after a `cargo ndk build -p platform-android-shellbin` for that ABI.
SHELLBIN_CRATE="platform-android-shellbin"
case "$(uname -s)" in
  Darwin) NDK_HOST_TAG="darwin-x86_64" ;;
  Linux)  NDK_HOST_TAG="linux-x86_64" ;;
  *)      NDK_HOST_TAG="unknown" ;;
esac
LLVM_STRIP="${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/${NDK_HOST_TAG}/bin/llvm-strip"

log "Building ${SHELLBIN_CRATE} (mksh+toybox) per ABI and packaging as lib*.so…"
for t in "${TARGETS[@]}"; do
  abi="$(abi_of "${t}")"
  ( cd "${CARGO_DIR}" && cargo ndk -t "${abi}" \
      build --manifest-path "${CARGO_DIR}/Cargo.toml" -p "${SHELLBIN_CRATE}" --"${PROFILE}" )

  # Locate the two executables for this triple's build-script out dir. A clean
  # tree has exactly one platform-android-shellbin-* build dir per triple; if a
  # stale dir lingers, prefer the newest with both binaries present.
  build_glob="${CARGO_TARGET_DIR}/${t}/${PROFILE_DIR}/build/${SHELLBIN_CRATE}-*/out"
  mksh_src=""; toybox_src=""
  for d in $(ls -dt ${build_glob} 2>/dev/null); do
    if [[ -f "${d}/mksh" && -f "${d}/toybox" ]]; then
      mksh_src="${d}/mksh"; toybox_src="${d}/toybox"; break
    fi
  done
  [[ -n "${mksh_src}" ]] || { echo "ERROR: mksh/toybox not found under ${build_glob} for ${abi}" >&2; exit 1; }

  abi_dir="${JNILIBS_DIR}/${abi}"
  mkdir -p "${abi_dir}"
  cp -f "${mksh_src}"   "${abi_dir}/libmksh.so"
  cp -f "${toybox_src}" "${abi_dir}/libtoybox.so"
  # Strip to shrink the APK (unstripped debug ELFs are large). Best-effort: a
  # missing llvm-strip is non-fatal (the unstripped binaries still exec).
  if [[ -x "${LLVM_STRIP}" ]]; then
    "${LLVM_STRIP}" "${abi_dir}/libmksh.so" "${abi_dir}/libtoybox.so"
  else
    log "  WARN: llvm-strip not found at ${LLVM_STRIP}; shipping unstripped"
  fi
  log "  ${abi}/libmksh.so   ($(du -h "${abi_dir}/libmksh.so"   | awk '{print $1}'))"
  log "  ${abi}/libtoybox.so ($(du -h "${abi_dir}/libtoybox.so" | awk '{print $1}'))"
done

# ---------------------------------------------------------------------------
# 2. Generate Kotlin bindings (uniffi-bindgen --library, offline patched bin)
# ---------------------------------------------------------------------------
# `--library` introspection can read the UniFFI metadata sections out of any
# built library carrying them. We point it at the arm64 `.so` we just built (it
# carries the same metadata as the host dylib but needs no extra host build).
INTROSPECT_LIB="${JNILIBS_DIR}/$(abi_of aarch64-linux-android)/${SONAME}"

# The bindgen bin's `--library` metadata extractor expects to find a cdylib name
# it can compute; build the bin first (cli feature), then run it.
log "Building offline uniffi-bindgen bin (ios-framework --features cli)…"
cargo build --manifest-path "${CARGO_DIR}/Cargo.toml" -p ios-framework --features cli --bin uniffi-bindgen

# Clean only the generated bindings package subtree (KOTLIN_OUT also holds any
# hand-written app sources — never wipe the whole java/ root).
PKG_REL_PATH="com/lingxi/code/bindings"
GEN_PKG_DIR="${KOTLIN_OUT}/${PKG_REL_PATH}"
rm -rf "${GEN_PKG_DIR}"
mkdir -p "${KOTLIN_OUT}"

log "Generating Kotlin bindings → ${GEN_PKG_DIR} (package com.lingxi.code.bindings)…"
cargo run --manifest-path "${CARGO_DIR}/Cargo.toml" -p ios-framework --features cli \
  --bin uniffi-bindgen -- \
  generate \
  --library "${INTROSPECT_LIB}" \
  --language kotlin \
  --config "${UNIFFI_CONFIG}" \
  --out-dir "${KOTLIN_OUT}"

KT_COUNT="$(find "${KOTLIN_OUT}" -name '*.kt' -path "*${PKG_REL_PATH}*" 2>/dev/null | wc -l | tr -d ' ')"
[[ "${KT_COUNT}" -gt 0 ]] || { echo "ERROR: no Kotlin bindings generated under ${GEN_PKG_DIR}" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
for t in "${TARGETS[@]}"; do
  abi="$(abi_of "${t}")"
  mkdir -p "${FINAL_JNILIBS_DIR}/${abi}"
  cp -f \
    "${JNILIBS_DIR}/${abi}/${SONAME}" \
    "${JNILIBS_DIR}/${abi}/libmksh.so" \
    "${JNILIBS_DIR}/${abi}/libtoybox.so" \
    "${FINAL_JNILIBS_DIR}/${abi}/"
done

log "OK"
echo "variant        : ${VARIANT}"
echo "jniLibs        : ${FINAL_JNILIBS_DIR}"
echo "Kotlin bindings: ${GEN_PKG_DIR} (${KT_COUNT} .kt file(s))"
