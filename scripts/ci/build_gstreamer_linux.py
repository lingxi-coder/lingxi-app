#!/usr/bin/env python3
"""Build the pinned, minimal GStreamer Linux SDK from the upstream monorepo."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import platform
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
from gstreamer_sdk import SetupError, _require_runner_temp_root, _run_pkg_config, download_artifact, export_environment, load_lock, safe_extract_tar  # noqa: E402


MESON_VERSION = "1.4.1"
LIBDIR = "lib/x86_64-linux-gnu"
PLUGIN_NAMES = (
    "app", "audioconvert", "audioresample", "coreelements", "opus",
    "rtp", "rtpmanager", "vpx", "videoconvertscale",
)
RUNTIME_LIBRARIES = (
    "libgstreamer-1.0.so.0", "libgstapp-1.0.so.0", "libgstbase-1.0.so.0",
    "libgstaudio-1.0.so.0", "libgstvideo-1.0.so.0",
)
MESON_OPTIONS = (
    "-Dbase=enabled",
    "-Dgood=enabled",
    "-Dbad=disabled",
    "-Dugly=disabled",
    "-Dlibav=disabled",
    "-Ddevtools=disabled",
    "-Dges=disabled",
    "-Drtsp_server=disabled",
    "-Dgst-examples=disabled",
    "-Dgst-full=disabled",
    "-Dgstreamer-full-license=unknown",
    "-Dtests=disabled",
    "-Dexamples=disabled",
    "-Dtools=disabled",
    "-Ddoc=disabled",
    "-Dintrospection=disabled",
    "-Dpython=disabled",
    "-Dtls=disabled",
    "-Dlibnice=disabled",
    "-Dgtk=disabled",
    "-Dorc-source=subproject",
    "-Dgst-plugins-base:app=enabled",
    "-Dgst-plugins-base:audioconvert=enabled",
    "-Dgst-plugins-base:audioresample=enabled",
    "-Dgst-plugins-base:opus=enabled",
    "-Dgst-plugins-base:videoconvertscale=enabled",
    "-Dgst-plugins-good:rtp=enabled",
    "-Dgst-plugins-good:rtpmanager=enabled",
    "-Dgst-plugins-good:vpx=enabled",
)


def build_identity(lock: dict) -> str:
    builder_digest = hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()
    sdk_helper = pathlib.Path(__file__).with_name("gstreamer_sdk.py")
    sdk_helper_digest = hashlib.sha256(sdk_helper.read_bytes()).hexdigest()
    requirements = pathlib.Path(__file__).with_name("gstreamer-linux-meson-requirements.txt")
    requirements_digest = hashlib.sha256(requirements.read_bytes()).hexdigest()
    values = [
        f"gstreamer={lock['artifacts']['linux-source']['sha256']}",
        f"orc={lock['artifacts']['linux-orc']['sha256']}",
        f"meson={MESON_VERSION}",
        f"builder={builder_digest}",
        f"sdk-helper={sdk_helper_digest}",
        f"meson-requirements={requirements_digest}",
        "build-platform=ubuntu-22.04-x86_64",
        f"libdir={LIBDIR}",
        *MESON_OPTIONS,
    ]
    return hashlib.sha256("\n".join(values).encode("utf-8")).hexdigest()[:24]


def _require_system_tool(name: str) -> str:
    path = shutil.which(name)
    if not path:
        raise SetupError(f"{name} is required by the Linux GStreamer SDK build")
    return path


def _verify_build_tools() -> None:
    meson = subprocess.run(
        [sys.executable, "-m", "mesonbuild.mesonmain", "--version"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    if meson != MESON_VERSION:
        raise SetupError(f"Meson {MESON_VERSION} is pinned for GStreamer 1.28.8; found {meson}")
    _require_system_tool("ninja")
    pkg_config = _require_system_tool("pkg-config")
    for module in ("glib-2.0", "opus", "vpx"):
        subprocess.run([pkg_config, "--exists", module], check=True)


def _copy_source_notices(source_root: pathlib.Path, prefix: pathlib.Path) -> None:
    components = {
        "gstreamer": source_root / "subprojects/gstreamer",
        "gst-plugins-base": source_root / "subprojects/gst-plugins-base",
        "gst-plugins-good": source_root / "subprojects/gst-plugins-good",
        "orc": source_root / "subprojects/orc",
    }
    notice_root = prefix / "share/licenses/GStreamer-1.28.8"
    for name, component_root in components.items():
        for filename in ("COPYING", "COPYING.LIB", "LICENSE"):
            source = component_root / filename
            if not source.is_file():
                continue
            destination = notice_root / name / filename
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)


def _validate_prefix(prefix: pathlib.Path, version: str) -> None:
    library_dir = prefix / LIBDIR
    pkgconfig_dir = library_dir / "pkgconfig"
    plugin_dir = library_dir / "gstreamer-1.0"
    if not pkgconfig_dir.is_dir() or not plugin_dir.is_dir():
        raise SetupError(f"Meson did not install the GStreamer SDK layout under {prefix}")
    for filename in RUNTIME_LIBRARIES:
        if not (library_dir / filename).is_file():
            raise SetupError(f"GStreamer source build did not install required library {filename}")
    missing = [f"libgst{name}.so" for name in PLUGIN_NAMES if not (plugin_dir / f"libgst{name}.so").is_file()]
    if missing:
        raise SetupError(f"GStreamer source build is missing required plugins: {missing}")
    pkg_config = _require_system_tool("pkg-config")
    system_pc_dirs = (
        "/usr/lib/x86_64-linux-gnu/pkgconfig",
        "/usr/lib/pkgconfig",
        "/usr/share/pkgconfig",
    )
    pkg_config_libdir = os.pathsep.join(
        (str(pkgconfig_dir), *(path for path in system_pc_dirs if pathlib.Path(path).is_dir()))
    )
    _run_pkg_config(
        pkg_config,
        pkg_config_libdir,
        expected_plugins=tuple(f"libgst{name}.so" for name in PLUGIN_NAMES),
        expected_version=version,
    )
    if not (prefix / "share/licenses/GStreamer-1.28.8").is_dir():
        raise SetupError("GStreamer source license notices were not staged into the SDK prefix")


def _environment(prefix: pathlib.Path) -> dict[str, str]:
    library_dir = prefix / LIBDIR
    pkgconfig_dir = library_dir / "pkgconfig"
    current_loader_path = os.environ.get("LD_LIBRARY_PATH", "")
    loader_path = str(library_dir)
    if current_loader_path:
        loader_path += os.pathsep + current_loader_path
    system_pc_dirs = (
        "/usr/lib/x86_64-linux-gnu/pkgconfig",
        "/usr/lib/pkgconfig",
        "/usr/share/pkgconfig",
    )
    pkg_config_libdir = os.pathsep.join(
        (str(pkgconfig_dir), *(path for path in system_pc_dirs if pathlib.Path(path).is_dir()))
    )
    return {
        "LINGXI_GSTREAMER_SDK_LINUX_SOURCE": str(prefix),
        "PKG_CONFIG_PATH": str(pkgconfig_dir),
        "PKG_CONFIG_LIBDIR": pkg_config_libdir,
        "LD_LIBRARY_PATH": loader_path,
    }


def _build_sdk(root: pathlib.Path, lock: dict) -> pathlib.Path:
    identity = build_identity(lock)
    output = root / f"gstreamer-1.28.8-{identity}"
    marker = output / ".lingxi-gstreamer-linux-source-sdk"
    prefix = output / "prefix"
    if output.exists():
        if marker.is_file() and marker.read_text(encoding="ascii").strip() == identity:
            _validate_prefix(prefix, lock["version"])
            return prefix
        raise SetupError(f"Linux GStreamer source build directory is incomplete or stale: {output}")

    source_archive = download_artifact(lock["artifacts"]["linux-source"], root / "downloads")
    orc_archive = download_artifact(lock["artifacts"]["linux-orc"], root / "downloads")
    output.mkdir(parents=True)
    source_stage = output / "source"
    safe_extract_tar(source_archive, source_stage)
    source_candidates = [path for path in source_stage.iterdir() if path.is_dir()]
    source_root = next((path for path in source_candidates if path.name == "gstreamer-1.28.8"), None)
    if source_root is None:
        raise SetupError(f"official GStreamer source archive has no gstreamer-1.28.8 root: {source_candidates}")

    orc_stage = output / "orc-source"
    safe_extract_tar(orc_archive, orc_stage)
    orc_sources = [path for path in orc_stage.iterdir() if path.is_dir() and (path / "meson.build").is_file()]
    if len(orc_sources) != 1 or orc_sources[0].name != "orc-0.4.42":
        raise SetupError(f"official ORC source archive has unexpected layout: {orc_sources}")
    orc_destination = source_root / "subprojects/orc"
    orc_destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(orc_sources[0], orc_destination)

    _verify_build_tools()
    build_dir = output / "build"
    meson = [sys.executable, "-m", "mesonbuild.mesonmain"]
    configure = [
        *meson,
        "setup",
        str(build_dir),
        str(source_root),
        f"--prefix={prefix}",
        f"--libdir={LIBDIR}",
        "--buildtype=release",
        "--wrap-mode=nodownload",
        *MESON_OPTIONS,
    ]
    subprocess.run(configure, check=True)
    subprocess.run([*meson, "compile", "-C", str(build_dir), "--jobs", "2"], check=True)
    subprocess.run([*meson, "install", "-C", str(build_dir)], check=True)
    _copy_source_notices(source_root, prefix)
    _validate_prefix(prefix, lock["version"])
    # Keep only the verified runtime SDK in the Actions cache.
    shutil.rmtree(build_dir)
    shutil.rmtree(source_stage)
    shutil.rmtree(orc_stage)
    marker.write_text(identity + "\n", encoding="ascii")
    return prefix


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = _require_runner_temp_root(args.root)
    lock = load_lock()
    if not sys.platform.startswith("linux") or platform.machine() != "x86_64":
        raise SetupError("the pinned Linux SDK build currently targets x86_64 Linux CI")
    root.mkdir(parents=True, exist_ok=True)
    prefix = _build_sdk(root, lock)
    shutil.rmtree(root / "downloads", ignore_errors=True)
    values = _environment(prefix)
    # Confirm pkg-config selects the built SDK in this step before exporting it.
    os.environ.update(values)
    _validate_prefix(prefix, lock["version"])
    export_environment(values)


if __name__ == "__main__":
    try:
        main()
    except (SetupError, OSError, subprocess.CalledProcessError) as error:
        print(f"gstreamer-linux-build: ERROR: {error}", file=sys.stderr)
        raise SystemExit(1) from error
