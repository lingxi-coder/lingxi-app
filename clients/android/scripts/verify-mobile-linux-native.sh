#!/usr/bin/env bash
set -euo pipefail

VARIANT=""
if [[ "${1:-}" == "--variant" ]]; then
  VARIANT="${2:-}"
fi
case "${VARIANT}" in
  play|direct) ;;
  *) echo "usage: $0 --variant <play|direct>" >&2; exit 2 ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
JNI_ROOT="${SCRIPT_DIR}/../app/src/${VARIANT}/jniLibs"

python3 - "${JNI_ROOT}" <<'PY'
import pathlib
import struct
import sys

root = pathlib.Path(sys.argv[1])
machines = {"arm64-v8a": 183, "x86_64": 62}
required = (
    "libandroid_aar.so",
    "libproot.so",
    "libproot-loader.so",
    "libmobile_linux_policy_launcher.so",
    "libpty_bridge.so",
    "libmksh.so",
    "libtoybox.so",
)

errors = []
for abi, expected_machine in machines.items():
    for filename in required:
        path = root / abi / filename
        if not path.is_file():
            errors.append(f"missing {path}")
            continue
        header = path.read_bytes()[:20]
        if len(header) < 20 or header[:4] != b"\x7fELF":
            errors.append(f"not ELF: {path}")
            continue
        if header[4] != 2:
            errors.append(f"not ELF64: {path}")
        endian = "<" if header[5] == 1 else ">" if header[5] == 2 else None
        if endian is None:
            errors.append(f"invalid ELF byte order: {path}")
            continue
        machine = struct.unpack(endian + "H", header[18:20])[0]
        if machine != expected_machine:
            errors.append(
                f"wrong machine for {path}: expected {expected_machine}, got {machine}"
            )

if errors:
    raise SystemExit("\n".join(errors))
print(f"verified seven MobileLinux native artifacts for both ABIs under {root}")
PY
