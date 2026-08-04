#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../../.." && pwd)"
tool="${script_dir}/rootfs_tool.py"
packager="${script_dir}/package-rootfs-release.sh"
manifest_validator="${script_dir}/check-rootfs-manifest.sh"
tmp_root="$(mktemp -d)"
trap 'rm -rf "${tmp_root}"' EXIT
export SOURCE_DATE_EPOCH=0

fixture_root="${tmp_root}/rootfs"
mkdir -p "${fixture_root}/bin" "${fixture_root}/sbin" "${fixture_root}/usr/bin" "${fixture_root}/usr/lib" "${fixture_root}/lib/apk/db" "${fixture_root}/etc/apk" "${fixture_root}/tmp" "${fixture_root}/var/tmp" "${fixture_root}/workspace" "${fixture_root}/root"

python3 - <<'PY' "${fixture_root}" "${repo_root}/docs/mobile-linux/local-app-runtime-pins.json"
import json
import os
import pathlib
import stat
import sys

PINS_PATH = pathlib.Path(sys.argv[2])

root = pathlib.Path(sys.argv[1])

def write_elf(path: pathlib.Path, payload: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\x7fELF" + payload)
    path.chmod(0o755)

write_elf(root / "bin" / "busybox", b"busybox")
os.link(root / "bin" / "busybox", root / "bin" / "sh")
write_elf(root / "sbin" / "apk", b"apk")
write_elf(root / "usr" / "bin" / "git", b"git")
write_elf(root / "usr" / "bin" / "node", b"node")
write_elf(root / "usr" / "bin" / "ssh", b"ssh")
write_elf(root / "usr" / "bin" / "python3", b"python3")
write_elf(root / "usr" / "lib" / "libpython3.12.so.1.0", b"libpython")
(root / "usr" / "lib" / "python3.12").mkdir(parents=True, exist_ok=True)
(root / "usr" / "lib" / "python3.12" / "site.py").write_text(
    "# immutable stdlib fixture\n",
    encoding="utf-8",
)
# Derived from the pins so the fixture cannot drift away from the versions
# the tool enforces -- hardcoding them here is what made this test fail the
# moment the product moved to Alpine 3.24.1.
_pins = json.loads(PINS_PATH.read_text(encoding="utf-8"))
_alpine = _pins["alpine"]
(root / "etc" / "apk" / "repositories").write_text(
    "\n".join(_alpine["repositories"]) + "\n", encoding="utf-8"
)
(root / "etc" / "alpine-release").write_text(_alpine["version"] + "\n", encoding="utf-8")
_entries = [
    ("apk-tools", "2.14-r0", "GPL-2.0-only"),
    ("busybox", "1.0-r0", "GPL-2.0-only"),
]
_entries += [
    (name, version, "NOASSERTION")
    for name, version in sorted(_pins["runtime_packages"].items())
]
(root / "lib" / "apk" / "db" / "installed").write_text(
    "\n".join(
        f"P:{name}\nV:{version}\nA:arm64\nL:{license}\n"
        for name, version, license in _entries
    ),
    encoding="utf-8",
)
PY

python3 "${tool}" verify-tree --root "${fixture_root}"

cp "${fixture_root}/lib/apk/db/installed" "${tmp_root}/installed.good"
# Bump the pinned Git version inside the fixture. Derived from the pins and the
# substitution is asserted, because a sed pattern that silently stops matching
# leaves this test passing while proving nothing.
python3 - <<'MUTATE' "${fixture_root}/lib/apk/db/installed" "${repo_root}/docs/mobile-linux/local-app-runtime-pins.json"
import json
import pathlib
import sys

db = pathlib.Path(sys.argv[1])
pinned = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))["runtime_packages"]["git"]
text = db.read_text(encoding="utf-8")
needle = f"V:{pinned}"
if needle not in text:
    raise SystemExit(f"fixture does not contain the pinned Git version {needle!r}")
db.write_text(text.replace(needle, "V:0.0.0-r0", 1), encoding="utf-8")
MUTATE
if python3 "${tool}" verify-tree --root "${fixture_root}"; then
  echo "expected verify-tree to fail on fixed Git version drift" >&2
  exit 1
fi
cp "${tmp_root}/installed.good" "${fixture_root}/lib/apk/db/installed"

# npm and npx are shipped now, so they must be ACCEPTED; the alternative
# package managers are what stay out. Assert both directions -- testing only
# the rejection would keep passing if the allow-list silently emptied.
touch "${fixture_root}/usr/bin/npm" "${fixture_root}/usr/bin/npx"
python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null || {
  echo "verify-tree must accept the shipped npm/npx" >&2
  exit 1
}
for forbidden in corepack pnpm yarn; do
  touch "${fixture_root}/usr/bin/${forbidden}"
  if python3 "${tool}" verify-tree --root "${fixture_root}"; then
    echo "expected verify-tree to reject ${forbidden} in the runtime image" >&2
    exit 1
  fi
  rm -f "${fixture_root}/usr/bin/${forbidden}"
done
rm -f "${fixture_root}/usr/bin/npm" "${fixture_root}/usr/bin/npx"

# Alpine ships npm, npx and git's helper commands as relative symlinks in
# binary directories, so those must be ACCEPTED -- but only while they stay
# safe. Assert the accept case first: a policy that rejected everything would
# otherwise pass all the reject cases below and look correct.
ln -sf ../lib/node_modules/npm/bin/npm-cli.js "${fixture_root}/usr/bin/npm-link"
mkdir -p "${fixture_root}/usr/lib/node_modules/npm/bin"
printf '#!/bin/sh\n' > "${fixture_root}/usr/lib/node_modules/npm/bin/npm-cli.js"
python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null || {
  echo "verify-tree must accept a safe relative symlink in a binary directory" >&2
  exit 1
}
rm -f "${fixture_root}/usr/bin/npm-link"

# Each of these is what makes such a link dangerous.
ln -sf ./definitely-not-here "${fixture_root}/usr/bin/lx-dangling"
if python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null 2>&1; then
  echo "expected verify-tree to reject a dangling symlink in /usr/bin" >&2
  exit 1
fi
rm -f "${fixture_root}/usr/bin/lx-dangling"

ln -sf /etc/passwd "${fixture_root}/usr/bin/lx-absolute"
if python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null 2>&1; then
  echo "expected verify-tree to reject an absolute symlink in /usr/bin" >&2
  exit 1
fi
rm -f "${fixture_root}/usr/bin/lx-absolute"

ln -sf ../../../../../../etc/passwd "${fixture_root}/usr/bin/lx-escape"
if python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null 2>&1; then
  echo "expected verify-tree to reject a rootfs-escaping symlink in /usr/bin" >&2
  exit 1
fi
rm -f "${fixture_root}/usr/bin/lx-escape"

ln -sf ../lib "${fixture_root}/usr/bin/lx-dir"
if python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null 2>&1; then
  echo "expected verify-tree to reject a directory symlink in /usr/bin" >&2
  exit 1
fi
rm -f "${fixture_root}/usr/bin/lx-dir"
rm -rf "${fixture_root}/usr/lib/node_modules"

manifest_path="${tmp_root}/rootfs-manifest.json"
lock_path="${tmp_root}/rootfs-build.lock.json"
spdx_path="${tmp_root}/rootfs.spdx.json"
allowlist_path="${tmp_root}/executable-allowlist.json"

python3 "${tool}" generate-lock --root "${fixture_root}" --output "${lock_path}"
python3 "${tool}" generate-manifest \
  --root "${fixture_root}" \
  --runtime android-proot \
  --platform android \
  --abi arm64 \
  --rootfs-version 1.0.0 \
  --archive-filename alpine-rootfs-android-arm64-v1.0.0.tar.gz \
  --archive-sha256 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef \
  --archive-size 123 \
  --output "${manifest_path}"
python3 "${tool}" generate-spdx --root "${fixture_root}" --name test-rootfs --output "${spdx_path}"
python3 "${tool}" snapshot-allowlist --manifest "${manifest_path}" --output "${allowlist_path}"
python3 "${tool}" validate-lock --lock "${lock_path}" --manifest "${manifest_path}"

cp "${manifest_path}" "${tmp_root}/mismatched-manifest.json"
python3 - <<'PY' "${tmp_root}/mismatched-manifest.json"
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
manifest = json.loads(path.read_text(encoding="utf-8"))
manifest["packages"][0]["version"] = "9999-r0"
path.write_text(json.dumps(manifest), encoding="utf-8")
PY
if python3 "${tool}" validate-lock \
  --lock "${lock_path}" \
  --manifest "${tmp_root}/mismatched-manifest.json"
then
  echo "expected lock validation to reject package-version drift" >&2
  exit 1
fi

tar_archive="${tmp_root}/rootfs.tar"
python3 "${tool}" build-archive --root "${fixture_root}" --output "${tar_archive}" --source-date-epoch "${SOURCE_DATE_EPOCH}"
python3 "${tool}" verify-archive --archive "${tar_archive}"

python3 - <<'PY' "${tmp_root}/bad-archive.tar"
import io
import pathlib
import sys
import tarfile

archive = pathlib.Path(sys.argv[1])
with tarfile.open(archive, "w") as tar:
    data = b"\x7fELFbusybox"
    busybox = tarfile.TarInfo("bin/busybox")
    busybox.mode = 0o755
    busybox.size = len(data)
    tar.addfile(busybox, io.BytesIO(data))

    sh = tarfile.TarInfo("bin/sh")
    sh.type = tarfile.SYMTYPE
    sh.linkname = "busybox"
    sh.mode = 0o777
    tar.addfile(sh)
PY
if python3 "${tool}" verify-archive --archive "${tmp_root}/bad-archive.tar"; then
  echo "expected bad archive verification to fail" >&2
  exit 1
fi

# A relative, resolvable symlink in /bin is legitimate (busybox applets, npm,
# git's helpers) and must be accepted; the dangerous shapes are covered above.
ln -s busybox "${fixture_root}/bin/sh.symlink-test"
python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null || {
  echo "verify-tree must accept a safe relative symlink in /bin" >&2
  exit 1
}
rm -f "${fixture_root}/bin/sh.symlink-test"

ln -s ../../../etc/passwd "${fixture_root}/bin/escape.symlink-test"
if python3 "${tool}" verify-tree --root "${fixture_root}" >/dev/null 2>&1; then
  echo "expected verify-tree to fail with a rootfs-escaping symlink in /bin" >&2
  exit 1
fi
rm -f "${fixture_root}/bin/escape.symlink-test"

mv "${fixture_root}/sbin/apk" "${fixture_root}/sbin/apk.disabled"
if python3 "${tool}" verify-tree --root "${fixture_root}"; then
  echo "expected verify-tree to fail when apk is missing" >&2
  exit 1
fi
mv "${fixture_root}/sbin/apk.disabled" "${fixture_root}/sbin/apk"

release1="${tmp_root}/release-1"
release2="${tmp_root}/release-2"
archive1="${tmp_root}/release-1/rootfs.tar.gz"
archive2="${tmp_root}/release-2/rootfs.tar.gz"

bash "${packager}" "${fixture_root}" android-proot android arm64 1.0.0 "${archive1}" "${release1}"
bash "${packager}" "${fixture_root}" android-proot android arm64 1.0.0 "${archive2}" "${release2}"
python3 "${tool}" verify-archive --archive "${archive1}"
bash "${manifest_validator}" "${release1}/rootfs-manifest.json"

python3 - <<'PY' "${archive1}" "${archive2}" "${release1}" "${release2}"
import hashlib
import pathlib
import sys

archive1 = pathlib.Path(sys.argv[1])
archive2 = pathlib.Path(sys.argv[2])
release1 = pathlib.Path(sys.argv[3])
release2 = pathlib.Path(sys.argv[4])

def sha(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()

if sha(archive1) != sha(archive2):
    raise SystemExit("expected repeated packaging archives to be byte-identical")

for filename in [
    "rootfs-manifest.json",
    "rootfs-build.lock.json",
    "rootfs.spdx.json",
    "executable-allowlist.json",
]:
    left = (release1 / filename).read_bytes()
    right = (release2 / filename).read_bytes()
    if left != right:
        raise SystemExit(f"expected repeated packaging output to match exactly: {filename}")
PY

echo "rootfs tooling tests passed"
