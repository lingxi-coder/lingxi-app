#!/usr/bin/env bash
# Reuse only native support built from the same immutable SDK revision.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${IOS_DIR}/../.." && pwd)"
SDK_ROOT="$(python3 "${REPO_ROOT}/lingxi-code/scripts/mobile_linux_source.py" --root)"
SDK_OUTPUT="${IOS_DIR}/build/mobile-linux-sdk"
FRAMEWORKS="${LINGXI_FRAMEWORKS_DIR:-${IOS_DIR}/Frameworks}"
python3 "${SDK_ROOT}/scripts/verify-ios-native.py" --artifact-dir "${SDK_OUTPUT}"
diff -qr "${SDK_OUTPUT}/MobileLinuxNativeSupport.xcframework" "${FRAMEWORKS}/MobileLinuxNativeSupport.xcframework"
python3 - "${REPO_ROOT}" "${SDK_OUTPUT}" "${IOS_DIR}/build/linux-runtime/openminis" <<'CHECK'
import hashlib, json, pathlib, subprocess, sys
repo, output, stage = map(pathlib.Path, sys.argv[1:])
resolved = json.loads(subprocess.check_output([sys.executable, str(repo / "lingxi-code/scripts/mobile_linux_source.py"), "--json"], text=True))
native = json.loads((output / "native-support-manifest.json").read_text())
if "iphoneos-arm64" not in native.get("slices", []):
    raise SystemExit("staged native support contains simulator stubs only; rebuild device support")
recorded = json.loads((output / "sdk-source.json").read_text())
for key in ("source", "revision"):
    if recorded[key] != resolved[key]:
        raise SystemExit("native support SDK revision differs from Cargo.lock; rebuild it")
manifest = json.loads((stage / "manifest.json").read_text())
archive = stage / "resources/alpine-rootfs.zip"
if hashlib.sha256(archive.read_bytes()).hexdigest() != manifest["rootfs_zip_sha256"]:
    raise SystemExit("staged iOS rootfs digest mismatch")
if manifest.get("sdk_source", {}).get("revision") != resolved["revision"]:
    raise SystemExit("rootfs was staged for a different SDK revision")
if not (stage / "resources/RootfsPatch.bundle").is_dir():
    raise SystemExit("staged iOS rootfs is missing RootfsPatch.bundle")
CHECK
