#!/usr/bin/env bash
set -euo pipefail

VARIANT=""
INPUT_DIR=""
APK_DIR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:-}"; shift 2 ;;
    --input) INPUT_DIR="${2:-}"; shift 2 ;;
    --apk-dir) APK_DIR="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct> --input <release-evidence-dir> --apk-dir <apk-closure-dir>" >&2; exit 2 ;;
esac
[[ -d "${INPUT_DIR}" ]] || { echo "input directory not found: ${INPUT_DIR}" >&2; exit 1; }
[[ -d "${APK_DIR}" ]] || { echo "APK closure directory not found: ${APK_DIR}" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ANDROID_DIR}/../.." && pwd)"
OUTPUT="${ANDROID_DIR}/app/build/generated/mobileLinux/${VARIANT}/assets/mobile-linux"
PINS="${REPO_ROOT}/docs/mobile-linux/mobile-linux-pins.json"

bash "${SCRIPT_DIR}/verify-local-app-supply-chain.sh" --release --apk-dir "${APK_DIR}"

python3 - "${INPUT_DIR}" "${OUTPUT}" "${PINS}" "${REPO_ROOT}" <<'PY'
import hashlib
import json
import pathlib
import shutil
import subprocess
import sys

source, output, pins_path, repo = map(pathlib.Path, sys.argv[1:])
pins = json.loads(pins_path.read_text())

if output.exists():
    shutil.rmtree(output)
output.mkdir(parents=True)

for abi in pins["rootfs"]["archives"]:
    manifest = source / abi / "rootfs-manifest.json"
    sbom = source / abi / "rootfs.spdx.json"
    lock = source / abi / "rootfs-build.lock.json"
    allowlist = source / abi / "executable-allowlist.json"
    for required in (manifest, sbom, lock, allowlist):
        if not required.is_file():
            raise SystemExit(f"missing release input: {required}")
    manifest_data = json.loads(manifest.read_text())
    filename = manifest_data.get("archive", {}).get("filename")
    if not isinstance(filename, str) or pathlib.PurePosixPath(filename).name != filename:
        raise SystemExit(f"unsafe rootfs archive filename for {abi}: {filename!r}")
    archive = source / abi / filename
    if not archive.is_file() or archive.is_symlink():
        raise SystemExit(f"missing or unsafe release input: {archive}")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    expected_manifest_abi = {"arm64-v8a": "arm64", "x86_64": "x86_64"}[abi]
    if manifest_data.get("abi") != expected_manifest_abi:
        raise SystemExit(f"rootfs manifest ABI mismatch for {abi}")
    recorded = manifest_data.get("archive", {}).get("sha256")
    if recorded != digest:
        raise SystemExit(f"rootfs manifest archive hash mismatch for {abi}")
    # The manifest travels with the archive, so it can only prove the evidence
    # directory is self-consistent. These two bind the shipped bytes to the
    # repository: the committed release digest, then the archive policy.
    subprocess.check_call(
        [
            sys.executable,
            str(repo / "lingxi-code/scripts/mobile-linux/rootfs_tool.py"),
            "verify-release-archive",
            "--pins",
            str(pins_path),
            "--abi",
            abi,
            "--archive",
            str(archive),
        ]
    )
    subprocess.check_call(
        [
            sys.executable,
            str(repo / "lingxi-code/scripts/mobile-linux/rootfs_tool.py"),
            "verify-archive",
            "--archive",
            str(archive),
        ]
    )
    subprocess.check_call(
        [
            sys.executable,
            str(repo / "lingxi-code/scripts/mobile-linux/rootfs_tool.py"),
            "validate-lock",
            "--lock",
            str(lock),
            "--manifest",
            str(manifest),
        ]
    )
    subprocess.check_call(
        [
            "bash",
            str(repo / "lingxi-code/scripts/mobile-linux/check-rootfs-manifest.sh"),
            str(manifest),
        ]
    )
    abi_out = output / "rootfs" / abi
    abi_out.mkdir(parents=True)
    shutil.copy2(archive, abi_out / filename)
    shutil.copy2(manifest, abi_out / manifest.name)
    shutil.copy2(sbom, abi_out / sbom.name)
    shutil.copy2(lock, abi_out / lock.name)
    shutil.copy2(allowlist, abi_out / allowlist.name)

licenses = output / "licenses"
licenses.mkdir()
shutil.copy2(repo / "docs/mobile-linux/mobile-linux-pins.json", output / "mobile-linux-pins.json")
shutil.copy2(repo / "docs/mobile-linux/local-app-runtime-pins.json", output / "local-app-runtime-pins.json")
shutil.copy2(repo / "docs/mobile-linux/sbom/local-app-runtime.spdx.json", output / "local-app-runtime.spdx.json")
shutil.copy2(repo / "docs/mobile-linux/LICENSES/NOTICE.md", licenses / "NOTICE.md")
openminis = repo / "docs/superpowers/references/OpenMinis"
proot = openminis / "deps/proot"
(licenses / "GPL-3.0-only.txt").write_bytes(
    subprocess.check_output(
        ["git", "-C", str(openminis), "show", f"{pins['components']['openminis']['commit']}:LICENSE"]
    )
)
(licenses / "GPL-2.0-or-later.txt").write_bytes(
    subprocess.check_output(
        ["git", "-C", str(proot), "show", f"{pins['components']['proot']['commit']}:COPYING"]
    )
)
print(f"staged verified MobileLinux assets: {output}")
PY
