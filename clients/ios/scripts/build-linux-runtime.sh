#!/usr/bin/env bash
#
# Build and stage the pinned OpenMinis iSH ARM64 runtime artifacts used by the
# native iOS mobile-linux bridge.
#
# This script intentionally stages generated artifacts under `clients/ios/build/`
# only. Nothing under the stage directory should be committed.
#
# Inputs (pinned in-repo):
#   - docs/superpowers/references/OpenMinis/deps/build_ish.sh
#   - docs/superpowers/references/OpenMinis/deps/prepare_alpine_rootfs.sh
#
# Outputs:
#   clients/ios/build/linux-runtime/openminis/
#     include/
#     libs/
#     resources/{libvdso.so.elf,alpine-rootfs,alpine-rootfs.zip,default_mount}
#     xcode/openminis-ish.xcconfig
#     manifest.json
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${IOS_DIR}/../.." && pwd)"
OPENMINIS_ROOT="${REPO_ROOT}/docs/superpowers/references/OpenMinis"
DEPS_DIR="${OPENMINIS_ROOT}/deps"

BUILD_ISH="${DEPS_DIR}/build_ish.sh"
PREPARE_ROOTFS="${DEPS_DIR}/prepare_alpine_rootfs.sh"
ISH_SOURCE="${DEPS_DIR}/ish"
ISH_NETWORK_POLICY_PATCH="${IOS_DIR}/Sources/LinuxRuntimeNative/patches/ish-socket-network-policy.patch"

STAGE_ROOT="${IOS_DIR}/build/linux-runtime/openminis"
STAGE_INCLUDE="${STAGE_ROOT}/include"
STAGE_LIBS="${STAGE_ROOT}/libs"
STAGE_RESOURCES="${STAGE_ROOT}/resources"
STAGE_XCODE="${STAGE_ROOT}/xcode"

BUILD_TYPE="release"
# Default branch for the legacy (non-local-app) path only. The local-app rootfs
# takes its exact Alpine release from the pins, via build-local-app-rootfs.sh.
ALPINE_VERSION="3.21"
CLEAN=0
LOCAL_APP_RUNTIME=0
APK_DIR=""

usage() {
  cat <<'EOF'
Usage: clients/ios/scripts/build-linux-runtime.sh [options]

Options:
  --clean                 Remove the staged output before rebuilding
  --debug                 Use OpenMinis debug iSH build artifacts
  --release               Use OpenMinis release iSH build artifacts (default)
  --alpine-version <ver>  Pass a version to prepare_alpine_rootfs.sh (default: 3.21)
  --local-app-runtime     Require the exact Node/Git local-app rootfs contract
  --apk-dir <dir>         Hashed recursive APK closure for --local-app-runtime
  -h, --help              Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --clean)
      CLEAN=1
      shift
      ;;
    --debug)
      BUILD_TYPE="debug"
      shift
      ;;
    --release)
      BUILD_TYPE="release"
      shift
      ;;
    --alpine-version)
      ALPINE_VERSION="${2:?missing value for --alpine-version}"
      shift 2
      ;;
    --local-app-runtime)
      LOCAL_APP_RUNTIME=1
      shift
      ;;
    --apk-dir)
      APK_DIR="${2:?missing value for --apk-dir}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

LOCAL_APP_ROOTFS_DIR="${IOS_DIR}/build/local-app-rootfs"
LOCAL_APP_TARBALL=""

# Refuse to silently replace a staged local-app runtime (Node/npm/git/python)
# with the legacy bare minirootfs. The two write to the same staging directory,
# so whichever ran last used to win -- and a plain `build-xcframework.sh` run
# after a local-app build shipped an app whose rootfs had no toolchain at all,
# with nothing in the log to say so.
STAGED_MANIFEST="${STAGE_ROOT}/manifest.json"
if [[ "${LOCAL_APP_RUNTIME}" != "1" && "${CLEAN}" != "1" && -f "${STAGED_MANIFEST}" ]]; then
  if grep -q '"local_app_runtime": true' "${STAGED_MANIFEST}" 2>/dev/null; then
    echo "error: a local-app runtime is already staged at ${STAGE_ROOT}." >&2
    echo "       Re-run with --local-app-runtime, or pass --clean to replace it" >&2
    echo "       with the legacy bare minirootfs on purpose." >&2
    exit 1
  fi
fi

if [[ "${LOCAL_APP_RUNTIME}" == "1" ]]; then
  # Build the rootfs first: build-local-app-rootfs.sh resolves the recursive APK
  # closure, checks every artifact against the pins, and installs the closure
  # OFFLINE into a digest-verified minirootfs. `--apk-dir` used to be the only
  # input here, and it was passed to the verifier alone -- nothing ever
  # installed those packages, which is why the shipped rootfs had no Node.
  echo "[build-linux-runtime] Building local-app Alpine rootfs (aarch64)"
  bash "${REPO_ROOT}/lingxi-code/scripts/mobile-linux/build-local-app-rootfs.sh" \
    --arch aarch64 \
    --output "${LOCAL_APP_ROOTFS_DIR}"
  LOCAL_APP_TARBALL="${LOCAL_APP_ROOTFS_DIR}/aarch64/rootfs.tar.gz"
  [[ -f "${LOCAL_APP_TARBALL}" ]] || {
    echo "local-app rootfs build produced no tarball: ${LOCAL_APP_TARBALL}" >&2
    exit 1
  }
  if [[ -n "${APK_DIR}" ]]; then
    # An explicit --apk-dir is a request for the full release check against that
    # closure, so run it. It will refuse while any supported ABI is short of
    # release-ready -- which is the point of asking.
    bash "${SCRIPT_DIR}/verify-local-app-supply-chain.sh" --release --apk-dir "${APK_DIR}"
  else
    # Deliberately NOT --release by default: that gate demands every supported
    # ABI be release-ready, and iOS ships arm64 only. The arm64 closure was
    # already verified artifact-by-artifact against the pins by the builder
    # above. `package-rootfs-release.sh` remains the step that requires
    # --release for an actual release.
    bash "${SCRIPT_DIR}/verify-local-app-supply-chain.sh"
  fi
else
  bash "${SCRIPT_DIR}/verify-local-app-supply-chain.sh"
fi

for required in "$BUILD_ISH" "$PREPARE_ROOTFS"; do
  [[ -x "$required" || -f "$required" ]] || {
    echo "Missing pinned OpenMinis helper: $required" >&2
    exit 1
  }
done

if [[ "$CLEAN" == "1" ]]; then
  rm -rf "${STAGE_ROOT}"
fi

mkdir -p "${STAGE_INCLUDE}" "${STAGE_LIBS}" "${STAGE_RESOURCES}" "${STAGE_XCODE}"

echo "[build-linux-runtime] Building pinned OpenMinis iSH (${BUILD_TYPE})"
[[ -f "${ISH_NETWORK_POLICY_PATCH}" ]] || {
  echo "iSH network-policy patch is missing: ${ISH_NETWORK_POLICY_PATCH}" >&2
  exit 1
}
# Initialize nested iSH submodules before applying the temporary policy patch.
# `build_ish.sh` performs this check itself, but doing it first is important:
# `git submodule update --init --recursive` may refresh the parent checkout when
# nested modules are missing, which would silently discard a patch applied just
# before entering that helper.
git -C "${ISH_SOURCE}" submodule update --init --recursive
# This patch is intentionally generated with zero context so it remains small
# beside the pinned iSH snapshot. Tell git to honor the exact hunk line numbers;
# without this flag git may relocate insertion-only hunks to EOF and leave the
# source uncompilable.
git -C "${ISH_SOURCE}" apply --unidiff-zero --check "${ISH_NETWORK_POLICY_PATCH}"
git -C "${ISH_SOURCE}" apply --unidiff-zero "${ISH_NETWORK_POLICY_PATCH}"
ISH_POLICY_PATCH_APPLIED=1
restore_ish_policy_source() {
  if [[ "${ISH_POLICY_PATCH_APPLIED:-0}" == "1" ]]; then
    if ! git -C "${ISH_SOURCE}" apply --reverse --unidiff-zero "${ISH_NETWORK_POLICY_PATCH}"; then
      echo "failed to restore pinned iSH source after policy build" >&2
      return 1
    fi
    ISH_POLICY_PATCH_APPLIED=0
  fi
}
on_ish_policy_signal() {
  local status="$1"
  restore_ish_policy_source || status=1
  exit "${status}"
}
trap restore_ish_policy_source EXIT
trap 'on_ish_policy_signal 130' INT
trap 'on_ish_policy_signal 143' TERM
bash "${BUILD_ISH}" "${BUILD_TYPE}"
restore_ish_policy_source
# Prove the pinned checkout is back at the pre-build state. A failed or partial
# reverse must stop staging instead of silently shipping from a dirty submodule.
git -C "${ISH_SOURCE}" apply --unidiff-zero --check "${ISH_NETWORK_POLICY_PATCH}"
trap - EXIT INT TERM

FAKEFSIFY="${DEPS_DIR}/ish/build-native/tools/fakefsify"
ROOTFS_SOURCE_DIR="${DEPS_DIR}/resources/alpine-rootfs"

if [[ "${LOCAL_APP_RUNTIME}" == "1" ]]; then
  # The vendored prepare script only ever extracts a bare minirootfs and
  # fakefsifies it -- it installs nothing. Reuse only its fakefsify build, and
  # convert the rootfs we assembled instead. Running it at all is just how
  # fakefsify gets built without editing the pinned snapshot.
  if [[ ! -x "${FAKEFSIFY}" ]]; then
    echo "[build-linux-runtime] Building fakefsify via the pinned OpenMinis helper"
    bash "${PREPARE_ROOTFS}" "${ALPINE_VERSION}"
  fi
  [[ -x "${FAKEFSIFY}" ]] || {
    echo "fakefsify was not built: ${FAKEFSIFY}" >&2
    exit 1
  }

  ROOTFS_SOURCE_DIR="${LOCAL_APP_ROOTFS_DIR}/alpine-rootfs"
  echo "[build-linux-runtime] Converting the local-app rootfs to fakefs"
  rm -rf "${ROOTFS_SOURCE_DIR}"
  "${FAKEFSIFY}" "${LOCAL_APP_TARBALL}" "${ROOTFS_SOURCE_DIR}"
  [[ -d "${ROOTFS_SOURCE_DIR}/data" && -f "${ROOTFS_SOURCE_DIR}/meta.db" ]] || {
    echo "fakefsify produced no usable rootfs at ${ROOTFS_SOURCE_DIR}" >&2
    exit 1
  }

  # verify-tree reads host filesystem semantics -- symlink vs hardlink, modes,
  # inode identity. A fakefsified `data/` directory cannot answer those: fakefs
  # stores a symlink as a regular file whose contents are the target, and keeps
  # the real mode in meta.db. Verifying the tree as it exists BEFORE conversion
  # is what actually checks the rootfs; pointing this at `data/` (as it used to)
  # inspects fakefs's on-disk encoding instead of the filesystem it encodes.
  echo "[build-linux-runtime] Verifying exact local-app runtime packages"
  VERIFY_TREE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/lingxi-rootfs-verify.XXXXXX")"
  trap 'chmod -R u+w "${VERIFY_TREE_DIR}" 2>/dev/null || true; rm -rf "${VERIFY_TREE_DIR}"' EXIT
  tar -xzf "${LOCAL_APP_TARBALL}" -C "${VERIFY_TREE_DIR}"
  python3 "${REPO_ROOT}/lingxi-code/scripts/mobile-linux/rootfs_tool.py" verify-tree \
    --root "${VERIFY_TREE_DIR}"
else
  echo "[build-linux-runtime] Preparing pinned Alpine rootfs (${ALPINE_VERSION})"
  bash "${PREPARE_ROOTFS}" "${ALPINE_VERSION}"
fi

echo "[build-linux-runtime] Staging headers, static libs, and resources"
rsync -a --delete "${DEPS_DIR}/include/" "${STAGE_INCLUDE}/"
rsync -a --delete "${DEPS_DIR}/libs/" "${STAGE_LIBS}/"

# OpenMinis currently exports four ARM64 gadget names from both bits.S and
# math.S. The generator targets the parameter-aware math.S implementations, so
# localize only the redundant definitions in the bits.S archive member before
# Xcode links the staged library. Keep the pinned source checkout untouched.
ISH_EMU_ARCHIVE="${STAGE_LIBS}/libish_emu.a"
if [[ -f "${ISH_EMU_ARCHIVE}" ]]; then
  ARCHIVE_FIX_DIR="$(mktemp -d "${TMPDIR:-/tmp}/lingxi-ish-archive.XXXXXX")"
  (
    cd "${ARCHIVE_FIX_DIR}"
    ar -x "${ISH_EMU_ARCHIVE}"
    printf '%s\n' \
      _gadget_sxtw \
      _gadget_uxtb \
      _gadget_uxth \
      _gadget_rev32 > duplicate-gadgets.txt
    xcrun nmedit -R duplicate-gadgets.txt \
      asbestos_guest-arm64_gadgets-aarch64_bits.S.o
    rm duplicate-gadgets.txt
    xcrun libtool -static -o "${ISH_EMU_ARCHIVE}.fixed" ./*.o
  )
  mv "${ISH_EMU_ARCHIVE}.fixed" "${ISH_EMU_ARCHIVE}"
  rm -R "${ARCHIVE_FIX_DIR}"
fi

mkdir -p "${STAGE_RESOURCES}/default_mount"
if [[ -f "${DEPS_DIR}/resources/libvdso.so.elf" ]]; then
  cp "${DEPS_DIR}/resources/libvdso.so.elf" "${STAGE_RESOURCES}/libvdso.so.elf"
fi
if [[ -d "${ROOTFS_SOURCE_DIR}" ]]; then
  rsync -a --delete "${ROOTFS_SOURCE_DIR}/" "${STAGE_RESOURCES}/alpine-rootfs/"
  rm -f "${STAGE_RESOURCES}/alpine-rootfs.zip"
  (
    cd "${STAGE_RESOURCES}"
    ditto -c -k --sequesterRsrc --keepParent "alpine-rootfs" "alpine-rootfs.zip"
  )
fi
if [[ -d "${OPENMINIS_ROOT}/src/ios/default_mount" ]]; then
  rsync -a --delete "${OPENMINIS_ROOT}/src/ios/default_mount/" "${STAGE_RESOURCES}/default_mount/"
fi

MANIFEST="${STAGE_ROOT}/manifest.json"
# The manifest must report the release actually shipped. For the local-app path
# that is the exact pinned release (e.g. 3.24.1), not the branch default the
# legacy path passes to the vendored helper.
if [[ "${LOCAL_APP_RUNTIME}" == "1" ]]; then
  ALPINE_VERSION="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["alpine"]["version"])' \
    "${REPO_ROOT}/docs/mobile-linux/local-app-runtime-pins.json")"
fi
if [[ "${LOCAL_APP_RUNTIME}" == "1" ]]; then LOCAL_APP_RUNTIME_JSON=true; else LOCAL_APP_RUNTIME_JSON=false; fi
ROOTFS_SHA=""
if [[ -f "${STAGE_RESOURCES}/alpine-rootfs.zip" ]]; then
  ROOTFS_SHA="$(shasum -a 256 "${STAGE_RESOURCES}/alpine-rootfs.zip" | awk '{print $1}')"
fi

cat > "${STAGE_XCODE}/openminis-ish.xcconfig" <<EOF
// Generated by clients/ios/scripts/build-linux-runtime.sh
HEADER_SEARCH_PATHS = \$(inherited) ${STAGE_INCLUDE} ${STAGE_INCLUDE}/ish
LIBRARY_SEARCH_PATHS = \$(inherited) ${STAGE_LIBS}
OTHER_CFLAGS = \$(inherited) -DLINGXI_ENABLE_OPENMINIS_ISH=1
OTHER_LDFLAGS = \$(inherited) ${STAGE_LIBS}/libish.a ${STAGE_LIBS}/libish_emu.a ${STAGE_LIBS}/libfakefs.a
EOF

cat > "${MANIFEST}" <<EOF
{
  "generated_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "source_root": "${OPENMINIS_ROOT}",
  "build_type": "${BUILD_TYPE}",
  "alpine_version": "${ALPINE_VERSION}",
  "local_app_runtime": ${LOCAL_APP_RUNTIME_JSON},
  "stage_root": "${STAGE_ROOT}",
  "rootfs_zip_sha256": "${ROOTFS_SHA}",
  "link_xcconfig": "${STAGE_XCODE}/openminis-ish.xcconfig",
  "resources": {
    "rootfs_zip": "${STAGE_RESOURCES}/alpine-rootfs.zip",
    "rootfs_dir": "${STAGE_RESOURCES}/alpine-rootfs",
    "default_mount": "${STAGE_RESOURCES}/default_mount",
    "vdso": "${STAGE_RESOURCES}/libvdso.so.elf"
  }
}
EOF

echo "[build-linux-runtime] Stage complete"
echo "  root:      ${STAGE_ROOT}"
echo "  xcconfig:  ${STAGE_XCODE}/openminis-ish.xcconfig"
echo "  manifest:  ${MANIFEST}"
if [[ -n "${ROOTFS_SHA}" ]]; then
  echo "  rootfs sha256: ${ROOTFS_SHA}"
fi
