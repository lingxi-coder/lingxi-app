#!/usr/bin/env bash
# Build native support from the exact Cargo-locked SDK; stage only host outputs.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${IOS_DIR}/../../.." && pwd)"
LOCAL_APP_RUNTIME=0
SIMULATOR_ONLY=0
CLEAN=0
CONFIGURATION=Release
APK_DIR=""
ALPINE_VERSION=""
ROOTFS_ARCHIVE_INPUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --clean) CLEAN=1; shift ;;
    --debug) CONFIGURATION=Debug; shift ;;
    --release) CONFIGURATION=Release; shift ;;
    --simulator-only) SIMULATOR_ONLY=1; shift ;;
    --local-app-runtime) LOCAL_APP_RUNTIME=1; shift ;;
    --rootfs-archive) ROOTFS_ARCHIVE_INPUT="${2:?missing rootfs archive}"; shift 2 ;;
    --apk-dir) APK_DIR="${2:?missing APK directory}"; shift 2 ;;
    --alpine-version) ALPINE_VERSION="${2:?missing Alpine version}"; shift 2 ;;
    -h|--help)
      echo "usage: $0 [--debug|--release] [--simulator-only] [--local-app-runtime] [--rootfs-archive FILE] [--apk-dir DIR] [--clean]"
      exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -z "${ROOTFS_ARCHIVE_INPUT}" || "${LOCAL_APP_RUNTIME}" == 1 ]] || { echo "--rootfs-archive requires --local-app-runtime" >&2; exit 2; }
SDK_ROOT="$(python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --root)"
SDK_OUTPUT="${IOS_DIR}/build/mobile-linux-sdk"
CACHE="${IOS_DIR}/build/mobile-linux-cache"
# Clang/Swift PCH files embed their module-cache path. A moved checkout cannot
# reuse that cache; native sources and packaged outputs remain independently pinned.
MODULE_CACHE="${CACHE}/swift-modules"
LOCATION_STAMP="${MODULE_CACHE}/.build-location"
if [[ -d "${MODULE_CACHE}" ]] && [[ ! -f "${LOCATION_STAMP}" || "$(cat "${LOCATION_STAMP}")" != "${MODULE_CACHE}" ]]; then
  rm -rf "${MODULE_CACHE}"
fi
mkdir -p "${MODULE_CACHE}"
printf '%s\n' "${MODULE_CACHE}" > "${LOCATION_STAMP}"

STAGE_ROOT="${IOS_DIR}/build/linux-runtime/openminis"
FRAMEWORKS="${LINGXI_FRAMEWORKS_DIR:-${IOS_DIR}/Frameworks}"
FRAMEWORK=MobileLinuxNativeSupport.xcframework
# Replacing a toolchain rootfs with a bare rootfs must be deliberate.
if [[ "${SIMULATOR_ONLY}" == 0 && "${LOCAL_APP_RUNTIME}" == 0 && "${CLEAN}" == 0 && -f "${STAGE_ROOT}/manifest.json" ]]; then
  python3 - "${STAGE_ROOT}/manifest.json" <<'CHECK'
import json, sys
if json.load(open(sys.argv[1])).get("local_app_runtime"):
    raise SystemExit("local-app runtime already staged: use --local-app-runtime or --clean")
CHECK
fi
if [[ "${CLEAN}" == 1 ]]; then rm -rf "${STAGE_ROOT}"; fi
# Generated output must not retain module interfaces from an earlier SDK.
rm -rf "${SDK_OUTPUT}"
ARGS=(--kind native-support --output "${SDK_OUTPUT}" --cache "${CACHE}" --configuration "${CONFIGURATION}")
if [[ "${SIMULATOR_ONLY}" == 1 ]]; then ARGS+=(--simulator-only); fi
bash "${SDK_ROOT}/scripts/build/build-ios-xcframework.sh" "${ARGS[@]}"
python3 "${SDK_ROOT}/scripts/checks/verify-ios-native.py" --artifact-dir "${SDK_OUTPUT}"
mkdir -p "${FRAMEWORKS}"
rsync -a --delete "${SDK_OUTPUT}/${FRAMEWORK}/" "${FRAMEWORKS}/${FRAMEWORK}/"
python3 "${REPO_ROOT}/scripts/lib/mobile_linux_source.py" --json | python3 -c 'import json,sys; d=json.load(sys.stdin); json.dump({k:d[k] for k in ("source","revision")},sys.stdout)' > "${SDK_OUTPUT}/sdk-source.json"
if [[ "${SIMULATOR_ONLY}" == 1 ]]; then
  echo "Staged simulator-only native-support stubs; no device runtime claimed."
  exit 0
fi
if [[ "${LOCAL_APP_RUNTIME}" == 1 ]]; then
  ROOTFS_PROFILE=toolchain
  if [[ -n "${ROOTFS_ARCHIVE_INPUT}" ]]; then
    ARCHIVE="$(python3 - "${ROOTFS_ARCHIVE_INPUT}" "${SDK_ROOT}" <<'VERIFY'
import hashlib, json, pathlib, sys
archive, sdk = map(pathlib.Path, sys.argv[1:])
pins = json.loads((sdk / "docs/toolchains/runtime-pins.json").read_text(encoding="utf-8"))
evidence = sdk / "docs/mobile-linux/releases" / pins["alpine"]["version"] / "arm64-v8a"
record = json.loads((evidence / "rootfs-manifest.json").read_text(encoding="utf-8"))["archive"]
if archive.is_symlink() or not archive.is_file() or archive.stat().st_size != record["size_bytes"]:
    raise SystemExit("release rootfs archive missing or wrong size")
with archive.open("rb") as file:
    if hashlib.file_digest(file, "sha256").hexdigest() != record["sha256"]:
        raise SystemExit("release rootfs archive differs from Cargo-locked SDK digest")
print(archive.resolve())
VERIFY
)"
  else
    bash "${REPO_ROOT}/scripts/local-apps/build-local-app-rootfs.sh" --arch aarch64
    ARCHIVE="${IOS_DIR}/build/local-app-rootfs/aarch64/rootfs.tar.gz"
  fi
  VERSION="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["alpine"]["version"])' "${SDK_ROOT}/docs/toolchains/runtime-pins.json")"
  EXPECTED_ARCHIVE_SHA="$(python3 -c 'import hashlib,sys; print(hashlib.file_digest(open(sys.argv[1],"rb"),"sha256").hexdigest())' "${ARCHIVE}")"
else
  ROOTFS_PROFILE=base
  # Downloading is a host build policy. The SDK receives a verified local file.
  mkdir -p "${CACHE}"
  ARCHIVE="${CACHE}/base-rootfs-aarch64.tar.gz"
  VERSION="$(python3 - "${SDK_ROOT}/docs/toolchains/runtime-pins.json" "${ARCHIVE}" <<'FETCH'
import hashlib, json, pathlib, sys, urllib.request
pins = json.load(open(sys.argv[1]))["alpine"]
record = pins["minirootfs"]["aarch64"]
target = pathlib.Path(sys.argv[2])
if not target.is_file() or hashlib.sha256(target.read_bytes()).hexdigest() != record["sha256"]:
    temporary = target.with_suffix(".download")
    with urllib.request.urlopen(record["url"]) as response:
        temporary.write_bytes(response.read())
    if hashlib.sha256(temporary.read_bytes()).hexdigest() != record["sha256"]:
        temporary.unlink()
        raise SystemExit("pinned base rootfs digest mismatch")
    temporary.replace(target)
print(pins["version"])
FETCH
)"
  EXPECTED_ARCHIVE_SHA="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["alpine"]["minirootfs"]["aarch64"]["sha256"])' "${SDK_ROOT}/docs/toolchains/runtime-pins.json")"
fi
if [[ -n "${ALPINE_VERSION}" && "${ALPINE_VERSION}" != "${VERSION}" ]]; then
  echo "--alpine-version must match the SDK's pinned version ${VERSION}" >&2; exit 1
fi
bash "${SDK_ROOT}/scripts/build/prepare-ios-rootfs.sh" --archive "${ARCHIVE}" \
  --profile "${ROOTFS_PROFILE}" --expected-archive-sha256 "${EXPECTED_ARCHIVE_SHA}" \
  --output "${STAGE_ROOT}" --cache "${CACHE}" --native-output "${SDK_OUTPUT}/native"
python3 - "${STAGE_ROOT}/manifest.json" "${LOCAL_APP_RUNTIME}" "${VERSION}" "${SDK_OUTPUT}/sdk-source.json" <<'PROFILE'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
manifest = json.loads(path.read_text())
manifest.update(local_app_runtime=sys.argv[2] == "1", alpine_version=sys.argv[3],
                sdk_source=json.loads(pathlib.Path(sys.argv[4]).read_text()))
path.write_text(json.dumps(manifest, indent=2) + "\n")
PROFILE
if [[ -n "${APK_DIR}" ]]; then
  "${SCRIPT_DIR}/verify-local-app-supply-chain.sh" --release --apk-dir "${APK_DIR}"
else
  "${SCRIPT_DIR}/verify-local-app-supply-chain.sh"
fi
"${SCRIPT_DIR}/verify-linux-runtime.sh"
echo "Staged Cargo-locked SDK native support and rootfs: ${STAGE_ROOT}"
