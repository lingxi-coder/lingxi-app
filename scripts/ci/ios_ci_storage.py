#!/usr/bin/env python3
"""Own and release only this iOS CI job's temporary build intermediates."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile


MARKER = ".lingxi-ios-ci-storage.json"
ARCHIVE = "libios_framework.a"


def context() -> dict[str, str]:
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("GITHUB_JOB") != "ios":
        raise ValueError("iOS storage cleanup is restricted to the GitHub Actions ios job")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "")
    if not re.fullmatch(r"[0-9]+", run_id) or not re.fullmatch(r"[0-9]+", attempt):
        raise ValueError("missing or invalid GitHub run identity")
    temp = Path(os.environ["RUNNER_TEMP"])
    workspace = Path(os.environ["GITHUB_WORKSPACE"])
    if not temp.is_absolute() or not workspace.is_absolute():
        raise ValueError("runner temporary directory and workspace must be absolute")
    temp = temp.resolve(strict=True)
    workspace = workspace.resolve(strict=True)
    if not temp.is_dir() or not workspace.is_dir():
        raise ValueError("runner temporary directory and workspace must be directories")
    if temp.is_relative_to(workspace) or workspace.is_relative_to(temp):
        raise ValueError("runner temporary directory must be separate from the workspace")
    if any("\n" in str(path) or "\r" in str(path) for path in (temp, workspace)):
        raise ValueError("runner paths cannot contain newlines")
    return {"run_id": run_id, "attempt": attempt, "temp": str(temp), "workspace": str(workspace)}


def prefix(ctx: dict[str, str]) -> str:
    return f"lingxi-ios-cargo-{ctx['run_id']}-{ctx['attempt']}-"


def disk(label: str, path: Path) -> None:
    usage = shutil.disk_usage(path)
    print(f"{label}: {path}: free={usage.free} bytes total={usage.total} bytes", file=sys.stderr)


def size(label: str, path: Path) -> None:
    if not path.exists():
        print(f"{label}: absent: {path}", file=sys.stderr)
        return
    # du reports allocated KiB, which can overcount shared APFS clones. The
    # before/after filesystem-free values are the observed reclamation.
    result = subprocess.run(["du", "-sk", str(path)], check=True, capture_output=True, text=True)
    print(f"{label}: allocated KiB (may include shared blocks): {result.stdout.strip()}", file=sys.stderr)


def prepare(ctx: dict[str, str]) -> Path:
    temp = Path(ctx["temp"])
    disk("before iOS build", temp)
    disk("before iOS build", Path(ctx["workspace"]))
    target = Path(tempfile.mkdtemp(prefix=prefix(ctx), dir=temp))
    target_stat = target.stat()
    marker = {**ctx, "target": str(target), "device": target_stat.st_dev, "inode": target_stat.st_ino}
    (target / MARKER).write_text(json.dumps(marker, sort_keys=True) + "\n")
    return target


def owned_target(ctx: dict[str, str], argument: str) -> tuple[Path, os.stat_result]:
    target = Path(argument)
    if not target.is_absolute() or target.is_symlink() or not target.is_dir():
        raise ValueError("iOS build target must be a real absolute directory")
    target = target.resolve(strict=True)
    if target.parent != Path(ctx["temp"]) or not target.name.startswith(prefix(ctx)):
        raise ValueError("iOS build target is not a job-owned direct child of RUNNER_TEMP")
    target_stat = target.stat()
    if target_stat.st_uid != os.geteuid():
        raise ValueError("iOS build target is not owned by this runner user")
    marker = target / MARKER
    if marker.is_symlink() or not marker.is_file():
        raise ValueError("iOS build ownership marker is missing or not a regular file")
    expected = {**ctx, "target": str(target), "device": target_stat.st_dev, "inode": target_stat.st_ino}
    if json.loads(marker.read_text()) != expected:
        raise ValueError("iOS build ownership marker does not match this job")
    return target, target_stat


def plain_file(path: Path) -> None:
    if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
        raise ValueError(f"required nonempty file is missing or is a symlink: {path}")


def protected_directory(path: Path, workspace: Path, target: Path) -> Path:
    if path.is_symlink() or not path.is_dir():
        raise ValueError(f"required input directory is missing or is a symlink: {path}")
    root = path.resolve(strict=True)
    if not root.is_relative_to(workspace) or root.is_relative_to(target):
        raise ValueError(f"input directory is outside the workspace or inside the build target: {path}")
    return root


def protected_tree(path: Path, workspace: Path, target: Path) -> None:
    root = protected_directory(path, workspace, target)
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            entry = Path(directory) / name
            if entry.is_symlink():
                resolved = entry.resolve(strict=True)
                # Internal framework links are normal; an external link may
                # hide a dependency on the directory about to be removed.
                if not resolved.is_relative_to(root) or resolved.is_relative_to(target):
                    raise ValueError(f"input symlink escapes its preserved tree: {entry}")


def digest(path: Path) -> str:
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def packaged_archive(ffi: Path, entry: dict) -> Path:
    identifier = entry.get("LibraryIdentifier")
    library = entry.get("LibraryPath")
    if not isinstance(identifier, str) or not isinstance(library, str):
        raise ValueError("XCFramework slice has invalid library paths")
    path = ffi / identifier / library
    if not path.resolve(strict=True).is_relative_to(ffi.resolve(strict=True)):
        raise ValueError("XCFramework library path escapes its framework")
    plain_file(path)
    return path


def verify_inputs(workspace: Path, target: Path) -> dict[Path, str]:
    ios = workspace / "apps/ios/native"
    generated = ios / "Generated"
    frameworks = ios / "Frameworks"
    rootfs = ios / "build/linux-runtime/openminis"
    for tree in (generated, frameworks):
        protected_tree(tree, workspace, target)
    rootfs_root = protected_directory(rootfs, workspace, target)
    resources = protected_directory(rootfs / "resources", workspace, target)
    if not resources.is_relative_to(rootfs_root):
        raise ValueError("staged rootfs resources escape their root")
    # The unpacked guest rootfs can contain guest symlinks. Xcode consumes the
    # zip and these explicitly staged resources, not that unpacked tree.
    protected_tree(rootfs / "resources/RootfsPatch.bundle", workspace, target)
    default_mount = rootfs / "resources/default_mount"
    if default_mount.exists() or default_mount.is_symlink():
        protected_tree(default_mount, workspace, target)

    generated_names = ("client.swift", "harness_runtime.swift", "LingxiCodeBindings.swift", "module.modulemap")
    witnesses: dict[Path, str] = {}
    for name in generated_names:
        path = generated / name
        plain_file(path)
        witnesses[path] = digest(path)
    headers = sorted(generated.glob("*FFI.h"))
    if not headers:
        raise ValueError("generated UniFFI headers are missing")
    for path in headers:
        plain_file(path)
        witnesses[path] = digest(path)

    for name in ("LingxiCodeFFI", "GStreamer", "MobileLinuxNativeSupport", "sherpa-onnx", "onnxruntime"):
        metadata = frameworks / f"{name}.xcframework" / "Info.plist"
        plain_file(metadata)
        witnesses[metadata] = digest(metadata)
    rootfs_manifest = rootfs / "manifest.json"
    plain_file(rootfs_manifest)
    witnesses[rootfs_manifest] = digest(rootfs_manifest)
    plain_file(rootfs / "resources/alpine-rootfs.zip")
    vdso = rootfs / "resources/libvdso.so.elf"
    if vdso.exists() or vdso.is_symlink():
        plain_file(vdso)
        witnesses[vdso] = digest(vdso)

    ffi = frameworks / "LingxiCodeFFI.xcframework"
    info = plistlib.loads((ffi / "Info.plist").read_bytes())
    libraries = info.get("AvailableLibraries", [])
    if not isinstance(libraries, list) or len(libraries) != 2:
        raise ValueError("expected exactly one device and one simulator XCFramework slice")
    device = [entry for entry in libraries if entry.get("SupportedPlatform") == "ios"
              and not entry.get("SupportedPlatformVariant")
              and set(entry.get("SupportedArchitectures", [])) == {"arm64"}]
    simulator = [entry for entry in libraries if entry.get("SupportedPlatform") == "ios"
                 and entry.get("SupportedPlatformVariant") == "simulator"
                 and set(entry.get("SupportedArchitectures", [])) == {"arm64", "x86_64"}]
    if len(device) != 1 or len(simulator) != 1:
        raise ValueError("XCFramework is missing the device or both simulator architectures")

    for entry, source in (
        (device[0], target / "xcframework-work/aarch64-apple-ios" / ARCHIVE),
        (simulator[0], target / "xcframework-work" / ARCHIVE),
    ):
        plain_file(source)
        if not source.resolve(strict=True).is_relative_to(target):
            raise ValueError(f"intermediate archive escapes its job-owned build target: {source}")
        packaged = packaged_archive(ffi, entry)
        packaged_digest = digest(packaged)
        if packaged_digest != digest(source):
            raise ValueError(f"packaged XCFramework archive differs from its build input: {packaged}")
        witnesses[packaged] = packaged_digest
        headers_path = entry.get("HeadersPath")
        if not isinstance(headers_path, str):
            raise ValueError("XCFramework slice is missing its generated headers")
        slice_headers = (ffi / entry["LibraryIdentifier"] / headers_path).resolve(strict=True)
        if not slice_headers.is_relative_to(ffi.resolve(strict=True)):
            raise ValueError("XCFramework headers escape its framework")
        for name in ["module.modulemap", *(header.name for header in headers)]:
            packaged_header = slice_headers / name
            plain_file(packaged_header)
            if digest(packaged_header) != witnesses[generated / name]:
                raise ValueError(f"packaged header differs from generated binding: {packaged_header}")
    return witnesses


def same_directory(left: os.stat_result, right: os.stat_result) -> bool:
    return (left.st_dev, left.st_ino) == (right.st_dev, right.st_ino)


def remove_owned_directory(parent_fd: int, name: str, expected: os.stat_result) -> None:
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    child_fd = os.open(name, flags, dir_fd=parent_fd)
    try:
        if not same_directory(expected, os.fstat(child_fd)):
            raise ValueError(f"iOS build subdirectory changed before it was opened: {name}")
        remove_directory_contents(child_fd)
        current = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
        if not same_directory(expected, current):
            raise ValueError(f"iOS build subdirectory changed during cleanup: {name}")
        # rmdir never follows a symlink and refuses a nonempty replacement.
        os.rmdir(name, dir_fd=parent_fd)
    finally:
        os.close(child_fd)


def remove_directory_contents(target_fd: int, *, preserve_marker: bool = False) -> None:
    # The completed CI build must not have concurrent writers. POSIX has no
    # atomic inode-checked unlink, so this is not a boundary against a hostile
    # process running as the same user that can replace entries in this tree.
    with os.scandir(target_fd) as entries:
        snapshot = [(entry.name, entry.stat(follow_symlinks=False)) for entry in entries
                    if not (preserve_marker and entry.name == MARKER)]
    for name, expected in snapshot:
        if stat.S_ISDIR(expected.st_mode):
            remove_owned_directory(target_fd, name, expected)
        else:
            current = os.stat(name, dir_fd=target_fd, follow_symlinks=False)
            if not same_directory(expected, current):
                raise ValueError(f"iOS build entry changed during cleanup: {name}")
            os.unlink(name, dir_fd=target_fd)


def remove_owned_contents(target_fd: int) -> None:
    # All removal is relative to the opened, verified directory, even if its
    # pathname is renamed or replaced while cleanup is in progress.
    remove_directory_contents(target_fd, preserve_marker=True)
    with os.scandir(target_fd) as entries:
        if any(entry.name != MARKER for entry in entries):
            raise ValueError("iOS build target gained entries during cleanup")


def release(ctx: dict[str, str], argument: str) -> None:
    target, target_stat = owned_target(ctx, argument)
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    parent_fd = os.open(target.parent, flags)
    try:
        target_fd = os.open(target.name, flags, dir_fd=parent_fd)
        try:
            if not same_directory(target_stat, os.fstat(target_fd)):
                raise ValueError("iOS build target changed before it was opened")
            workspace = Path(ctx["workspace"])
            witnesses = verify_inputs(workspace, target)
            rootfs_archive = workspace / "apps/ios/native/build/linux-runtime/openminis/resources/alpine-rootfs.zip"
            rootfs_stat = rootfs_archive.stat()
            disk("before iOS intermediate cleanup", workspace)
            disk("before iOS intermediate cleanup", Path(ctx["temp"]))
            size("iOS build intermediates", target)
            if not all(function in os.supports_dir_fd for function in (os.open, os.stat, os.unlink, os.rmdir)) or os.scandir not in os.supports_fd:
                raise ValueError("runner Python does not support descriptor-relative iOS build cleanup")
            _, current_stat = owned_target(ctx, argument)
            if not same_directory(target_stat, current_stat):
                raise ValueError("iOS build target changed during verification")
            remove_owned_contents(target_fd)
            _, current_stat = owned_target(ctx, argument)
            if not same_directory(target_stat, current_stat):
                raise ValueError("iOS build target changed during cleanup")
        finally:
            os.close(target_fd)
    finally:
        os.close(parent_fd)
    # Keep the empty directory and its ownership marker. Checking a path and
    # then rmdir-ing it cannot atomically prevent removal of a replacement.
    for path, expected in witnesses.items():
        plain_file(path)
        if digest(path) != expected:
            raise ValueError(f"preserved iOS build input changed during cleanup: {path}")
    plain_file(rootfs_archive)
    after_rootfs_stat = rootfs_archive.stat()
    if (rootfs_stat.st_dev, rootfs_stat.st_ino, rootfs_stat.st_size, rootfs_stat.st_mtime_ns) != (
        after_rootfs_stat.st_dev, after_rootfs_stat.st_ino, after_rootfs_stat.st_size, after_rootfs_stat.st_mtime_ns
    ):
        raise ValueError("staged iOS rootfs changed during cleanup")
    disk("after iOS intermediate cleanup", workspace)
    disk("after iOS intermediate cleanup", Path(ctx["temp"]))
    print("Released only the verified job-owned iOS build intermediates.", file=sys.stderr)


def report(ctx: dict[str, str], argument: str) -> None:
    workspace = Path(ctx["workspace"])
    disk("iOS job workspace", workspace)
    disk("iOS job runner temp", Path(ctx["temp"]))
    target = Path(argument)
    if target.is_absolute() and target.parent == Path(ctx["temp"]) and target.name.startswith(prefix(ctx)):
        size("iOS build intermediates", target)
    for label, path in (
        ("iOS frameworks", workspace / "apps/ios/native/Frameworks"),
        ("iOS generated bindings", workspace / "apps/ios/native/Generated"),
        ("staged iOS rootfs", workspace / "apps/ios/native/build/linux-runtime/openminis"),
    ):
        size(label, path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("prepare", "release", "report"))
    parser.add_argument("target", nargs="?")
    args = parser.parse_args()
    try:
        ctx = context()
        if args.operation == "prepare":
            if args.target is not None:
                raise ValueError("prepare does not accept a target")
            print(prepare(ctx))
        elif args.target is None:
            raise ValueError(f"{args.operation} requires a target")
        elif args.operation == "release":
            release(ctx, args.target)
        else:
            report(ctx, args.target)
    except (OSError, ValueError, KeyError, plistlib.InvalidFileException, subprocess.CalledProcessError) as error:
        print(f"ios-ci-storage: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
