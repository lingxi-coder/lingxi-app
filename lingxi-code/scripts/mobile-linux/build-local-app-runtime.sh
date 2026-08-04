#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TEMPLATE_DIR="${REPO_ROOT}/lingxi-code/local-apps/templates/next-static-v1"
STAGE_SCRIPT="${SCRIPT_DIR}/stage-local-app-runtime.py"

IMAGE="docker.io/library/alpine:3.24@sha256:e7a1a92a5bfeee40966aea60f0796b0e7917cc35591542701834f03a68fa3d18"
NODE_PACKAGE="nodejs=24.18.1-r0"
NPM_PACKAGE="npm=11.12.1-r0"
CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-}"
PLATFORM=""
VARIANT=""
OUTPUT=""

usage() {
  cat <<'EOF'
Usage: build-local-app-runtime.sh --platform <ios|android> --variant <name> [--output <dir>]

Build the local-app Node runtime inside a digest-pinned Alpine 3.24 arm64/musl
container using the frozen next-static-v1 lockfile, then stage a read-only
runtime tree under a client build directory.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform) PLATFORM="${2:-}"; shift 2 ;;
    --variant) VARIANT="${2:-}"; shift 2 ;;
    --output) OUTPUT="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "${PLATFORM}" in
  ios|android) ;;
  *) echo "--platform must be ios or android" >&2; exit 2 ;;
esac
if [[ -z "${VARIANT}" ]]; then
  echo "--variant is required" >&2
  exit 2
fi

if [[ -z "${OUTPUT}" ]]; then
  case "${PLATFORM}" in
    ios) OUTPUT="${REPO_ROOT}/clients/ios/build/local-app-runtime/${VARIANT}" ;;
    android) OUTPUT="${REPO_ROOT}/clients/android/app/build/local-app-runtime/${VARIANT}" ;;
  esac
fi

if [[ -z "${CONTAINER_RUNTIME}" ]]; then
  if command -v podman >/dev/null 2>&1; then
    CONTAINER_RUNTIME="podman"
  elif command -v docker >/dev/null 2>&1; then
    CONTAINER_RUNTIME="docker"
  else
    echo "neither podman nor docker is available" >&2
    exit 1
  fi
fi
command -v "${CONTAINER_RUNTIME}" >/dev/null 2>&1 || {
  echo "container runtime not found: ${CONTAINER_RUNTIME}" >&2
  exit 1
}
[[ -f "${TEMPLATE_DIR}/package.json" && -f "${TEMPLATE_DIR}/package-lock.json" ]] || {
  echo "template package metadata is missing: ${TEMPLATE_DIR}" >&2
  exit 1
}

LOCK_SHA_BEFORE="$(shasum -a 256 "${TEMPLATE_DIR}/package-lock.json" | awk '{print $1}')"
PKG_SHA_BEFORE="$(shasum -a 256 "${TEMPLATE_DIR}/package.json" | awk '{print $1}')"

WORKDIR="$(mktemp -d)"
cleanup() {
  chmod -R u+w "${WORKDIR}" 2>/dev/null || true
  rm -rf "${WORKDIR}"
}
trap cleanup EXIT

cp "${TEMPLATE_DIR}/package.json" "${WORKDIR}/package.json"
cp "${TEMPLATE_DIR}/package-lock.json" "${WORKDIR}/package-lock.json"

"${CONTAINER_RUNTIME}" run --rm --platform linux/arm64 \
  -v "${WORKDIR}:/work" \
  -w /work \
  "${IMAGE}" \
  sh -lc "
    set -euo pipefail
    apk add --no-cache ${NODE_PACKAGE} ${NPM_PACKAGE} >/dev/null
    test \"\$(node --version)\" = 'v24.18.1'
    test \"\$(npm --version)\" = '11.12.1'
    test \"\$(npx --version)\" = '11.12.1'
    npm ci --ignore-scripts --no-audit --no-fund --loglevel=error
    find node_modules/@next -maxdepth 1 -mindepth 1 -type d -name 'swc-*' ! -name 'swc-linux-arm64-musl' -exec rm -rf {} +
    test -d node_modules/@next/swc-linux-arm64-musl
    test -f node_modules/@next/swc-linux-arm64-musl/next-swc.linux-arm64-musl.node
    if find node_modules/@next -maxdepth 1 -mindepth 1 -type d -name 'swc-*' ! -name 'swc-linux-arm64-musl' | grep -q .; then
      echo 'non-arm64-musl SWC package resolved in runtime node_modules' >&2
      exit 1
    fi
    rm -f node_modules/.package-lock.json
  "

LOCK_SHA_AFTER="$(shasum -a 256 "${WORKDIR}/package-lock.json" | awk '{print $1}')"
PKG_SHA_AFTER="$(shasum -a 256 "${WORKDIR}/package.json" | awk '{print $1}')"
if [[ "${LOCK_SHA_BEFORE}" != "${LOCK_SHA_AFTER}" ]]; then
  echo "npm ci mutated package-lock.json; refusing to stage runtime" >&2
  exit 1
fi
if [[ "${PKG_SHA_BEFORE}" != "${PKG_SHA_AFTER}" ]]; then
  echo "npm ci mutated package.json; refusing to stage runtime" >&2
  exit 1
fi

python3 "${STAGE_SCRIPT}" \
  --repo-root "${REPO_ROOT}" \
  --node-modules "${WORKDIR}/node_modules" \
  --output "${OUTPUT}" \
  --platform "${PLATFORM}" \
  --variant "${VARIANT}"

echo "built local-app runtime: ${OUTPUT}"
