#!/usr/bin/env bash
#
# Build the local-app Alpine rootfs for one architecture, with every version
# read from docs/mobile-linux/local-app-runtime-pins.json.
#
# This is the step the vendored OpenMinis `prepare_alpine_rootfs.sh` never had:
# it installs the committed, hashed APK closure OFFLINE into a digest-verified
# minirootfs. The output tarball is what should be handed to
# `fakefsify`, in place of the bare minirootfs.
#
# Usage:
#   build-local-app-rootfs.sh --arch <aarch64|x86_64> [--output <dir>]
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
PINS="${REPO_ROOT}/docs/mobile-linux/local-app-runtime-pins.json"
INNER="${SCRIPT_DIR}/rootfs-build-inner.sh"

# Digest-pinned so a moving `alpine:3.24` tag cannot change the builder itself.
# These are per-architecture manifest digests, not the index digest: passing a
# single arch's digest with --platform for the other arch does NOT cross-build,
# it silently hands back the pinned image and warns, so the "x86_64" build would
# run an arm64 builder while claiming to have produced an x86_64 rootfs.
BUILDER_IMAGE_AARCH64="docker.io/library/alpine:3.24@sha256:e7a1a92a5bfeee40966aea60f0796b0e7917cc35591542701834f03a68fa3d18"
BUILDER_IMAGE_X86_64="docker.io/library/alpine:3.24@sha256:79ff19e9084a00eece421b2523fb93e22d730e2c0e525905de047e848e56d95f"

ARCH=""
OUTPUT=""
CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-}"

usage() { sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "${ARCH}" in
  aarch64) PLATFORM="linux/arm64"; BUILDER_IMAGE="${BUILDER_IMAGE_AARCH64}" ;;
  x86_64) PLATFORM="linux/amd64"; BUILDER_IMAGE="${BUILDER_IMAGE_X86_64}" ;;
  *) echo "--arch must be aarch64 or x86_64" >&2; exit 2 ;;
esac
[[ -n "${OUTPUT}" ]] || OUTPUT="${REPO_ROOT}/clients/ios/build/local-app-rootfs"
[[ -f "${PINS}" ]] || { echo "missing pins: ${PINS}" >&2; exit 1; }

if [[ -z "${CONTAINER_RUNTIME}" ]]; then
  if command -v podman >/dev/null 2>&1; then CONTAINER_RUNTIME=podman
  elif command -v docker >/dev/null 2>&1; then CONTAINER_RUNTIME=docker
  else echo "neither podman nor docker is available" >&2; exit 1; fi
fi

# Every version below comes from the pins. Nothing in this script may introduce
# a second copy — that divergence is exactly what took the release gate red.
read -r ALPINE_VERSION ALPINE_BRANCH ROOTFS_SHA PACKAGES PNPM_VERSION PNPM_URL PNPM_SHA512 < <(
  python3 - "${PINS}" "${ARCH}" <<'PY'
import json, pathlib, sys

pins = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
arch = sys.argv[2]
alpine = pins["alpine"]
minirootfs = alpine.get("minirootfs", {}).get(arch)
if not minirootfs:
    raise SystemExit(f"pins carry no minirootfs digest for {arch}")
packages = " ".join(
    f"{name}={version}" for name, version in sorted(pins["runtime_packages"].items())
)
pnpm = pins["pnpm"]
print(alpine["version"], alpine["branch"], minirootfs["sha256"], packages,
      pnpm["version"], pnpm["url"], pnpm["sha512"])
PY
)

echo "[rootfs] alpine ${ALPINE_VERSION} (${ALPINE_BRANCH}) arch=${ARCH}"
echo "[rootfs] packages: ${PACKAGES}"

mkdir -p "${OUTPUT}"

"${CONTAINER_RUNTIME}" run --rm --platform "${PLATFORM}" \
  -e LINGXI_ARCH="${ARCH}" \
  -e LINGXI_ALPINE_VERSION="${ALPINE_VERSION}" \
  -e LINGXI_ALPINE_BRANCH="${ALPINE_BRANCH}" \
  -e LINGXI_ROOTFS_SHA256="${ROOTFS_SHA}" \
  -e LINGXI_PACKAGES="${PACKAGES}" \
  -e LINGXI_PNPM_VERSION="${PNPM_VERSION}" \
  -e LINGXI_PNPM_URL="${PNPM_URL}" \
  -e LINGXI_PNPM_SHA512="${PNPM_SHA512}" \
  -v "${INNER}:/inner.sh:ro" \
  -v "${PINS}:/pins.json:ro" \
  -v "${OUTPUT}:/out" \
  "${BUILDER_IMAGE}" \
  sh -c 'test "$(uname -m)" = "'"${ARCH}"'" || {
           echo "builder is $(uname -m), expected '"${ARCH}"'" >&2; exit 1; }
         sh /inner.sh'

CLOSURE="${OUTPUT}/${ARCH}/closure.json"
[[ -f "${CLOSURE}" ]] || { echo "[rootfs] builder produced no closure manifest" >&2; exit 1; }

# The release verifier takes `--apk-dir <dir>` and looks for <dir>/<abi>/<pkg>.apk,
# keyed by product ABI. The builder works in Alpine arch names, so publish a
# second view under the ABI name rather than making the caller translate.
case "${ARCH}" in
  aarch64) ABI="arm64-v8a" ;;
  x86_64) ABI="x86_64" ;;
esac
APK_CLOSURE_DIR="${OUTPUT}/apk-closure/${ABI}"
rm -rf "${APK_CLOSURE_DIR}"
mkdir -p "${APK_CLOSURE_DIR}"
for apk in "${OUTPUT}/${ARCH}/repo/${ARCH}"/*.apk; do
  ln "${apk}" "${APK_CLOSURE_DIR}/$(basename "${apk}")" 2>/dev/null \
    || cp "${apk}" "${APK_CLOSURE_DIR}/$(basename "${apk}")"
done
echo "[rootfs] release apk-dir view: ${OUTPUT}/apk-closure (--apk-dir)"

python3 "${SCRIPT_DIR}/update-local-app-pins.py" \
  --pins "${PINS}" --arch "${ARCH}" --closure "${CLOSURE}" --check
echo "[rootfs] closure matches the pins for ${ARCH}"

echo "[rootfs] output: ${OUTPUT}/${ARCH}/rootfs.tar.gz"
