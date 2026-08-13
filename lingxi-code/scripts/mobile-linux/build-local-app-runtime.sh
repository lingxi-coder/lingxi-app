#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TEMPLATE_DIR="${REPO_ROOT}/lingxi-code/local-apps/templates/vite-react-static-v1"
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
container using the frozen Vite lockfile, then stage a read-only runtime tree
under a client build directory.
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

# Which native slices survive the prune is PLATFORM-DEPENDENT, and the answer is
# owned by stage-local-app-runtime.py — the very next step, which rejects a tree
# that carries anything else *or* is missing anything it expects. iOS ships the
# arm64/musl slice alone (devices and Apple Silicon simulators are both arm64);
# one Android asset tree serves every ABI in the APK, so it keeps the full
# pinned set. Pruning unconditionally to arm64, as this used to, built a tree
# `--platform android` could never stage.
#
# Read from the staging script rather than re-listing here: a second hand-kept
# copy of this set is exactly how the two would drift apart.
expected_native_slices() {
  python3 - "${STAGE_SCRIPT}" "${PLATFORM}" "$1" <<'PY'
import importlib.util
import pathlib
import sys

source = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("local_app_staging", source)
if spec is None or spec.loader is None:
    raise SystemExit(f"cannot load {source}")
staging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(staging)

platform = sys.argv[2]
names, scope = staging.expected_rolldown_bindings_for(platform), "@rolldown/"
if not names:
    raise SystemExit(f"no rolldown packages expected for platform {platform}")
print(" ".join(sorted(name.removeprefix(scope) for name in names)))
PY
}

KEEP_ROLLDOWN="$(expected_native_slices rolldown)"

"${CONTAINER_RUNTIME}" run --rm --platform linux/arm64 \
  -v "${WORKDIR}:/work" \
  -w /work \
  -e "KEEP_ROLLDOWN=${KEEP_ROLLDOWN}" \
  "${IMAGE}" \
  sh -lc "
    set -euo pipefail
    apk add --no-cache ${NODE_PACKAGE} ${NPM_PACKAGE} >/dev/null
    test \"\$(node --version)\" = 'v24.18.1'
    test \"\$(npm --version)\" = '11.12.1'
    test \"\$(npx --version)\" = '11.12.1'
    npm ci --ignore-scripts --no-audit --no-fund --loglevel=error

    # \$1 scope dir, \$2 directory-name prefix, \$3 space-separated keep list.
    prune_native_slices() {
      for slice in \"node_modules/\$1/\$2\"*; do
        [ -d \"\$slice\" ] || continue
        name=\"\${slice##*/}\"
        case \" \$3 \" in
          *\" \$name \"*) ;;
          *) rm -rf \"\$slice\" ;;
        esac
      done
      for slice in \"node_modules/\$1/\$2\"*; do
        [ -d \"\$slice\" ] || continue
        name=\"\${slice##*/}\"
        case \" \$3 \" in
          *\" \$name \"*) ;;
          *)
            echo \"unexpected \$1 native slice survived the prune: \$name\" >&2
            exit 1
            ;;
        esac
      done
    }

    prune_native_slices '@rolldown' 'binding-' \"\$KEEP_ROLLDOWN\"

    for name in \$KEEP_ROLLDOWN; do
      test -d \"node_modules/@rolldown/\$name\"
      test -f \"node_modules/@rolldown/\$name/rolldown-binding.\${name#binding-}.node\"
    done
    test ! -e node_modules/next
    test ! -e node_modules/@next

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
