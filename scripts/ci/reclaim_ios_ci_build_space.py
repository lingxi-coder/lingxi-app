#!/usr/bin/env python3
"""Free only this job's finished Rust and XCFramework assembly intermediates."""
from __future__ import annotations

import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys


class CleanupError(RuntimeError):
    pass


FFI_ARCHIVE = "libios_framework.a"


def _within(path: Path, root: Path) -> bool:
    return path == root or root in path.parents


def _check_links(root: Path) -> None:
    """Reject broken or escaping output links before deleting their neighbors."""
    if root.is_symlink() or not root.is_dir():
        raise CleanupError(f"expected an independent output directory: {root}")
    root = root.resolve(strict=True)
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            path = Path(directory) / name
            if not path.is_symlink():
                continue
            try:
                resolved = path.resolve(strict=True)
            except (OSError, RuntimeError) as error:
                raise CleanupError(f"broken output link: {path}") from error
            if not _within(resolved, root):
                raise CleanupError(f"output link leaves {root}: {path} -> {resolved}")


def _archive_architectures(archive: Path) -> set[str]:
    result = subprocess.run(
        ["xcrun", "lipo", "-archs", str(archive)],
        check=True, capture_output=True, text=True,
    )
    return set(result.stdout.split())


def _read_framework_metadata(framework: Path) -> dict:
    try:
        with (framework / "Info.plist").open("rb") as source:
            metadata = plistlib.load(source)
    except (OSError, plistlib.InvalidFileException) as error:
        raise CleanupError(f"cannot read XCFramework metadata: {framework}") from error
    if not isinstance(metadata, dict) or metadata.get("CFBundlePackageType") != "XFWK":
        raise CleanupError(f"output is not an XCFramework: {framework}")
    libraries = metadata.get("AvailableLibraries")
    if not isinstance(libraries, list) or not libraries or not all(isinstance(entry, dict) for entry in libraries):
        raise CleanupError(f"invalid XCFramework library metadata: {framework}")
    return metadata


def find_ffi_framework(frameworks: Path) -> Path:
    """Find the single staged XCFramework containing the expected Rust FFI ABI."""
    candidates = []
    for framework in sorted(frameworks.glob("*.xcframework")):
        metadata = _read_framework_metadata(framework)
        if any(entry.get("LibraryPath") == FFI_ARCHIVE for entry in metadata["AvailableLibraries"]):
            candidates.append(framework)
    if len(candidates) != 1:
        raise CleanupError(f"expected exactly one FFI XCFramework, found {len(candidates)}")
    return candidates[0]


def validate_framework(framework: Path) -> tuple:
    """Check the assembled device and both simulator architectures for Xcode."""
    libraries = _read_framework_metadata(framework)["AvailableLibraries"]
    fingerprints = []
    for variant, required in ((None, {"arm64"}), ("simulator", {"arm64", "x86_64"})):
        matches = [entry for entry in libraries if entry.get("SupportedPlatform") == "ios"
                   and entry.get("SupportedPlatformVariant") == variant]
        if len(matches) != 1:
            raise CleanupError(f"expected one iOS {variant or 'device'} library")
        entry = matches[0]
        declared = set(entry.get("SupportedArchitectures", []))
        if not required <= declared:
            raise CleanupError(f"iOS {variant or 'device'} metadata is missing {sorted(required - declared)}")
        identifier = entry.get("LibraryIdentifier", "")
        library = entry.get("LibraryPath", "")
        headers = entry.get("HeadersPath", "")
        if library != FFI_ARCHIVE:
            raise CleanupError(f"iOS {variant or 'device'} library does not match the FFI ABI")
        if any(not value or Path(value).name != value or value in (".", "..")
               for value in (identifier, library, headers)):
            raise CleanupError("invalid XCFramework library paths")
        slice_root = framework / identifier
        archive = slice_root / library
        modulemap = slice_root / headers / "module.modulemap"
        if not archive.is_file() or not archive.stat().st_size or not modulemap.is_file():
            raise CleanupError(f"incomplete XCFramework slice: {identifier}")
        if not any((slice_root / headers).glob("*.h")):
            raise CleanupError(f"missing XCFramework headers: {identifier}")
        actual = _archive_architectures(archive)
        if not declared <= actual:
            raise CleanupError(f"XCFramework archive {identifier} is missing {sorted(declared - actual)}")
        fingerprints.append((identifier, archive.stat().st_size, tuple(sorted(actual))))
    return tuple(fingerprints)


def reclaim(repo: Path, runner_temp: Path, run_id: str, run_attempt: str) -> None:
    if not run_id.isascii() or not run_id.isdigit() or not run_attempt.isascii() or not run_attempt.isdigit():
        raise CleanupError("GitHub run ID and attempt must be decimal numbers")
    repo = repo.resolve(strict=True)
    runner_temp = runner_temp.resolve(strict=True)
    cargo_target = runner_temp / f"lingxi-ios-cargo-{run_id}-{run_attempt}"
    scratch = repo / "apps/ios/native/build/xcframework"
    frameworks = repo / "apps/ios/native/Frameworks"
    generated = repo / "apps/ios/native/Generated"
    for path in (cargo_target, scratch):
        if path.is_symlink() or not path.is_dir() or path.resolve(strict=True) != path:
            raise CleanupError(f"cleanup path is not the expected real directory: {path}")
    if _within(cargo_target, repo) or _within(repo, cargo_target):
        raise CleanupError("the private Cargo output must not overlap the repository")
    for output in (frameworks, generated):
        _check_links(output)
    framework = find_ffi_framework(frameworks)
    before = validate_framework(framework)

    print("Disk usage before reclaiming completed iOS intermediates:", flush=True)
    subprocess.run(["df", "-h", str(repo)], check=True)
    subprocess.run(["du", "-sh", str(cargo_target), str(scratch)], check=True)
    for path in (cargo_target, scratch):
        shutil.rmtree(path)
        if path.exists() or path.is_symlink():
            raise CleanupError(f"failed to remove intermediate directory: {path}")

    for output in (frameworks, generated):
        _check_links(output)
    if find_ffi_framework(frameworks) != framework or validate_framework(framework) != before:
        raise CleanupError("XCFramework changed after reclaiming intermediates")
    print("Disk usage after reclaiming completed iOS intermediates:", flush=True)
    subprocess.run(["df", "-h", str(repo)], check=True)


if __name__ == "__main__":
    try:
        reclaim(
            Path(__file__).resolve().parents[2], Path(os.environ["RUNNER_TEMP"]),
            os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_RUN_ATTEMPT"],
        )
    except (CleanupError, OSError, KeyError, plistlib.InvalidFileException, subprocess.CalledProcessError) as error:
        print(f"iOS CI cleanup: {error}", file=sys.stderr)
        raise SystemExit(1) from error
