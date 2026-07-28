#!/usr/bin/env python3
import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import stat
import sys
import tarfile
from dataclasses import dataclass
from typing import Dict, List, Tuple

FIXED_PRIMARY_PACKAGES = [
    "busybox",
    "git",
    "openssh-client",
    "python3",
    "ca-certificates",
]

FORBIDDEN_PACKAGE_MANAGER_PATHS = [
    "/bin/apk",
    "/sbin/apk",
    "/usr/bin/apk",
    "/usr/sbin/apk",
    "/usr/bin/pip",
    "/usr/bin/pip3",
    "/usr/bin/npm",
    "/usr/bin/npx",
]

WELL_KNOWN_REPOSITORY_PATHS = [
    "/etc/apk/repositories",
]

BINARY_SYMLINK_FORBIDDEN_PREFIXES = [
    "/bin/",
    "/sbin/",
    "/usr/bin/",
    "/usr/sbin/",
]

WORLD_WRITABLE_ALLOWED = {"/tmp", "/var/tmp"}
DEFAULT_SOURCE_DATE_EPOCH = 0


@dataclass
class PackageRecord:
    name: str
    version: str
    license: str
    architecture: str
    origin: str


def fail(message: str) -> "None":
    print(message, file=sys.stderr)
    raise SystemExit(1)


def root_rel(path: pathlib.Path, root: pathlib.Path) -> str:
    rel = path.relative_to(root).as_posix()
    return "/" if rel == "." else f"/{rel}"


def safe_member_path(name: str) -> pathlib.PurePosixPath:
    pure = pathlib.PurePosixPath(name)
    if pure.parts and pure.parts[0] == ".":
        pure = pathlib.PurePosixPath(*pure.parts[1:])
    if pure.is_absolute():
        fail(f"archive entry must not be absolute: {name}")
    if any(part in {"", ".", ".."} for part in pure.parts):
        fail(f"archive entry contains unsafe path components: {name}")
    return pure


def resolve_symlink_target(member_name: str, linkname: str) -> pathlib.PurePosixPath:
    if pathlib.PurePosixPath(linkname).is_absolute():
        fail(f"archive link target must be relative: {member_name} -> {linkname}")
    member_parent = pathlib.PurePosixPath(member_name).parent
    resolved = member_parent.joinpath(linkname)
    normalized_parts: List[str] = []
    for part in resolved.parts:
        if part in {"", "."}:
            continue
        if part == "..":
            if not normalized_parts:
                fail(f"archive link escapes root: {member_name} -> {linkname}")
            normalized_parts.pop()
            continue
        normalized_parts.append(part)
    if not normalized_parts:
        fail(f"archive link resolves to root: {member_name} -> {linkname}")
    return pathlib.PurePosixPath(*normalized_parts)


def normalize_hardlink_target(member_name: str, linkname: str) -> pathlib.PurePosixPath:
    if pathlib.PurePosixPath(linkname).is_absolute():
        fail(f"archive hardlink target must not be absolute: {member_name} -> {linkname}")
    return safe_member_path(linkname)


def read_sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def stable_json_dumps(data: object) -> str:
    return json.dumps(data, indent=2, sort_keys=False) + "\n"


def canonical_json_bytes(data: object) -> bytes:
    return json.dumps(
        data,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")


def source_date_epoch(value: int | None) -> int:
    if value is not None:
        return value
    env_value = os.environ.get("SOURCE_DATE_EPOCH")
    if env_value is None or env_value == "":
        return DEFAULT_SOURCE_DATE_EPOCH
    try:
        return int(env_value)
    except ValueError as exc:
        fail(f"SOURCE_DATE_EPOCH must be an integer: {exc}")


def format_created_timestamp(epoch: int) -> str:
    return dt.datetime.fromtimestamp(epoch, tz=dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def normalized_tar_mode(mode: int, *, is_dir: bool, is_symlink: bool) -> int:
    if is_symlink:
        return 0o777
    permission_bits = stat.S_IMODE(mode)
    if is_dir:
        if permission_bits & stat.S_IWOTH:
            return 0o1777 if permission_bits & stat.S_ISVTX else 0o777
        return 0o755
    if permission_bits & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH):
        return 0o755
    return 0o644


def parse_apk_installed(installed_path: pathlib.Path) -> List[PackageRecord]:
    if not installed_path.is_file():
        fail(f"missing APK installed database: {installed_path}")
    packages: List[PackageRecord] = []
    fields: Dict[str, str] = {}
    for line in installed_path.read_text(encoding="utf-8").splitlines():
        if not line:
            if fields:
                packages.append(
                    PackageRecord(
                        name=fields.get("P", ""),
                        version=fields.get("V", ""),
                        license=fields.get("L", "NOASSERTION") or "NOASSERTION",
                        architecture=fields.get("A", "unknown") or "unknown",
                        origin=fields.get("o", fields.get("P", "")) or fields.get("P", ""),
                    )
                )
                fields = {}
            continue
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        fields[key] = value
    if fields:
        packages.append(
            PackageRecord(
                name=fields.get("P", ""),
                version=fields.get("V", ""),
                license=fields.get("L", "NOASSERTION") or "NOASSERTION",
                architecture=fields.get("A", "unknown") or "unknown",
                origin=fields.get("o", fields.get("P", "")) or fields.get("P", ""),
            )
        )
    if not packages:
        fail(f"no packages found in APK installed database: {installed_path}")
    names = {package.name for package in packages}
    missing = sorted(set(FIXED_PRIMARY_PACKAGES) - names)
    if missing:
        fail(f"fixed primary packages missing from APK installed database: {missing}")
    forbidden = sorted(names & {"apk-tools", "py3-pip", "nodejs", "npm"})
    if forbidden:
        fail(f"forbidden package-manager packages present in rootfs: {forbidden}")
    return sorted(packages, key=lambda package: package.name)


def is_elf(path: pathlib.Path) -> bool:
    try:
        with path.open("rb") as handle:
            return handle.read(4) == b"\x7fELF"
    except OSError as exc:
        fail(f"failed to read file signature {path}: {exc}")


def classify_elf(path_rel: str) -> str:
    basename = pathlib.PurePosixPath(path_rel).name
    if ".so" in basename:
        return "shared-library"
    if path_rel in {"/bin/busybox", "/bin/sh", "/usr/bin/python3"}:
        return "interpreter"
    return "elf"


def validate_rootfs_tree(root: pathlib.Path) -> List[PackageRecord]:
    if not root.is_dir():
        fail(f"rootfs directory not found: {root}")

    busybox = root / "bin" / "busybox"
    sh_path = root / "bin" / "sh"
    if not busybox.is_file() or busybox.is_symlink():
        fail("rootfs must contain a real /bin/busybox file")
    if not sh_path.exists():
        fail("rootfs must contain /bin/sh")
    if sh_path.is_symlink():
        fail("BusyBox applets must use hardlinks, not symlinks: /bin/sh is a symlink")
    if os.stat(busybox).st_ino != os.stat(sh_path).st_ino:
        fail("/bin/sh must be a hardlink to /bin/busybox")

    for forbidden in FORBIDDEN_PACKAGE_MANAGER_PATHS + WELL_KNOWN_REPOSITORY_PATHS:
        candidate = root / forbidden.lstrip("/")
        if candidate.exists() or candidate.is_symlink():
            fail(f"forbidden package-manager artifact present in rootfs: {forbidden}")

    for current_root, dirnames, filenames in os.walk(root, topdown=True, followlinks=False):
        current_dir = pathlib.Path(current_root)
        dirnames.sort()
        filenames.sort()

        for dirname in list(dirnames):
            path = current_dir / dirname
            rel = root_rel(path, root)
            mode = os.lstat(path).st_mode
            if stat.S_ISLNK(mode):
                if any(rel.startswith(prefix) for prefix in BINARY_SYMLINK_FORBIDDEN_PREFIXES):
                    fail(f"symlinks are forbidden in binary directories: {rel}")
                target = os.readlink(path)
                resolve_symlink_target(rel.lstrip("/"), target)
                dirnames.remove(dirname)
                continue
            if stat.S_ISSOCK(mode) or stat.S_ISCHR(mode) or stat.S_ISBLK(mode) or stat.S_ISFIFO(mode):
                fail(f"forbidden special file in rootfs: {rel}")
            if mode & stat.S_ISUID or mode & stat.S_ISGID:
                fail(f"suid/sgid bits are forbidden in rootfs directories: {rel}")
            if mode & stat.S_IWOTH and rel not in WORLD_WRITABLE_ALLOWED:
                fail(f"world-writable directory is forbidden outside scratch paths: {rel}")

        for filename in filenames:
            path = current_dir / filename
            rel = root_rel(path, root)
            mode = os.lstat(path).st_mode
            if stat.S_ISLNK(mode):
                if any(rel.startswith(prefix) for prefix in BINARY_SYMLINK_FORBIDDEN_PREFIXES):
                    fail(f"symlinks are forbidden in binary directories: {rel}")
                target = os.readlink(path)
                resolve_symlink_target(rel.lstrip("/"), target)
                continue
            if not stat.S_ISREG(mode):
                fail(f"non-regular rootfs file is forbidden: {rel}")
            if mode & stat.S_ISUID or mode & stat.S_ISGID:
                fail(f"suid/sgid bits are forbidden in rootfs files: {rel}")
            if mode & stat.S_IWOTH and rel not in WORLD_WRITABLE_ALLOWED:
                fail(f"world-writable file is forbidden outside scratch paths: {rel}")

    return parse_apk_installed(root / "lib" / "apk" / "db" / "installed")


def collect_allowlist(root: pathlib.Path) -> List[dict]:
    entries: List[dict] = []
    seen: set[str] = set()
    for current_root, _, filenames in os.walk(root, topdown=True, followlinks=False):
        current_dir = pathlib.Path(current_root)
        filenames.sort()
        for filename in filenames:
            path = current_dir / filename
            rel = root_rel(path, root)
            if path.is_symlink():
                continue
            mode = os.stat(path).st_mode
            if not stat.S_ISREG(mode):
                continue
            if not is_elf(path):
                continue
            if not ((mode & stat.S_IXUSR) or ".so" in pathlib.PurePosixPath(rel).name):
                continue
            if rel in seen:
                continue
            seen.add(rel)
            entries.append(
                {
                    "path": rel,
                    "sha256": read_sha256(path),
                    "kind": classify_elf(rel),
                    "size_bytes": path.stat().st_size,
                }
            )
    required = {"/bin/busybox", "/bin/sh", "/usr/bin/git", "/usr/bin/ssh", "/usr/bin/python3"}
    present = {entry["path"] for entry in entries}
    missing = sorted(required - present)
    if missing:
        fail(f"required ELF/interpreter paths missing from rootfs allowlist: {missing}")
    return sorted(entries, key=lambda entry: entry["path"])


def generate_manifest(args: argparse.Namespace) -> None:
    root = pathlib.Path(args.root).resolve()
    packages = validate_rootfs_tree(root)
    allowlist = collect_allowlist(root)
    manifest = {
        "schema_version": 1,
        "runtime": args.runtime,
        "platform": args.platform,
        "abi": args.abi,
        "rootfs_version": args.rootfs_version,
        "archive": {
            "filename": args.archive_filename,
            "sha256": args.archive_sha256,
            "size_bytes": args.archive_size,
        },
        "packages": [
            {"name": package.name, "version": package.version}
            for package in packages
        ],
        "executable_allowlist": allowlist,
        "writable_paths": ["/root", "/tmp", "/var/tmp", "/workspace"],
    }
    output = pathlib.Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(stable_json_dumps(manifest), encoding="utf-8")


def generate_spdx(args: argparse.Namespace) -> None:
    root = pathlib.Path(args.root).resolve()
    packages = validate_rootfs_tree(root)
    created = format_created_timestamp(source_date_epoch(args.source_date_epoch))
    package_payload = [
        {
            "name": package.name,
            "version": package.version,
            "license": package.license,
            "architecture": package.architecture,
            "origin": package.origin,
        }
        for package in packages
    ]
    namespace_hash = hashlib.sha256(canonical_json_bytes(package_payload)).hexdigest()
    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": args.name,
        "documentNamespace": f"https://lingxi-code/mobile-linux/spdx/{args.name}/{namespace_hash}",
        "creationInfo": {
            "created": created,
            "creators": ["Tool: lingxi-code/scripts/mobile-linux/rootfs_tool.py"],
        },
        "packages": [
            {
                "name": package.name,
                "SPDXID": f"SPDXRef-Package-{package.name}",
                "versionInfo": package.version,
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": package.license,
                "licenseDeclared": package.license,
                "supplier": "NOASSERTION",
                "originator": package.origin or "NOASSERTION",
            }
            for package in packages
        ],
    }
    output = pathlib.Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(stable_json_dumps(document), encoding="utf-8")


def generate_lock(args: argparse.Namespace) -> None:
    root = pathlib.Path(args.root).resolve()
    packages = validate_rootfs_tree(root)
    lock = {
        "schema_version": 1,
        "alpine": {
            "version": "3.24.1",
            "branch": "v3.24",
            "repositories": [
                "https://dl-cdn.alpinelinux.org/alpine/v3.24/main",
                "https://dl-cdn.alpinelinux.org/alpine/v3.24/community",
            ],
        },
        "policy": {
            "archive_format": "tar.zst",
            "busybox_applet_strategy": "hardlink",
            "apk_disabled": True,
            "forbidden_package_manager_paths": FORBIDDEN_PACKAGE_MANAGER_PATHS,
            "fixed_primary_packages": FIXED_PRIMARY_PACKAGES,
        },
        "resolved_packages": [
            {
                "name": package.name,
                "version": package.version,
                "license": package.license,
                "architecture": package.architecture,
                "origin": package.origin,
            }
            for package in packages
        ],
    }
    output = pathlib.Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(stable_json_dumps(lock), encoding="utf-8")


def snapshot_allowlist(args: argparse.Namespace) -> None:
    manifest = json.loads(pathlib.Path(args.manifest).read_text(encoding="utf-8"))
    if not isinstance(manifest.get("executable_allowlist"), list):
        fail("manifest missing executable_allowlist")
    snapshot = {
        "schema_version": 1,
        "entries": manifest["executable_allowlist"],
    }
    output = pathlib.Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(stable_json_dumps(snapshot), encoding="utf-8")


def validate_lock(args: argparse.Namespace) -> None:
    lock = json.loads(pathlib.Path(args.lock).read_text(encoding="utf-8"))
    manifest = json.loads(pathlib.Path(args.manifest).read_text(encoding="utf-8"))
    if lock.get("schema_version") != 1:
        fail("rootfs lock schema_version must be 1")
    alpine = lock.get("alpine")
    if not isinstance(alpine, dict):
        fail("rootfs lock missing alpine block")
    if alpine.get("version") != "3.24.1" or alpine.get("branch") != "v3.24":
        fail("rootfs lock must pin Alpine 3.24.1 / v3.24")
    policy = lock.get("policy")
    if not isinstance(policy, dict):
        fail("rootfs lock missing policy block")
    if policy.get("archive_format") != "tar.zst":
        fail("rootfs lock must pin tar.zst archive format")
    if policy.get("busybox_applet_strategy") != "hardlink":
        fail("rootfs lock must require hardlink BusyBox applets")
    if policy.get("apk_disabled") is not True:
        fail("rootfs lock must require apk_disabled=true")
    if policy.get("fixed_primary_packages") != FIXED_PRIMARY_PACKAGES:
        fail("rootfs lock fixed_primary_packages diverged")
    if policy.get("forbidden_package_manager_paths") != FORBIDDEN_PACKAGE_MANAGER_PATHS:
        fail("rootfs lock forbidden_package_manager_paths diverged")
    resolved = lock.get("resolved_packages")
    if not isinstance(resolved, list) or not resolved:
        fail("rootfs lock must contain resolved_packages[]")
    resolved_names = {
        entry.get("name")
        for entry in resolved
        if isinstance(entry, dict) and isinstance(entry.get("name"), str)
    }
    manifest_names = {
        entry.get("name")
        for entry in manifest.get("packages", [])
        if isinstance(entry, dict) and isinstance(entry.get("name"), str)
    }
    if manifest_names - resolved_names:
        fail(
            "rootfs lock missing manifest package entries: "
            f"{sorted(manifest_names - resolved_names)}"
        )
    print(f"rootfs build lock verified: {args.lock}")


def verify_archive(args: argparse.Namespace) -> None:
    archive = pathlib.Path(args.archive)
    if not archive.is_file() or archive.is_symlink():
        fail(f"archive not found or unsafe: {archive}")
    seen_paths: set[str] = set()
    hardlinks: List[Tuple[str, str]] = []
    with tarfile.open(archive, mode="r:*") as tar:
        members = tar.getmembers()
        for member in members:
            path = safe_member_path(member.name).as_posix()
            if path in seen_paths:
                fail(f"duplicate archive entry: {path}")
            seen_paths.add(path)
            rel = f"/{path}"
            if member.isdev():
                fail(f"device entries are forbidden in rootfs archive: {rel}")
            if member.isfifo():
                fail(f"FIFO entries are forbidden in rootfs archive: {rel}")
            if member.mode & stat.S_ISUID or member.mode & stat.S_ISGID:
                fail(f"suid/sgid entries are forbidden in rootfs archive: {rel}")
            if rel in FORBIDDEN_PACKAGE_MANAGER_PATHS or rel in WELL_KNOWN_REPOSITORY_PATHS:
                fail(f"forbidden package-manager artifact present in archive: {rel}")
            if member.issym():
                if any(rel.startswith(prefix) for prefix in BINARY_SYMLINK_FORBIDDEN_PREFIXES):
                    fail(f"symlinks are forbidden in binary directories: {rel}")
                resolve_symlink_target(path, member.linkname)
            elif member.islnk():
                normalized_target = normalize_hardlink_target(path, member.linkname).as_posix()
                hardlinks.append((path, normalized_target))
            elif member.isdir():
                if member.mode & stat.S_IWOTH and rel not in WORLD_WRITABLE_ALLOWED:
                    fail(f"world-writable directory is forbidden outside scratch paths: {rel}")
            elif member.isreg():
                if member.mode & stat.S_IWOTH and rel not in WORLD_WRITABLE_ALLOWED:
                    fail(f"world-writable file is forbidden outside scratch paths: {rel}")
            else:
                fail(f"unsupported archive entry type for {rel}")

    if "bin/busybox" not in seen_paths:
        fail("archive missing /bin/busybox")
    if "bin/sh" not in seen_paths:
        fail("archive missing /bin/sh")

    hardlink_pairs = {frozenset((path, link)) for path, link in hardlinks}
    if frozenset(("bin/sh", "bin/busybox")) not in hardlink_pairs:
        fail("/bin/sh and /bin/busybox must be recorded as hardlinked archive entries")

    print(f"archive verified: {archive}")


def build_archive(args: argparse.Namespace) -> None:
    root = pathlib.Path(args.root).resolve()
    output = pathlib.Path(args.output)
    validate_rootfs_tree(root)
    output.parent.mkdir(parents=True, exist_ok=True)
    epoch = source_date_epoch(args.source_date_epoch)

    inode_first_path: Dict[Tuple[int, int], str] = {}
    with tarfile.open(output, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for current_root, dirnames, filenames in os.walk(root, topdown=True, followlinks=False):
            current_dir = pathlib.Path(current_root)
            dirnames.sort()
            filenames.sort()
            dir_rel = root_rel(current_dir, root)
            if dir_rel != "/":
                dir_name = dir_rel.lstrip("/")
                dir_stat = os.lstat(current_dir)
                info = tarfile.TarInfo(dir_name)
                info.type = tarfile.DIRTYPE
                info.mode = normalized_tar_mode(dir_stat.st_mode, is_dir=True, is_symlink=False)
                info.uid = 0
                info.gid = 0
                info.uname = "root"
                info.gname = "root"
                info.mtime = epoch
                tar.addfile(info)

            for dirname in dirnames:
                path = current_dir / dirname
                if path.is_symlink():
                    rel = root_rel(path, root).lstrip("/")
                    target = os.readlink(path)
                    info = tarfile.TarInfo(rel)
                    info.type = tarfile.SYMTYPE
                    info.linkname = target
                    info.mode = normalized_tar_mode(os.lstat(path).st_mode, is_dir=False, is_symlink=True)
                    info.uid = 0
                    info.gid = 0
                    info.uname = "root"
                    info.gname = "root"
                    info.mtime = epoch
                    tar.addfile(info)

            for filename in filenames:
                path = current_dir / filename
                rel = root_rel(path, root).lstrip("/")
                st = os.lstat(path)
                if stat.S_ISLNK(st.st_mode):
                    info = tarfile.TarInfo(rel)
                    info.type = tarfile.SYMTYPE
                    info.linkname = os.readlink(path)
                    info.mode = normalized_tar_mode(st.st_mode, is_dir=False, is_symlink=True)
                    info.uid = 0
                    info.gid = 0
                    info.uname = "root"
                    info.gname = "root"
                    info.mtime = epoch
                    tar.addfile(info)
                    continue

                inode_key = (st.st_dev, st.st_ino)
                if inode_key in inode_first_path:
                    info = tarfile.TarInfo(rel)
                    info.type = tarfile.LNKTYPE
                    info.linkname = inode_first_path[inode_key]
                    info.mode = normalized_tar_mode(st.st_mode, is_dir=False, is_symlink=False)
                    info.uid = 0
                    info.gid = 0
                    info.uname = "root"
                    info.gname = "root"
                    info.mtime = epoch
                    tar.addfile(info)
                    continue

                inode_first_path[inode_key] = rel
                info = tarfile.TarInfo(rel)
                info.size = st.st_size
                info.mode = normalized_tar_mode(st.st_mode, is_dir=False, is_symlink=False)
                info.uid = 0
                info.gid = 0
                info.uname = "root"
                info.gname = "root"
                info.mtime = epoch
                with path.open("rb") as handle:
                    tar.addfile(info, handle)

    print(f"deterministic tar archive built: {output}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    verify_tree_cmd = sub.add_parser("verify-tree")
    verify_tree_cmd.add_argument("--root", required=True)
    verify_tree_cmd.set_defaults(func=lambda args: (validate_rootfs_tree(pathlib.Path(args.root)), print(f"rootfs tree verified: {args.root}")))

    verify_archive_cmd = sub.add_parser("verify-archive")
    verify_archive_cmd.add_argument("--archive", required=True)
    verify_archive_cmd.set_defaults(func=verify_archive)

    build_archive_cmd = sub.add_parser("build-archive")
    build_archive_cmd.add_argument("--root", required=True)
    build_archive_cmd.add_argument("--output", required=True)
    build_archive_cmd.add_argument("--source-date-epoch", type=int)
    build_archive_cmd.set_defaults(func=build_archive)

    manifest_cmd = sub.add_parser("generate-manifest")
    manifest_cmd.add_argument("--root", required=True)
    manifest_cmd.add_argument("--runtime", required=True)
    manifest_cmd.add_argument("--platform", required=True)
    manifest_cmd.add_argument("--abi", required=True)
    manifest_cmd.add_argument("--rootfs-version", required=True)
    manifest_cmd.add_argument("--archive-filename", required=True)
    manifest_cmd.add_argument("--archive-sha256", required=True)
    manifest_cmd.add_argument("--archive-size", required=True, type=int)
    manifest_cmd.add_argument("--output", required=True)
    manifest_cmd.set_defaults(func=generate_manifest)

    spdx_cmd = sub.add_parser("generate-spdx")
    spdx_cmd.add_argument("--root", required=True)
    spdx_cmd.add_argument("--name", required=True)
    spdx_cmd.add_argument("--output", required=True)
    spdx_cmd.add_argument("--source-date-epoch", type=int)
    spdx_cmd.set_defaults(func=generate_spdx)

    lock_cmd = sub.add_parser("generate-lock")
    lock_cmd.add_argument("--root", required=True)
    lock_cmd.add_argument("--output", required=True)
    lock_cmd.set_defaults(func=generate_lock)

    snapshot_cmd = sub.add_parser("snapshot-allowlist")
    snapshot_cmd.add_argument("--manifest", required=True)
    snapshot_cmd.add_argument("--output", required=True)
    snapshot_cmd.set_defaults(func=snapshot_allowlist)

    validate_lock_cmd = sub.add_parser("validate-lock")
    validate_lock_cmd.add_argument("--lock", required=True)
    validate_lock_cmd.add_argument("--manifest", required=True)
    validate_lock_cmd.set_defaults(func=validate_lock)

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
