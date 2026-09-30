#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FRAMEWORKS_DIR="${IOS_DIR}/Frameworks"
BUILD_DIR="${IOS_DIR}/build/sherpa-runtime"
REPO_ROOT="$(cd "${IOS_DIR}/../../.." && pwd)"
VOICE_MANIFEST="${REPO_ROOT}/resources/voice/models.json"
[[ -f "${VOICE_MANIFEST}" ]] || { echo "error: shared voice manifest is missing: ${VOICE_MANIFEST}" >&2; exit 1; }
command -v node >/dev/null 2>&1 || { echo "error: node is required to read the shared voice manifest" >&2; exit 1; }
RUNTIME_META="$(
  node -e '
    const fs = require("fs");
    const manifest = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    const artifact = manifest.runtime.ios;
    process.stdout.write([manifest.runtime.version, artifact.name, artifact.url, artifact.sha256].join("\t"));
  ' "${VOICE_MANIFEST}"
)"
# `node` intentionally emits no trailing newline. A here-string supplies the
# delimiter that `read` needs under `set -e`, while keeping real node failures
# visible through the command substitution above.
IFS=$'\t' read -r RUNTIME_VERSION ARCHIVE_NAME URL SHA256 <<<"${RUNTIME_META}"
[[ -n "${RUNTIME_VERSION}" && -n "${ARCHIVE_NAME}" && -n "${URL}" && -n "${SHA256}" ]] || {
  echo "error: ${VOICE_MANIFEST} did not yield a complete iOS runtime artifact" >&2
  exit 1
}
ARCHIVE="${BUILD_DIR}/${ARCHIVE_NAME}"
EXTRACTED="${BUILD_DIR}/extracted"

mkdir -p "${BUILD_DIR}" "${FRAMEWORKS_DIR}"

if [ ! -f "${ARCHIVE}" ]; then
  curl --fail --location --retry 3 --output "${ARCHIVE}.partial" "${URL}"
  mv "${ARCHIVE}.partial" "${ARCHIVE}"
fi

ACTUAL_SHA="$(shasum -a 256 "${ARCHIVE}" | awk '{print $1}')"
if [ "${ACTUAL_SHA}" != "${SHA256}" ]; then
  echo "error: sherpa-onnx iOS runtime checksum mismatch" >&2
  echo "expected: ${SHA256}" >&2
  echo "actual:   ${ACTUAL_SHA}" >&2
  exit 1
fi

rm -rf "${EXTRACTED}"
mkdir -p "${EXTRACTED}"
tar -xjf "${ARCHIVE}" -C "${EXTRACTED}"

SHERPA_SOURCE="${EXTRACTED}/build-ios/sherpa-onnx.xcframework"
ORT_SOURCE="${EXTRACTED}/build-ios/ios-onnxruntime/1.17.1/onnxruntime.xcframework"
test -d "${SHERPA_SOURCE}"
test -d "${ORT_SOURCE}"

rm -rf "${FRAMEWORKS_DIR}/sherpa-onnx.xcframework"
rm -rf "${FRAMEWORKS_DIR}/onnxruntime.xcframework"
cp -R "${SHERPA_SOURCE}" "${FRAMEWORKS_DIR}/sherpa-onnx.xcframework"
cp -R "${ORT_SOURCE}" "${FRAMEWORKS_DIR}/onnxruntime.xcframework"

echo "Installed sherpa-onnx ${RUNTIME_VERSION} iOS runtime into ${FRAMEWORKS_DIR}"
