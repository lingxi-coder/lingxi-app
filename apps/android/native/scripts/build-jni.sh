#!/usr/bin/env bash
#
# T2.4 — build-jni.sh  (Android counterpart of apps/ios/native/scripts/build-xcframework.sh)
#
# Reproduces the GITIGNORED Android integration artifacts the Gradle project links
# against, so a fresh checkout can build the app:
#
#   1. cargo-ndk cross-compiles the `android-aar` crate's `cdylib`
#      (`libandroid_aar.so`) for every target ABI into
#        apps/android/native/app/src/<play|direct>/jniLibs/<abi>/libandroid_aar.so
#      ABIs: arm64-v8a (aarch64-linux-android) + x86_64 (x86_64-linux-android).
#   2. Generates the Kotlin UniFFI bindings from the built `.so` in `--library`
#      mode into
#        apps/android/native/app/src/main/java/  (package com.lingxi.code.bindings)
#      using apps/android/ffi/uniffi.toml for the package name.
#
# Swift and Kotlin share the pinned UniFFI 0.32 public generation pipeline.
# Each Kotlin component has a configured package and native cross-component converters.
#
# Idempotent: generated artifacts are staged before replacement. Prints output
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
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"                  # apps/android/native
REPO_ROOT="$(cd "${ANDROID_DIR}/../../.." && pwd)"               # worktree root
CARGO_DIR="${REPO_ROOT}"                          # Rust workspace
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${CARGO_DIR}/target}"

CRATE="android-aar"
LIB_STEM="android_aar"                # cargo turns the `-` into `_`
SONAME="lib${LIB_STEM}.so"
HOST_DYLIB="lib${LIB_STEM}.dylib"     # macOS host cdylib (bindgen introspection)

UNIFFI_CONFIG="${CARGO_DIR}/apps/android/ffi/uniffi.toml"
VOICE_MANIFEST="${REPO_ROOT}/resources/voice/models.json"

[[ -f "${VOICE_MANIFEST}" ]] || {
  echo "ERROR: shared voice manifest is missing: ${VOICE_MANIFEST}" >&2
  exit 1
}

# Stage every ABI and Kotlin bindings before replacing owned artifacts. Each
# destination uses a sibling rename and retains its previous value for rollback;
# this is not a globally atomic replacement of multiple directories.
# Optional output roots keep a complete JNI/bindings refresh isolated from
# concurrent Gradle builds until the caller promotes the verified pair.
FINAL_JNILIBS_DIR="${LINGXI_ANDROID_JNILIBS_DIR:-${ANDROID_DIR}/app/src/${VARIANT}/jniLibs}"
STAGING_PARENT="${ANDROID_DIR}/app/build/nativeStaging/${VARIANT}"
KOTLIN_OUT="${LINGXI_KOTLIN_OUT:-${ANDROID_DIR}/app/src/main/java}"   # bindgen writes <pkg-path>/*.kt under here

PROFILE="release"

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

# These arrays own only the generated package and libandroid_aar.so per ABI.
# Never move an entire Java or jniLibs root: it includes handwritten sources and
# native support libraries published by other build steps.
PROMOTION_COUNT=0
PROMOTION_TARGETS=()
PROMOTION_DIRS=()
PROMOTION_ORIGINALS=()
PROMOTION_APPLIED=()
PROMOTION_KEEP=()
PROMOTION_COMMITTED=false
LOCK_COUNT=0
PROMOTION_LOCKS=()

finish_staging() {
  local status=$? index
  trap - EXIT HUP INT TERM
  set +e
  if [[ "${PROMOTION_COMMITTED}" == false ]]; then
    index=$((PROMOTION_COUNT - 1))
    while [[ ${index} -ge 0 ]]; do
      if [[ "${PROMOTION_ORIGINALS[index]}" == 1 ]]; then
        if [[ -e "${PROMOTION_DIRS[index]}/old" || -L "${PROMOTION_DIRS[index]}/old" ]]; then
          if ! rm -rf "${PROMOTION_TARGETS[index]}" \
            || ! mv "${PROMOTION_DIRS[index]}/old" "${PROMOTION_TARGETS[index]}"; then
            PROMOTION_KEEP[index]=1
            echo "ERROR: rollback could not restore ${PROMOTION_TARGETS[index]}; previous artifact retained at ${PROMOTION_DIRS[index]}/old" >&2
            status=1
          fi
        elif [[ "${PROMOTION_APPLIED[index]}" == 1 ]]; then
          PROMOTION_KEEP[index]=1
          echo "ERROR: rollback backup is missing for ${PROMOTION_TARGETS[index]}" >&2
          status=1
        fi
      elif [[ "${PROMOTION_APPLIED[index]}" == 1 ]]; then
        if ! rm -rf "${PROMOTION_TARGETS[index]}"; then
          PROMOTION_KEEP[index]=1
          echo "ERROR: rollback could not remove ${PROMOTION_TARGETS[index]}" >&2
          status=1
        fi
      fi
      index=$((index - 1))
    done
  fi
  index=0
  while [[ ${index} -lt ${PROMOTION_COUNT} ]]; do
    if [[ "${PROMOTION_KEEP[index]}" == 0 ]]; then
      rm -rf "${PROMOTION_DIRS[index]}" || echo "WARN: could not remove staging directory ${PROMOTION_DIRS[index]}" >&2
    fi
    index=$((index + 1))
  done
  index=0
  while [[ ${index} -lt ${LOCK_COUNT} ]]; do
    rmdir "${PROMOTION_LOCKS[index]}" || echo "WARN: could not release JNI promotion lock ${PROMOTION_LOCKS[index]}" >&2
    index=$((index + 1))
  done
  rm -rf "${STAGING_ROOT}" || echo "WARN: could not remove build staging directory ${STAGING_ROOT}" >&2
  exit "${status}"
}

prepare_artifact() {
  local source="$1" target="$2" temporary index=${PROMOTION_COUNT}
  mkdir -p "$(dirname "${target}")"
  temporary="$(mktemp -d "$(dirname "${target}")/.jni-stage.XXXXXX")"
  PROMOTION_TARGETS[index]="${target}"
  PROMOTION_DIRS[index]="${temporary}"
  PROMOTION_ORIGINALS[index]=0
  PROMOTION_APPLIED[index]=0
  PROMOTION_KEEP[index]=0
  PROMOTION_COUNT=$((PROMOTION_COUNT + 1))
  cp -R "${source}" "${temporary}/new"
}

lock_promotion_root() {
  local root="$1" lock="$1/.jni-promotion.lock" index=0
  while [[ ${index} -lt ${LOCK_COUNT} ]]; do
    [[ "${PROMOTION_LOCKS[index]}" != "${lock}" ]] || return 0
    index=$((index + 1))
  done
  mkdir -p "${root}"
  mkdir "${lock}" || { echo "ERROR: JNI output publication is locked: ${lock}" >&2; return 1; }
  PROMOTION_LOCKS[LOCK_COUNT]="${lock}"
  LOCK_COUNT=$((LOCK_COUNT + 1))
}

# ---------------------------------------------------------------------------
# 0. Preflight — tools, NDK, Rust targets
# ---------------------------------------------------------------------------
for tool in cargo rustc cargo-ndk node shasum; do
  command -v "${tool}" >/dev/null 2>&1 || { echo "ERROR: required tool not found: ${tool}" >&2; exit 1; }
done

# Read via a herestring, NOT `< <(...)`. The node program ends its output with
# `process.stdout.write`, so there is no trailing newline; `read` then hits EOF
# without a delimiter and returns 1 even though it assigned every variable. Under
# this script's `set -e` that killed the build right here, silently — no message,
# no partial output, just exit 1, which reads like the toolchain is missing. A
# herestring appends the newline, and keeping the command substitution in its own
# assignment means a genuine node failure still trips `set -e` instead of being
# swallowed the way `|| true` would swallow it.
SHERPA_AAR_META="$(
  node -e '
    const fs = require("fs");
    const manifest = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    const artifact = manifest.runtime.android;
    process.stdout.write([manifest.runtime.version, artifact.name, artifact.url, artifact.sha256].join("\t"));
  ' "${VOICE_MANIFEST}"
)"
IFS=$'\t' read -r SHERPA_AAR_VER SHERPA_AAR_NAME SHERPA_AAR_URL SHERPA_AAR_SHA256 <<<"${SHERPA_AAR_META}"
[[ -n "${SHERPA_AAR_VER}" && -n "${SHERPA_AAR_NAME}" && -n "${SHERPA_AAR_URL}" && -n "${SHERPA_AAR_SHA256}" ]] \
  || { echo "ERROR: ${VOICE_MANIFEST} did not yield a complete runtime.android artifact" >&2; exit 1; }

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
  # Keep host bindgen on the workspace toolchain even when called outside it;
  # --manifest-path alone does not select rustup's directory override.
  export RUSTUP_TOOLCHAIN="${ACTIVE_TOOLCHAIN}"
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
#     from the shared voice manifest into app/libs/ (consumed via the flatDir
#     repo in settings.gradle.kts). ~38 MB; skipped if already present after
#     checksum verification.
# ---------------------------------------------------------------------------
SHERPA_AAR="${ANDROID_DIR}/app/libs/${SHERPA_AAR_NAME}"
# A failed transfer/checksum must never become the next invocation's cache.
# The subshell owns its temporary download and cleanup independently from the
# later JNI/bindings promotion transaction.
(
  SHERPA_AAR_TEMP=""
  trap '[[ -z "${SHERPA_AAR_TEMP}" ]] || rm -f "${SHERPA_AAR_TEMP}"' EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
  mkdir -p "$(dirname "${SHERPA_AAR}")"
  if [[ -f "${SHERPA_AAR}" ]]; then
    ACTUAL_SHERPA_AAR_SHA256="$(shasum -a 256 "${SHERPA_AAR}" | awk '{print $1}')"
    if [[ "${ACTUAL_SHERPA_AAR_SHA256}" == "${SHERPA_AAR_SHA256}" ]]; then
      log "sherpa-onnx AAR present: ${SHERPA_AAR}"
      exit 0
    fi
    log "Discarding invalid sherpa-onnx AAR cache: ${SHERPA_AAR}"
    rm -f "${SHERPA_AAR}"
  fi
  log "Downloading sherpa-onnx AAR v${SHERPA_AAR_VER} → ${SHERPA_AAR}"
  SHERPA_AAR_TEMP="$(mktemp "$(dirname "${SHERPA_AAR}")/.sherpa-aar-download.XXXXXX")"
  curl -fsSL "${SHERPA_AAR_URL}" -o "${SHERPA_AAR_TEMP}" \
    || { echo "ERROR: failed to fetch sherpa-onnx AAR" >&2; exit 1; }
  ACTUAL_SHERPA_AAR_SHA256="$(shasum -a 256 "${SHERPA_AAR_TEMP}" | awk '{print $1}')"
  if [[ "${ACTUAL_SHERPA_AAR_SHA256}" != "${SHERPA_AAR_SHA256}" ]]; then
    echo "ERROR: sherpa-onnx AAR checksum mismatch" >&2
    echo "expected: ${SHERPA_AAR_SHA256}" >&2
    echo "actual:   ${ACTUAL_SHERPA_AAR_SHA256}" >&2
    exit 1
  fi
  mv -f "${SHERPA_AAR_TEMP}" "${SHERPA_AAR}"
  SHERPA_AAR_TEMP=""
  log "  $(du -h "${SHERPA_AAR}" | awk '{print $1}')"
)

# ---------------------------------------------------------------------------
# 1. cargo-ndk cross-compile the cdylib into jniLibs/<abi>/
# ---------------------------------------------------------------------------
# `cargo ndk -t <abi> -o <jniLibs>` places each built `.so` under
# <jniLibs>/<abi>/. We pass the Gradle ABI names; cargo-ndk maps them to triples.
log "Cross-compiling ${CRATE} cdylib (${PROFILE}) for: ${TARGETS[*]/#/}"
mkdir -p "${STAGING_PARENT}"
STAGING_ROOT="$(mktemp -d "${STAGING_PARENT}/build.XXXXXX")"
JNILIBS_DIR="${STAGING_ROOT}/jniLibs"
BINDINGS_STAGE="${STAGING_ROOT}/kotlin"
trap finish_staging EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
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

# Native shell/PRoot/PTY support is built and verified by the Cargo-locked SDK
# through build-mobile-linux-native.sh. This script owns only LingXi's Rust FFI.

# ---------------------------------------------------------------------------
# 2. Generate Kotlin bindings (uniffi-bindgen --library, offline patched bin)
# ---------------------------------------------------------------------------
# `--library` introspection needs the UNSTRIPPED cdylib: llvm-strip drops the
# custom UniFFI metadata section, which makes bindgen fail with
# "no UniFFI metadata groups found in the library". Point bindgen at this
# invocation's unstripped staging copy and strip it only for APK packaging.
# Bindings must describe the library this invocation will publish. A shared
# Cargo target directory may be rebuilt by another checkout while bindgen runs.
INTROSPECT_LIB="${JNILIBS_DIR}/arm64-v8a/${SONAME}"

# The bindgen bin's `--library` metadata extractor expects to find a cdylib name
# it can compute; build the bin first (cli feature), then run it.
log "Building offline uniffi-bindgen bin (ios-framework --features cli)…"
cargo build --locked --manifest-path "${CARGO_DIR}/Cargo.toml" -p ios-framework --features cli --bin uniffi-bindgen

# Generate into this invocation's staging tree. The existing Kotlin package
# remains usable even if bindgen writes partial files and then fails.
PKG_REL_PATH="com/lingxi/code/bindings"
GEN_PKG_DIR="${KOTLIN_OUT}/${PKG_REL_PATH}"
mkdir -p "${BINDINGS_STAGE}"

log "Generating Kotlin bindings → ${GEN_PKG_DIR} (package com.lingxi.code.bindings)…"
cargo run --locked --manifest-path "${CARGO_DIR}/Cargo.toml" -p ios-framework --features cli \
  --bin uniffi-bindgen -- \
  generate \
  --library "${INTROSPECT_LIB}" \
  --language kotlin \
  --config "${UNIFFI_CONFIG}" \
  --out-dir "${BINDINGS_STAGE}"

KT_COUNT="$(find "${BINDINGS_STAGE}/${PKG_REL_PATH}" -type f -name '*.kt' 2>/dev/null | wc -l | tr -d ' ')"
[[ "${KT_COUNT}" -gt 0 ]] || { echo "ERROR: no Kotlin bindings generated under ${GEN_PKG_DIR}" >&2; exit 1; }

# UniFFI metadata lives in linker sections that llvm-strip removes. Keep the
# staging libraries intact until bindgen has inspected one of them, then strip
# only the runtime payloads immediately before copying them into the APK tree.
if [[ "${PROFILE}" == "release" ]]; then
  case "$(uname -s)" in
    Darwin) _NDK_HOST_TAG="darwin-x86_64" ;;
    Linux) _NDK_HOST_TAG="linux-x86_64" ;;
    *) _NDK_HOST_TAG="unknown" ;;
  esac
  _LLVM_STRIP="${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/${_NDK_HOST_TAG}/bin/llvm-strip"
  if [[ -x "${_LLVM_STRIP}" ]]; then
    for t in "${TARGETS[@]}"; do
      abi="$(abi_of "${t}")"
      so="${JNILIBS_DIR}/${abi}/${SONAME}"
      "${_LLVM_STRIP}" "${so}" || log "  WARN: llvm-strip failed for ${so}"
    done
  fi
fi

# ---------------------------------------------------------------------------
# Prepare all same-filesystem replacements before touching a live artifact.
# Serialize publishers sharing either output root so rollback cannot overwrite
# another invocation's successfully published pair. Gradle readers should use
# isolated output roots when they need to avoid the brief promotion window.
# ---------------------------------------------------------------------------
lock_promotion_root "${KOTLIN_OUT}"
lock_promotion_root "${FINAL_JNILIBS_DIR}"
prepare_artifact "${BINDINGS_STAGE}/${PKG_REL_PATH}" "${GEN_PKG_DIR}"
for t in "${TARGETS[@]}"; do
  abi="$(abi_of "${t}")"
  prepare_artifact "${JNILIBS_DIR}/${abi}/${SONAME}" "${FINAL_JNILIBS_DIR}/${abi}/${SONAME}"
done

index=0
while [[ ${index} -lt ${PROMOTION_COUNT} ]]; do
  if [[ -e "${PROMOTION_TARGETS[index]}" || -L "${PROMOTION_TARGETS[index]}" ]]; then
    # Record intent before invoking mv so a command interrupted after its
    # rename still restores the backup. A failed pre-rename leaves no backup
    # and the original destination intact.
    PROMOTION_ORIGINALS[index]=1
    mv "${PROMOTION_TARGETS[index]}" "${PROMOTION_DIRS[index]}/old"
  fi
  PROMOTION_APPLIED[index]=1
  mv "${PROMOTION_DIRS[index]}/new" "${PROMOTION_TARGETS[index]}"
  index=$((index + 1))
done
PROMOTION_COMMITTED=true

log "OK"
echo "variant        : ${VARIANT}"
echo "jniLibs        : ${FINAL_JNILIBS_DIR}"
echo "Kotlin bindings: ${GEN_PKG_DIR} (${KT_COUNT} .kt file(s))"
