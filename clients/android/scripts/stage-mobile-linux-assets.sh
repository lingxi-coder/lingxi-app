#!/usr/bin/env bash
set -euo pipefail

VARIANT=""
INPUT_DIR=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) VARIANT="${2:-}"; shift 2 ;;
    --input) INPUT_DIR="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct> --input <release-evidence-dir>" >&2; exit 2 ;;
esac
[[ -d "${INPUT_DIR}" ]] || { echo "input directory not found: ${INPUT_DIR}" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANDROID_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${ANDROID_DIR}/../.." && pwd)"
OUTPUT="${ANDROID_DIR}/app/build/generated/mobileLinux/${VARIANT}/assets/mobile-linux"
PINS="${REPO_ROOT}/docs/mobile-linux/mobile-linux-pins.json"

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

for abi, item in pins["rootfs"]["archives"].items():
    filename = pathlib.PurePosixPath(item["url"]).name
    archive = source / abi / filename
    manifest = source / abi / "rootfs-manifest.json"
    sbom = source / abi / "rootfs.spdx.json"
    for required in (archive, manifest, sbom):
        if not required.is_file():
            raise SystemExit(f"missing release input: {required}")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != item["sha256"]:
        raise SystemExit(
            f"rootfs SHA-256 mismatch for {abi}: expected {item['sha256']}, got {digest}"
        )
    manifest_data = json.loads(manifest.read_text())
    expected_manifest_abi = {"arm64-v8a": "arm64", "x86_64": "x86_64"}[abi]
    if manifest_data.get("abi") != expected_manifest_abi:
        raise SystemExit(f"rootfs manifest ABI mismatch for {abi}")
    recorded = manifest_data.get("archive", {}).get("sha256")
    if recorded != digest:
        raise SystemExit(f"rootfs manifest archive hash mismatch for {abi}")
    abi_out = output / "rootfs" / abi
    abi_out.mkdir(parents=True)
    shutil.copy2(archive, abi_out / filename)
    shutil.copy2(manifest, abi_out / manifest.name)
    shutil.copy2(sbom, abi_out / sbom.name)

licenses = output / "licenses"
licenses.mkdir()
shutil.copy2(repo / "docs/mobile-linux/mobile-linux-pins.json", output / "mobile-linux-pins.json")
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
