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
container using the frozen Vite lockfile, then stage a read-only host seed
under a client build directory. Builds copy its dependencies into a disposable
project snapshot instead of mounting this tree into the guest.
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

# The exact native package closure is platform-dependent and owned by
# stage-local-app-runtime.py. Read it from there instead of keeping a second
# hand-maintained copy in shell.
expected_native_packages() {
  python3 - "${STAGE_SCRIPT}" "$1" "$2" "$3" <<'PY'
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
family = sys.argv[3]
selector = sys.argv[4]
names = staging.expected_native_packages_for(platform, family)
if not names:
    raise SystemExit(f"no {family} packages expected for platform {platform}")
if selector == "arm64":
    names = {name: version for name, version in names.items() if "arm64" in name}
elif selector == "x64":
    names = {name: version for name, version in names.items() if "x64" in name}
elif selector != "all":
    raise SystemExit(f"unknown native package selector: {selector}")
if not names:
    raise SystemExit(f"no {family} packages matched selector {selector} for platform {platform}")
if family == "rolldown":
    print(" ".join(sorted(name.removeprefix("@rolldown/") for name in names)))
else:
    print(" ".join(sorted(names)))
PY
}

if [[ "${PLATFORM}" == "android" ]]; then
  X64_ROLLDOWN="$(expected_native_packages android rolldown x64)"
  X64_LIGHTNINGCSS="$(expected_native_packages android lightningcss x64)"
else
  X64_ROLLDOWN=""
  X64_LIGHTNINGCSS=""
fi

"${CONTAINER_RUNTIME}" run --rm --platform linux/arm64 \
  -v "${WORKDIR}:/work" \
  -w /work \
  -e "X64_ROLLDOWN=${X64_ROLLDOWN}" \
  -e "X64_LIGHTNINGCSS=${X64_LIGHTNINGCSS}" \
  "${IMAGE}" \
  sh -lc "
    set -euo pipefail
    apk add --no-cache ${NODE_PACKAGE} ${NPM_PACKAGE} >/dev/null
    test \"\$(node --version)\" = 'v24.18.1'
    test \"\$(npm --version)\" = '11.12.1'
    test \"\$(npx --version)\" = '11.12.1'

    mkdir -p /work/targets/arm64 /work/targets/x64
    cp package.json package-lock.json /work/targets/arm64/
    cp package.json package-lock.json /work/targets/x64/

    install_target_tree() {
      target_root=\"\$1\"
      target_cpu=\"\$2\"
      (
        cd \"\$target_root\"
        npm_config_os=linux npm_config_cpu=\"\$target_cpu\" npm_config_libc=musl \
          npm ci --ignore-scripts --no-audit --no-fund --loglevel=error
        test ! -e node_modules/next
        test ! -e node_modules/@next
        rm -f node_modules/.package-lock.json
      )
    }

    merge_native_package() {
      source_root=\"\$1\"
      dest_root=\"\$2\"
      package_name=\"\$3\"
      src=\"\$source_root/node_modules/\$package_name\"
      dst=\"\$dest_root/node_modules/\$package_name\"
      test -d \"\$src\"
      mkdir -p \"\$(dirname \"\$dst\")\"
      rm -rf \"\$dst\"
      cp -R \"\$src\" \"\$dst\"
    }

    install_target_tree /work/targets/arm64 arm64
    if [ -n \"\$X64_ROLLDOWN\" ] || [ -n \"\$X64_LIGHTNINGCSS\" ]; then
      install_target_tree /work/targets/x64 x64
      for name in \$X64_ROLLDOWN; do
        merge_native_package /work/targets/x64 /work/targets/arm64 \"@rolldown/\$name\"
      done
      for name in \$X64_LIGHTNINGCSS; do
        merge_native_package /work/targets/x64 /work/targets/arm64 \"\$name\"
      done
    fi

    rm -rf /work/node_modules
    mv /work/targets/arm64/node_modules /work/node_modules
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
