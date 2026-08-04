#!/bin/sh
#
# Runs INSIDE a digest-pinned Alpine container. Builds the local-app Alpine
# rootfs for one architecture and emits it as a tarball plus a closure manifest.
#
# The vendored OpenMinis `prepare_alpine_rootfs.sh` only ever extracts a bare
# minirootfs and runs fakefsify -- it never installs a single APK, which is why
# `--local-app-runtime` could never satisfy `rootfs_tool.py verify-tree`. This
# script is the missing step: it resolves the full recursive closure, installs
# it OFFLINE, and hands back a tree that already contains node/npm/git/python.
#
# Inputs (environment):
#   LINGXI_ARCH             Alpine arch (aarch64 | x86_64)
#   LINGXI_ALPINE_VERSION   e.g. 3.24.1
#   LINGXI_ALPINE_BRANCH    e.g. v3.24
#   LINGXI_PACKAGES         space-separated `name=version` list
#   LINGXI_ROOTFS_SHA256    expected minirootfs digest (empty to trust the CDN's)
#
# Outputs (under /out/<arch>/):
#   rootfs.tar.gz       installed rootfs, ready for fakefsify
#   closure.json        every APK in the closure with url + sha256 + repo
#   installed.txt       `apk info -v` of the finished tree
#
set -eu

ARCH="${LINGXI_ARCH:?LINGXI_ARCH required}"
ALPINE_VERSION="${LINGXI_ALPINE_VERSION:?LINGXI_ALPINE_VERSION required}"
ALPINE_BRANCH="${LINGXI_ALPINE_BRANCH:?LINGXI_ALPINE_BRANCH required}"
PKGS="${LINGXI_PACKAGES:?LINGXI_PACKAGES required}"
EXPECTED_ROOTFS_SHA="${LINGXI_ROOTFS_SHA256:-}"
CDN=https://dl-cdn.alpinelinux.org/alpine

OUT="/out/${ARCH}"
TARGET="/target-${ARCH}"
REPO="${OUT}/repo"

# Builder-side tools only; none of this reaches the rootfs being assembled.
# python3 is needed for the closure manifest and is not in the base image.
#
# Cached under the mounted output directory rather than fetched fresh each run.
# Keyed by arch: an .apk filename carries no architecture, so `gcc-15.2.0-r5.apk`
# is the same name for aarch64 and x86_64. One shared cache directory would let
# an x86_64 build pick up the aarch64 build's package.
BUILDER_CACHE="/out/.builder-cache/${ARCH}"
mkdir -p "${BUILDER_CACHE}"
apk add --cache-dir "${BUILDER_CACHE}" curl python3 >/dev/null 2>&1

rm -rf "${OUT}" "${TARGET}"
mkdir -p "${OUT}" "${TARGET}" "${REPO}/${ARCH}"

echo "[rootfs:${ARCH}] fetching minirootfs ${ALPINE_VERSION}"
curl -sSfL -o "${OUT}/minirootfs.tar.gz" \
  "${CDN}/${ALPINE_BRANCH}/releases/${ARCH}/alpine-minirootfs-${ALPINE_VERSION}-${ARCH}.tar.gz"
curl -sSfL -o "${OUT}/minirootfs.sha256" \
  "${CDN}/${ALPINE_BRANCH}/releases/${ARCH}/alpine-minirootfs-${ALPINE_VERSION}-${ARCH}.tar.gz.sha256"
ROOTFS_SHA="$(sha256sum "${OUT}/minirootfs.tar.gz" | awk '{print $1}')"
CDN_SHA="$(awk '{print $1}' "${OUT}/minirootfs.sha256")"
if [ "${ROOTFS_SHA}" != "${CDN_SHA}" ]; then
  echo "[rootfs:${ARCH}] minirootfs digest does not match the CDN checksum" >&2
  exit 1
fi
if [ -n "${EXPECTED_ROOTFS_SHA}" ] && [ "${ROOTFS_SHA}" != "${EXPECTED_ROOTFS_SHA}" ]; then
  echo "[rootfs:${ARCH}] minirootfs digest ${ROOTFS_SHA} diverged from the pin ${EXPECTED_ROOTFS_SHA}" >&2
  exit 1
fi

tar -xzf "${OUT}/minirootfs.tar.gz" -C "${TARGET}"
printf '%s/%s/main\n%s/%s/community\n' "${CDN}" "${ALPINE_BRANCH}" "${CDN}" "${ALPINE_BRANCH}" \
  > "${TARGET}/etc/apk/repositories"
cp /etc/resolv.conf "${TARGET}/etc/resolv.conf" 2>/dev/null || true
apk --root "${TARGET}" --arch "${ARCH}" update >/dev/null 2>&1

echo "[rootfs:${ARCH}] resolving recursive APK closure"
# Retried: a 60-package fetch against the public CDN routinely trips a transient
# "I/O error" or "DNS: no address for host" on one package, and losing the whole
# build to that is pure waste. Already-downloaded packages are skipped, so a
# retry only fetches the gaps. Integrity is not weakened -- every artifact is
# hashed and checked against the pins afterwards regardless of attempt count.
FETCH_OK=0
for attempt in 1 2 3; do
  # shellcheck disable=SC2086
  if apk --root "${TARGET}" --arch "${ARCH}" fetch --recursive \
       --output "${REPO}/${ARCH}" $PKGS > "${OUT}/fetch.log" 2>&1; then
    FETCH_OK=1
    break
  fi
  echo "[rootfs:${ARCH}] fetch attempt ${attempt} failed; retrying" >&2
  grep -i error "${OUT}/fetch.log" | head -5 >&2 || true
  sleep 5
done
if [ "${FETCH_OK}" != "1" ]; then
  echo "[rootfs:${ARCH}] fetch failed after 3 attempts" >&2
  tail -20 "${OUT}/fetch.log" >&2
  exit 1
fi

# Alpine's published per-arch index rewrites noarch packages onto the concrete
# arch and serves them from <repo>/<arch>/; there is no <repo>/noarch/ on the
# CDN, it 404s. Plain `apk index` preserves each .apk's own `A:noarch`, which
# sends the installer looking in a directory that does not exist and fails every
# pure-Python package with "package mentioned in index not found".
echo "[rootfs:${ARCH}] indexing offline repo"
( cd "${REPO}/${ARCH}" && apk index --rewrite-arch "${ARCH}" -o APKINDEX.tar.gz ./*.apk >/dev/null 2>&1 )

echo "[rootfs:${ARCH}] installing closure offline (--no-network)"
# shellcheck disable=SC2086
apk --root "${TARGET}" --arch "${ARCH}" add \
    --no-network --allow-untrusted --repository "${REPO}" $PKGS \
    > "${OUT}/install.log" 2>&1 || {
  echo "[rootfs:${ARCH}] offline install failed" >&2; grep -i error "${OUT}/install.log" | head -20 >&2; exit 1; }
if grep -qi '^ERROR' "${OUT}/install.log"; then
  echo "[rootfs:${ARCH}] offline install reported errors" >&2
  grep -i '^ERROR' "${OUT}/install.log" | head -20 >&2
  exit 1
fi

echo "[rootfs:${ARCH}] configuring guest shell"
# The vendored OpenMinis rootfs script appends an unconditional `cd ~` to
# /etc/profile, which silently discards the cwd every `sh -lc` invocation asks
# for -- including the local-app build directory. We build our own rootfs, so we
# author the profile instead of inheriting that. Nothing here changes directory.
cat > "${TARGET}/etc/profile" <<'PROFILE'
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export HOME=${HOME:-/root}
export TMPDIR=${TMPDIR:-/tmp}
export SSL_CERT_FILE=${SSL_CERT_FILE:-/etc/ssl/cert.pem}
export SSL_CERT_DIR=${SSL_CERT_DIR:-/etc/ssl/certs}
export GIT_SSL_CAINFO=${GIT_SSL_CAINFO:-/etc/ssl/cert.pem}
export NPM_CONFIG_CACHE=${NPM_CONFIG_CACHE:-$HOME/.npm}
export PIP_CACHE_DIR=${PIP_CACHE_DIR:-$HOME/.cache/pip}
export XDG_CACHE_HOME=${XDG_CACHE_HOME:-$HOME/.cache}
export PS1='\w \$ '
PROFILE

# Root's shell, and the directories the runtime expects to exist.
sed -i 's|^root:.*|root:x:0:0:root:/root:/bin/sh|' "${TARGET}/etc/passwd"
mkdir -p "${TARGET}/dev" "${TARGET}/proc" "${TARGET}/sys" "${TARGET}/run" \
         "${TARGET}/tmp" "${TARGET}/var/tmp" "${TARGET}/root" "${TARGET}/home" \
         "${TARGET}/workspace" "${TARGET}/var/lingxi" "${TARGET}/opt/lingxi"
chmod 1777 "${TARGET}/tmp" "${TARGET}/var/tmp"

# resolv.conf is replaced at boot by the native bridge's refreshDns(); ship a
# resolvable default so a first command before the first path update still works.
printf 'nameserver 1.1.1.1\nnameserver 8.8.8.8\n' > "${TARGET}/etc/resolv.conf"

echo "[rootfs:${ARCH}] rewriting absolute symlinks as relative"
# ca-certificates (and a few others) install links like
#   /etc/ssl/certs/ca-cert-X.pem -> /usr/share/ca-certificates/mozilla/X.crt
# An absolute target is interpreted against the HOST root by anything reading
# the tree outside the guest, so the rootfs policy forbids them outright and
# rootfs_tool.py rejects the archive. Rewrite each one relative to its own
# directory, which resolves identically inside the guest and survives packaging.
python3 - "${TARGET}" <<'PY'
import os
import pathlib
import sys

root = pathlib.Path(sys.argv[1]).resolve()
rewritten = 0
for dirpath, dirnames, filenames in os.walk(root):
    for name in list(dirnames) + list(filenames):
        path = pathlib.Path(dirpath) / name
        if not path.is_symlink():
            continue
        target = os.readlink(path)
        if not target.startswith("/"):
            continue
        destination = root / target.lstrip("/")
        # A link escaping the rootfs cannot be made relative safely; fail rather
        # than silently emit a `../..` chain that climbs out of the guest.
        try:
            destination.resolve().relative_to(root)
        except ValueError:
            raise SystemExit(f"absolute symlink escapes the rootfs: {path} -> {target}")
        relative = os.path.relpath(destination, path.parent)
        os.remove(path)
        os.symlink(relative, path)
        rewritten += 1
print(f"   rewrote {rewritten} absolute symlinks")
PY

echo "[rootfs:${ARCH}] verifying required binaries"
MISSING=""
for f in usr/bin/node usr/bin/npm usr/bin/npx usr/bin/git usr/bin/python3 usr/bin/pip3 usr/bin/virtualenv usr/bin/ssh; do
  [ -e "${TARGET}/$f" ] || [ -L "${TARGET}/$f" ] || MISSING="${MISSING} $f"
done
if [ -n "${MISSING}" ]; then
  echo "[rootfs:${ARCH}] required binaries missing:${MISSING}" >&2
  exit 1
fi

apk --root "${TARGET}" info -v 2>/dev/null | sort > "${OUT}/installed.txt"

echo "[rootfs:${ARCH}] emitting closure manifest"
python3 - "${ARCH}" "${ALPINE_BRANCH}" "${CDN}" "${REPO}/${ARCH}" "${OUT}/closure.json" "${PKGS}" <<'PY'
import hashlib, json, pathlib, subprocess, sys

arch, branch, cdn, repo_dir, out_path, pkgs = sys.argv[1:7]
repo_dir = pathlib.Path(repo_dir)
primary = {spec.split("=", 1)[0] for spec in pkgs.split()}

# Which upstream repository serves each package. Recorded per artifact so a
# later refresh cannot silently move a package between main and community
# without the pins showing it.
origin = {}
for section in ("main", "community"):
    idx = pathlib.Path(f"/tmp/idx-{section}")
    idx.mkdir(parents=True, exist_ok=True)
    url = f"{cdn}/{branch}/{section}/{arch}/APKINDEX.tar.gz"
    tarball = idx / "APKINDEX.tar.gz"
    subprocess.run(["curl", "-sSfL", "-o", str(tarball), url], check=True)
    subprocess.run(["tar", "-xzf", str(tarball), "-C", str(idx), "APKINDEX"], check=True)
    text = (idx / "APKINDEX").read_text(encoding="utf-8", errors="replace")
    for block in text.split("\n\n"):
        fields = dict(
            line.split(":", 1) for line in block.splitlines() if ":" in line
        )
        name, version = fields.get("P"), fields.get("V")
        if name and version:
            origin.setdefault((name, version), section)

artifacts = []
for apk in sorted(repo_dir.glob("*.apk")):
    digest = hashlib.sha256(apk.read_bytes()).hexdigest()
    stem = apk.name[: -len(".apk")]
    name, _, version = stem.rpartition("-")
    name, _, release = name.rpartition("-")
    version = f"{release}-{version}"
    section = origin.get((name, version))
    if section is None:
        print(f"cannot attribute {apk.name} to main or community", file=sys.stderr)
        raise SystemExit(1)
    artifacts.append({
        "name": name,
        "version": version,
        "role": "primary" if name in primary else "transitive",
        "repository": section,
        "arch": arch,
        "url": f"{cdn}/{branch}/{section}/{arch}/{apk.name}",
        "sha256": digest,
        "availability": "available",
    })

pathlib.Path(out_path).write_text(
    json.dumps({"arch": arch, "artifacts": artifacts}, indent=2, sort_keys=True) + "\n",
    encoding="utf-8",
)
print(f"   {len(artifacts)} artifacts")
PY

echo "[rootfs:${ARCH}] packing rootfs tarball"
tar -czf "${OUT}/rootfs.tar.gz" -C "${TARGET}" .
sha256sum "${OUT}/rootfs.tar.gz" | awk '{print "[rootfs] tarball sha256="$1}'
du -sh "${TARGET}" | awk '{print "[rootfs] installed size "$1}'
echo "[rootfs:${ARCH}] done"
