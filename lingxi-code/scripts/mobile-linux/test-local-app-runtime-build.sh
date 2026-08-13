#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
BUILD_SCRIPT="${SCRIPT_DIR}/build-local-app-runtime.sh"
IOS_BUILD_SCRIPT="${REPO_ROOT}/clients/ios/scripts/build-local-app-runtime.sh"
OUTPUT_ROOT="${REPO_ROOT}/clients/ios/build/local-app-runtime-test-$RANDOM"
OUTPUT_WRAPPER="${REPO_ROOT}/clients/ios/build/local-app-runtime-wrapper-test-$RANDOM"
CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-podman}"
IMAGE="docker.io/library/alpine:3.24@sha256:e7a1a92a5bfeee40966aea60f0796b0e7917cc35591542701834f03a68fa3d18"
NODE_PACKAGE="nodejs=24.18.1-r0"
NPM_PACKAGE="npm=11.12.1-r0"
trap 'chmod -R u+w "${OUTPUT_ROOT}" "${OUTPUT_WRAPPER}" 2>/dev/null || true; rm -rf "${OUTPUT_ROOT}" "${OUTPUT_WRAPPER}"' EXIT

command -v "${CONTAINER_RUNTIME}" >/dev/null 2>&1 || {
  echo "container runtime not found: ${CONTAINER_RUNTIME}" >&2
  exit 1
}

LOCK_SHA_BEFORE="$(shasum -a 256 "${REPO_ROOT}/lingxi-code/local-apps/templates/vite-react-static-v1/package-lock.json" | awk '{print $1}')"
PKG_SHA_BEFORE="$(shasum -a 256 "${REPO_ROOT}/lingxi-code/local-apps/templates/vite-react-static-v1/package.json" | awk '{print $1}')"

WORKDIR="$(mktemp -d)"
trap 'chmod -R u+w "${OUTPUT_ROOT}" "${OUTPUT_WRAPPER}" "${WORKDIR}" 2>/dev/null || true; rm -rf "${OUTPUT_ROOT}" "${OUTPUT_WRAPPER}" "${WORKDIR}"' EXIT
cp -R "${REPO_ROOT}/lingxi-code/local-apps/templates/vite-react-static-v1/." "${WORKDIR}/"

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
    test -z "\${LD_PRELOAD:-}"
    node -e \"const c=require('node:crypto');const value=JSON.parse('{\\\"ok\\\":true}');if(!value.ok||c.createHash('sha256').update('lingxi').digest('hex').length!==64)process.exit(1)\"
    node -e \"fetch('https://registry.npmjs.org/vite').then(r=>{if(!r.ok)process.exit(1);return r.body.cancel()}).catch(()=>process.exit(1))\"
    for iteration in \$(seq 1 8); do node -e \"process.stdout.write(JSON.stringify({ok:true}))\" >/dev/null; done
    node -e \"const chunks=Array.from({length:16},()=>Buffer.alloc(8*1024*1024,7));if(chunks.reduce((n,b)=>n+b.length,0)!==134217728)process.exit(1)\"
    npm ci --ignore-scripts --no-audit --no-fund --loglevel=error
    find node_modules/@rolldown -maxdepth 1 -mindepth 1 -type d -name 'binding-*' ! -name 'binding-linux-arm64-musl' -exec rm -rf {} +
    test -f node_modules/@rolldown/binding-linux-arm64-musl/rolldown-binding.linux-arm64-musl.node
    test ! -e node_modules/next
    test ! -e node_modules/@next
    NODE_ENV=production node node_modules/vite/bin/vite.js build
    test -f out/index.html
  "

# `npm ci` ran against the WORKDIR copy, so the frozen-dependency assertion has
# to read the copy back. Re-hashing the untouched repo template compares it to
# itself and can never fail.
LOCK_SHA_AFTER="$(shasum -a 256 "${WORKDIR}/package-lock.json" | awk '{print $1}')"
PKG_SHA_AFTER="$(shasum -a 256 "${WORKDIR}/package.json" | awk '{print $1}')"
test "${LOCK_SHA_BEFORE}" = "${LOCK_SHA_AFTER}"
test "${PKG_SHA_BEFORE}" = "${PKG_SHA_AFTER}"
test -f "${WORKDIR}/out/index.html"
test -f "${WORKDIR}/node_modules/@rolldown/binding-linux-arm64-musl/rolldown-binding.linux-arm64-musl.node"
test ! -e "${WORKDIR}/node_modules/@rolldown/binding-linux-x64-musl"
test ! -e "${WORKDIR}/node_modules/next"
test ! -e "${WORKDIR}/node_modules/@next"

# These assertions run unconditionally. Gating them on a version constant the
# same commit was supposed to advance turns the only test of the staged runtime
# into one that prints "passed" having asserted nothing.
CONTAINER_RUNTIME="${CONTAINER_RUNTIME}" "${BUILD_SCRIPT}" --platform ios --variant store --output "${OUTPUT_ROOT}"
test -f "${OUTPUT_ROOT}/runtime-manifest.json"
test -f "${OUTPUT_ROOT}/node_modules/vite/bin/vite.js"
test -f "${OUTPUT_ROOT}/node_modules/@rolldown/binding-linux-arm64-musl/rolldown-binding.linux-arm64-musl.node"
test ! -e "${OUTPUT_ROOT}/node_modules/@rolldown/binding-linux-x64-musl"
test ! -e "${OUTPUT_ROOT}/node_modules/next"
test ! -e "${OUTPUT_ROOT}/node_modules/@next"
test ! -w "${OUTPUT_ROOT}/node_modules/vite/package.json"

python3 - "${OUTPUT_ROOT}" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / "runtime-manifest.json").read_text(encoding="utf-8"))
assert manifest["resolved_rolldown_bindings"] == ["@rolldown/binding-linux-arm64-musl"], manifest
assert manifest["read_only"] is True, manifest
PY

CONTAINER_RUNTIME="${CONTAINER_RUNTIME}" "${IOS_BUILD_SCRIPT}" --variant store --output "${OUTPUT_WRAPPER}"
test -f "${OUTPUT_WRAPPER}/runtime-manifest.json"

echo "local-app runtime build test passed"
