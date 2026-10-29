#!/usr/bin/env python3
"""Run mobile Cargo commands against an explicitly configured GStreamer SDK.

The official mobile SDK is supplied by the build environment. This helper does
not download it or fall back to host pkg-config results.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import sys


IOS_SDK_ENV = "LINGXI_GSTREAMER_SDK_IOS"
PLUGINS = [
    "gstapp", "gstaudioconvert", "gstaudioresample", "gstcoreelements",
    "gstopus", "gstrtp", "gstrtpmanager", "gstvpx", "gstvideoconvertscale",
]
MODULES = [
    "glib-2.0", "gobject-2.0", "gio-2.0", "gmodule-2.0",
    "gstreamer-1.0", "gstreamer-base-1.0", "gstreamer-app-1.0",
    "gstreamer-audio-1.0", "gstreamer-video-1.0", *PLUGINS,
]


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"mobile-gstreamer: ERROR: {message}")


def resolve_within(path: pathlib.Path, root: pathlib.Path, label: str) -> pathlib.Path:
    """Resolve an SDK input and reject symlinks that leave its configured root."""
    try:
        resolved_root = root.resolve(strict=True)
        resolved = path.resolve(strict=True)
        resolved.relative_to(resolved_root)
    except (OSError, RuntimeError, ValueError) as error:
        fail(f"{label} must resolve inside the configured SDK root {root}: {path} ({error})")
    return resolved


def validate_sdk_tree(root: pathlib.Path, tree: pathlib.Path, suffixes: set[str]) -> list[pathlib.Path]:
    """Collect SDK files by suffix while checking every file resolves in-root."""
    tree = resolve_within(tree, root, "SDK input directory")
    files: list[pathlib.Path] = []
    for path in tree.rglob("*"):
        if path.is_symlink():
            resolve_within(path, root, "SDK input symlink")
        if path.is_file() and path.suffix in suffixes:
            resolve_within(path, root, "SDK input file")
            files.append(path)
    return files


def content_digest(root: pathlib.Path, paths: list[pathlib.Path], identity: str) -> str:
    """Hash selected SDK inputs so same-path replacements cannot reuse Cargo output."""
    root = root.resolve(strict=True)
    digest = hashlib.sha256()
    digest.update(identity.encode("utf-8"))
    digest.update(b"\0")
    for path in sorted(set(paths), key=lambda item: item.relative_to(root).as_posix()):
        resolved = resolve_within(path, root, "SDK content input")
        relative = path.relative_to(root).as_posix()
        size = resolved.stat().st_size
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(size.to_bytes(8, "big"))
        with resolved.open("rb") as source:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
        digest.update(b"\0")
    return digest.hexdigest()


def resolve_android_sdk(target: str, requested: str | None) -> pathlib.Path:
    variable = "LINGXI_GSTREAMER_SDK_ANDROID_ARM64" if target == "aarch64-linux-android" else "LINGXI_GSTREAMER_SDK_ANDROID_X86_64"
    universal = requested or os.environ.get(variable) or os.environ.get("LINGXI_GSTREAMER_SDK_ANDROID")
    if not universal:
        fail(f"set {variable} or LINGXI_GSTREAMER_SDK_ANDROID to an installed GStreamer SDK slice")
    root = pathlib.Path(universal).expanduser().resolve(strict=True)
    aliases = ["arm64", "arm64-v8a", "aarch64"] if target == "aarch64-linux-android" else ["x86_64"]
    candidates = [root, *(root / alias for alias in aliases)]
    sdk = None
    for candidate in candidates:
        try:
            resolved_candidate = resolve_within(candidate, root, "Android SDK slice")
            pkgconfig = resolve_within(resolved_candidate / "lib/pkgconfig", root, "Android pkg-config directory")
        except SystemExit:
            continue
        if pkgconfig.is_dir():
            sdk = resolved_candidate
            break
    if sdk is None:
        fail(f"no Android GStreamer lib/pkgconfig slice found under {root} for {target}")
    return sdk


def ios_xcframework(requested: str | None) -> pathlib.Path:
    value = requested or os.environ.get(IOS_SDK_ENV)
    if not value:
        fail(f"set {IOS_SDK_ENV} to the official GStreamer.xcframework")
    path = pathlib.Path(value).expanduser().resolve(strict=True)
    if path.name != "GStreamer.xcframework" or not path.is_dir():
        fail(f"{IOS_SDK_ENV} must name GStreamer.xcframework: {path}")
    return path


def ios_slice(root: pathlib.Path, target: str) -> pathlib.Path:
    root = root.resolve(strict=True)
    identifier = {
        "aarch64-apple-ios": "ios-arm64",
        "aarch64-apple-ios-sim": "ios-arm64_x86_64-simulator",
        "x86_64-apple-ios": "ios-arm64_x86_64-simulator",
    }[target]
    slice_path = resolve_within(root / identifier, root, "iOS framework slice")
    archive = resolve_within(slice_path / "libGStreamer.a", root, "iOS static archive")
    headers = resolve_within(slice_path / "Headers", root, "iOS headers directory")
    header_arch = "x86_64" if target == "x86_64-apple-ios" else "arm64"
    glib_header = resolve_within(headers / "glib.h", root, "iOS GLib header")
    glib_config = resolve_within(headers / header_arch / "glibconfig.h", root, "iOS GLib architecture header")
    gst_version_header = resolve_within(headers / "gst/gstversion.h", root, "iOS GStreamer version header")
    if not archive.is_file() or not glib_header.is_file() or not glib_config.is_file() or not gst_version_header.is_file():
        fail(f"GStreamer XCFramework is missing the {identifier} archive or headers")
    # The compiler can follow nested includes below this directory. Reject
    # escaped symlinks there as well as for the three directly consumed files.
    validate_sdk_tree(root, headers, {".h"})
    info_plist = resolve_within(root / "Info.plist", root, "iOS framework Info.plist")
    try:
        metadata = plistlib.loads(info_plist.read_bytes())
        available = {entry["LibraryIdentifier"] for entry in metadata.get("AvailableLibraries", [])}
    except (OSError, KeyError, plistlib.InvalidFileException) as error:
        fail(f"cannot read GStreamer XCFramework Info.plist: {error}")
    if identifier not in available:
        fail(f"GStreamer XCFramework Info.plist does not declare slice {identifier}")
    expected_arch = "arm64" if target.startswith("aarch64") else "x86_64"
    result = subprocess.run(["xcrun", "lipo", "-archs", str(archive)], text=True, capture_output=True)
    if result.returncode != 0 or expected_arch not in result.stdout.split():
        fail(f"{archive} does not contain required architecture {expected_arch}")
    return slice_path


def android_pkgconfig_directories(sdk: pathlib.Path) -> list[pathlib.Path]:
    pkgconfig_dirs = []
    for path in (sdk / "lib/pkgconfig", sdk / "lib/gstreamer-1.0/pkgconfig"):
        if path.exists():
            resolved = resolve_within(path, sdk, "Android pkg-config directory")
            if resolved.is_dir():
                pkgconfig_dirs.append(resolved)
    if not pkgconfig_dirs:
        fail(f"Android GStreamer SDK has no pkg-config directories under {sdk}")
    return pkgconfig_dirs


def android_content_identity(sdk: pathlib.Path, target: str) -> str:
    pkgconfig_dirs = android_pkgconfig_directories(sdk)
    for module in MODULES:
        found = False
        for directory in pkgconfig_dirs:
            pc = directory / f"{module}.pc"
            if pc.exists():
                resolve_within(pc, sdk, "Android pkg-config metadata")
                found = True
                break
        if not found:
            fail(f"Android GStreamer SDK is missing required pkg-config metadata {module}.pc under {sdk}")
    pc_files = validate_sdk_tree(sdk, sdk / "lib", {".pc"})
    static_libraries = validate_sdk_tree(sdk, sdk / "lib", {".a"})
    header_files = validate_sdk_tree(sdk, sdk / "include", {".h"}) if (sdk / "include").exists() else []
    return content_digest(
        sdk,
        [*pc_files, *static_libraries, *header_files],
        f"android\n{target}\n{sdk}\nv1",
    )


def ios_content_identity(root: pathlib.Path, sdk: pathlib.Path, target: str) -> str:
    headers = resolve_within(sdk / "Headers", root, "iOS headers directory")
    header_files = validate_sdk_tree(root, headers, {".h", ".modulemap"})
    archive = resolve_within(sdk / "libGStreamer.a", root, "iOS static archive")
    return content_digest(root, [archive, *header_files], f"ios\n{target}\n{sdk}\nv1")


def version_from_header(path: pathlib.Path, pattern: str, label: str) -> str:
    try:
        contents = path.read_text()
    except OSError as error:
        fail(f"cannot read {label} header {path}: {error}")
    match = re.search(pattern, contents)
    if match is None:
        fail(f"cannot determine {label} version from {path}")
    return match.group(1)


def run_mobile(platform: str, target: str, sdk_arg: str | None, ndk_arg: str | None, command: list[str]) -> None:
    if not command:
        fail("provide Cargo arguments after --")
    env = os.environ.copy()
    env["PKG_CONFIG_ALLOW_CROSS"] = "1"
    env["PKG_CONFIG_PATH"] = ""
    env["CARGO_PROFILE_DEV_DEBUG"] = "0"
    env["CARGO_PROFILE_DEV_INCREMENTAL"] = "false"
    env["CARGO_PROFILE_DEV_OPT_LEVEL"] = "2"
    env["CARGO_PROFILE_DEV_DEBUG_ASSERTIONS"] = "false"
    if platform == "android":
        if target not in ("aarch64-linux-android", "x86_64-linux-android"):
            fail(f"unsupported Android Rust target: {target}")
        sdk = resolve_android_sdk(target, sdk_arg)
        ndk_value = ndk_arg or os.environ.get("ANDROID_NDK_HOME") or os.environ.get("ANDROID_NDK_ROOT")
        if not ndk_value:
            fail("Android builds require ANDROID_NDK_HOME or --ndk")
        ndk = pathlib.Path(ndk_value).expanduser().resolve(strict=True)
        host_tag = "darwin-arm64" if sys.platform == "darwin" and pathlib.Path(ndk, "toolchains/llvm/prebuilt/darwin-arm64").is_dir() else "darwin-x86_64" if sys.platform == "darwin" else "linux-x86_64"
        toolchain = ndk / "toolchains/llvm/prebuilt" / host_tag / "bin"
        api = "24"
        compiler = toolchain / f"{target}{api}-clang"
        if not compiler.is_file():
            fail(f"Android NDK compiler is missing: {compiler}")
        cargo_target = target.upper().replace("-", "_")
        env[f"CC_{target}"] = str(compiler)
        env[f"AR_{target}"] = str(toolchain / "llvm-ar")
        env[f"CARGO_TARGET_{cargo_target}_LINKER"] = str(compiler)
        identity = android_content_identity(sdk, target)
        target_root = pathlib.Path(env.get("CARGO_TARGET_DIR", pathlib.Path(__file__).resolve().parents[2] / "target")).expanduser().resolve()
        identity_pc_dir = target_root / ".mobile-gstreamer-pc" / "android" / target / identity[:24]
        identity_pc_dir.mkdir(parents=True, exist_ok=True)
        pkgconfig_dirs = android_pkgconfig_directories(sdk)
        env["PKG_CONFIG_LIBDIR"] = os.pathsep.join([*(str(path) for path in pkgconfig_dirs), str(identity_pc_dir.resolve())])
        env["PKG_CONFIG_ALL_STATIC"] = "1"
        subprocess.run(command, env=env, check=True)
        return

    if platform != "ios":
        fail(f"unsupported platform: {platform}")
    if target not in ("aarch64-apple-ios", "aarch64-apple-ios-sim", "x86_64-apple-ios"):
        fail(f"unsupported iOS Rust target: {target}")
    root = ios_xcframework(sdk_arg)
    sdk = ios_slice(root, target)
    headers = sdk / "Headers"
    header_arch = "x86_64" if target == "x86_64-apple-ios" else "arm64"
    glib_version = ".".join(
        version_from_header(headers / header_arch / "glibconfig.h", rf"#define GLIB_{part}_VERSION\s+(\d+)", "GLib")
        for part in ("MAJOR", "MINOR", "MICRO")
    )
    gst_version = ".".join(
        version_from_header(headers / "gst/gstversion.h", rf"#define GST_VERSION_{part}\s+\((\d+)\)", "GStreamer")
        for part in ("MAJOR", "MINOR", "MICRO")
    )
    env["IPHONEOS_DEPLOYMENT_TARGET"] = "26.0"
    # Cargo tracks PKG_CONFIG_LIBDIR. A random temporary path invalidates every
    # dependent crate on each invocation, including the framework assembly pass.
    target_root = pathlib.Path(env.get("CARGO_TARGET_DIR", pathlib.Path(__file__).resolve().parents[2] / "target"))
    sdk_identity = ios_content_identity(root, sdk, target)
    identity = hashlib.sha256(f"{sdk}\n{target}\n{glib_version}\n{gst_version}\n{sdk_identity}".encode()).hexdigest()[:24]
    pc_dir = target_root / ".mobile-gstreamer-pc" / identity
    pc_dir.mkdir(parents=True, exist_ok=True)
    env["PKG_CONFIG_LIBDIR"] = str(pc_dir.resolve())
    env.pop("PKG_CONFIG_ALL_STATIC", None)
    for module in MODULES:
        version = glib_version if module in MODULES[:4] else gst_version
        pc = pc_dir / f"{module}.pc"
        contents = (
            f"prefix={sdk}\nlibdir=${{prefix}}\nincludedir=${{prefix}}/Headers\n"
            f"Name: {module}\nDescription: Official GStreamer iOS XCFramework\nVersion: {version}\n"
            "Libs: -L${libdir} -lGStreamer\nCflags: -I${includedir}\n"
        )
        if not pc.exists() or pc.read_text() != contents:
            staging = pc.with_name(f"{pc.name}.{os.getpid()}.tmp")
            staging.write_text(contents)
            staging.replace(pc)
        system_deps = module.upper().replace("-", "_").replace(".", "_")
        env[f"SYSTEM_DEPS_{system_deps}_LIB"] = ""
        env[f"SYSTEM_DEPS_{system_deps}_LINK"] = "dynamic"
    subprocess.run(command, env=env, check=True)


def stage_ios_framework(source_value: str | None, destination_value: str, targets: list[str]) -> None:
    source = ios_xcframework(source_value)
    for target in targets:
        ios_slice(source, target)
    destination = pathlib.Path(destination_value).expanduser().resolve()
    if destination == source:
        fail("staged iOS GStreamer XCFramework cannot overwrite its configured source")
    destination.parent.mkdir(parents=True, exist_ok=True)
    staging = destination.with_name(destination.name + f".stage-{os.getpid()}")
    if staging.exists():
        shutil.rmtree(staging)
    # Official SDK archives contain multi-gigabyte static libraries. APFS
    # clones keep staging isolated without allocating a second full SDK.
    # Other filesystems retain the ordinary portable copy path.
    copied = False
    if sys.platform == "darwin":
        clone = subprocess.run(
            ["/bin/cp", "-cR", str(source), str(staging)],
            capture_output=True,
        )
        copied = clone.returncode == 0
        if not copied and staging.exists():
            shutil.rmtree(staging)
    if not copied:
        shutil.copytree(source, staging, symlinks=True)
    if destination.exists():
        shutil.rmtree(destination)
    staging.rename(destination)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="mode", required=True)
    run_parser = subparsers.add_parser("run")
    run_parser.add_argument("platform", choices=("android", "ios"))
    run_parser.add_argument("--target", required=True)
    run_parser.add_argument("--sdk")
    run_parser.add_argument("--ndk")
    stage_parser = subparsers.add_parser("stage-ios")
    stage_parser.add_argument("--sdk")
    stage_parser.add_argument("--destination", required=True)
    stage_parser.add_argument("--target", action="append", required=True)
    args, remainder = parser.parse_known_args()
    if args.mode == "run":
        command = remainder[1:] if remainder[:1] == ["--"] else remainder
        run_mobile(args.platform, args.target, args.sdk, args.ndk, command)
    else:
        if remainder:
            parser.error("stage-ios does not accept a command")
        stage_ios_framework(args.sdk, args.destination, args.target)


if __name__ == "__main__":
    main()
