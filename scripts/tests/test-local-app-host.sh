#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
TEMP_ROOT="$(mktemp -d)"
trap 'chmod -R u+w "${TEMP_ROOT}"; rm -rf "${TEMP_ROOT}"' EXIT
python3 "${SCRIPT_DIR}/../local-apps/verify-local-app-host.py" --repo-root "${REPO_ROOT}"
python3 "${SCRIPT_DIR}/test_host_sdk_integration.py"
expect_rejection() {
  local label="$1"
  shift
  local output status
  set +e
  output="$("$@" 2>&1)"
  status=$?
  set -e
  if [[ "${status}" -eq 0 ]]; then
    echo "expected ${label}, but the command succeeded" >&2
    printf '%s\n' "${output}" >&2
    exit 1
  fi
  if printf '%s' "${output}" | grep -q "Traceback (most recent call last)"; then
    echo "expected ${label}, but the command CRASHED instead of rejecting" >&2
    printf '%s\n' "${output}" >&2
    exit 1
  fi
  if [[ "${status}" -ne 1 ]]; then
    echo "expected ${label} to exit 1, got ${status}" >&2
    printf '%s\n' "${output}" >&2
    exit 1
  fi
}
INSTALL_STAGED="${TEMP_ROOT}/install-src/store"
INSTALL_DEST="${TEMP_ROOT}/install-dest/local-app-runtime"
mkdir -p \
  "${INSTALL_STAGED}/node_modules/vite/bin"
printf '{"schema_version":1}\n' > "${INSTALL_STAGED}/runtime-manifest.json"
printf '#!/usr/bin/env node\n' > "${INSTALL_STAGED}/node_modules/vite/bin/vite.js"
# Mirror stage-local-app-runtime.py: files 0444, directories 0555, root 0755.
find "${INSTALL_STAGED}" -type f -exec chmod 0444 {} +
find "${INSTALL_STAGED}" -mindepth 1 -type d -exec chmod 0555 {} +
chmod 0755 "${INSTALL_STAGED}"

for _ in 1 2; do
  "${REPO_ROOT}/clients/ios/scripts/install-staged-local-app-runtime.sh" \
    --staged "${INSTALL_STAGED}" \
    --destination "${INSTALL_DEST}"
done
# NOT `expect_rejection … [ -e A ] || [ -e B ]`: the `||` would bind at the
# AND-OR list level, so the second probe would be passed to the SHELL rather
# than to `expect_rejection`, and short-circuited away on every passing run.
# One command, so both nesting modes are actually checked.
expect_rejection "second install nested the staged runtime inside the previous copy" \
  bash -c '[ -e "$1/store" ] || [ -e "$1/local-app-runtime" ]' _ "${INSTALL_DEST}"
test -f "${INSTALL_DEST}/runtime-manifest.json"
test -f "${INSTALL_DEST}/node_modules/vite/bin/vite.js"
expect_rejection "a missing staged runtime to fail the install" \
  "${REPO_ROOT}/clients/ios/scripts/install-staged-local-app-runtime.sh" \
  --staged "${TEMP_ROOT}/install-src/absent" \
  --destination "${INSTALL_DEST}" 2>/dev/null

RUNTIME_ASSET_VALIDATOR="${REPO_ROOT}/clients/ios/scripts/validate-local-app-build-assets.sh"
ROOTFS_MANIFEST="${TEMP_ROOT}/ios-rootfs-manifest.json"
printf '{"local_app_runtime":false}\n' > "${ROOTFS_MANIFEST}"
expect_rejection "FullDebug iphoneos to reject a bare rootfs" \
  "${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --staged "${INSTALL_STAGED}" \
  --rootfs-manifest "${ROOTFS_MANIFEST}"
printf '{"local_app_runtime":true}\n' > "${ROOTFS_MANIFEST}"
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --staged "${INSTALL_STAGED}" \
  --rootfs-manifest "${ROOTFS_MANIFEST}"
expect_rejection "StoreDebug iphoneos to reject missing toolchain assets" \
  "${RUNTIME_ASSET_VALIDATOR}" \
  --configuration StoreDebug \
  --platform iphoneos \
  --staged "${TEMP_ROOT}/install-src/absent" \
  --rootfs-manifest "${TEMP_ROOT}/missing-manifest.json"

"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration StoreDebug --platform iphoneos \
  --staged "${INSTALL_STAGED}" --rootfs-manifest "${ROOTFS_MANIFEST}"
printf '{"local_app_runtime":false}\n' > "${ROOTFS_MANIFEST}"
expect_rejection "StoreDebug iphoneos to reject a bare rootfs" \
  "${RUNTIME_ASSET_VALIDATOR}" --configuration StoreDebug --platform iphoneos \
  --staged "${INSTALL_STAGED}" --rootfs-manifest "${ROOTFS_MANIFEST}"

# --rootfs-only judges the ROOTFS ALONE. The Xcode staging phase has no staged
# local-app node_modules tree (commit 1a4385f09 removed the phase that built
# one, on purpose), so the full validator can never run there. This scoped mode
# is what the build wires in. Assert the EXACT exit code: an unrecognized flag
# also exits non-zero, which would make a bare `if ! ...` pass for the wrong
# reason and certify a validator that never understood the request.
printf '{"local_app_runtime":false}\n' > "${ROOTFS_MANIFEST}"
set +e
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --rootfs-only \
  --rootfs-manifest "${ROOTFS_MANIFEST}"
rootfs_only_rc=$?
set -e
expect_rejection "--rootfs-only to REJECT a bare rootfs with exit 1, got ${rootfs_only_rc}" \
  [ "${rootfs_only_rc}" -ne 1 ]

printf '{"local_app_runtime":true}\n' > "${ROOTFS_MANIFEST}"
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphoneos \
  --rootfs-only \
  --rootfs-manifest "${ROOTFS_MANIFEST}"

# A simulator build stays exempt even in rootfs-only mode.
"${RUNTIME_ASSET_VALIDATOR}" \
  --configuration FullDebug \
  --platform iphonesimulator \
  --rootfs-only \
  --rootfs-manifest "${TEMP_ROOT}/missing-manifest.json"


echo "native local-app integration tests passed"
