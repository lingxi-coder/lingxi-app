#!/usr/bin/env python3
"""Prepare the Linux CI-only launcher for the real desktop terminal broker.

The desktop terminal intentionally filters loader variables. The unpackaged
CI binary nevertheless needs the pinned source SDK used to build it. Embed
that one SDK directory in a temporary exec wrapper; never use the ambient
loader path or modify the binary's rpath.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import sys

sys.dont_write_bytecode = True
from build_gstreamer_linux import (  # noqa: E402
    LIBDIR, RUNTIME_LIBRARIES, build_identity, source_sdk_marker, source_sdk_prefix,
)
from gstreamer_sdk import load_lock  # noqa: E402


# Keep this list aligned with terminalEnvironment. In particular, neither
# arbitrary loader variables nor caller-supplied SDK paths are forwarded.
TERMINAL_ENVIRONMENT = (
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE",
    "TMPDIR", "TMP", "TEMP", "TZ", "SystemRoot", "SYSTEMROOT", "WINDIR",
    "COMSPEC", "ComSpec", "PATHEXT", "USERPROFILE", "APPDATA", "LOCALAPPDATA",
    "HOMEDRIVE", "HOMEPATH", "SSH_AUTH_SOCK", "TERM", "COLORTERM",
)


class PreparationError(Exception):
    pass


def _inside(path: pathlib.Path, parent: pathlib.Path) -> bool:
    return path == parent or parent in path.parents


def _digest(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _file_record(kind: str, path: pathlib.Path) -> dict[str, str]:
    return {
        "kind": kind,
        "path": str(path),
        "resolved_path": str(path.resolve(strict=True)),
        "sha256": _digest(path),
    }


def _wrapper_source(bridge: pathlib.Path, library_dir: pathlib.Path) -> str:
    # /bin/sh and /usr/bin/env run with terminalEnvironment's filtered input in
    # the actual test. Unset loader controls before invoking even the latter;
    # env -i then allows only the same terminal variables and our SDK path.
    lines = [
        "#!/bin/sh",
        "set -eu",
        "unset LD_LIBRARY_PATH LD_PRELOAD LD_AUDIT LD_DEBUG LD_DEBUG_OUTPUT LD_PROFILE LD_ORIGIN_PATH LD_BIND_NOW GLIBC_TUNABLES",
        f"set -- {shlex.quote(str(bridge))} \"$@\"",
        f"set -- {shlex.quote('LD_LIBRARY_PATH=' + str(library_dir))} \"$@\"",
    ]
    for name in TERMINAL_ENVIRONMENT:
        lines.append(f'if [ "${{{name}+x}}" ]; then set -- "{name}=${name}" "$@"; fi')
    lines.append('exec /usr/bin/env -i "$@"')
    return "\n".join(lines) + "\n"


def prepare(workspace: pathlib.Path, runner_temp: pathlib.Path, sdk_prefix: pathlib.Path,
            bridge: pathlib.Path, wrapper: pathlib.Path, evidence: pathlib.Path) -> dict:
    workspace = workspace.resolve(strict=True)
    runner_temp = runner_temp.resolve(strict=True)
    lock = load_lock()
    identity = build_identity(lock)
    sdk_output = runner_temp / "gstreamer-sdk/linux-source" / f"gstreamer-{lock['version']}-{identity}"
    sdk_output_real = sdk_output.resolve(strict=True)
    if not _inside(sdk_output_real, runner_temp):
        raise PreparationError("pinned SDK output escapes RUNNER_TEMP")
    expected_prefix = (sdk_output / "prefix").resolve(strict=True)
    if not _inside(expected_prefix, sdk_output_real) or sdk_prefix.resolve(strict=True) != expected_prefix:
        raise PreparationError("source SDK is not the pinned CI SDK prefix")
    marker = source_sdk_marker(sdk_output)
    if marker.resolve(strict=True).parent != sdk_output_real or marker.read_text(encoding="ascii").strip() != identity:
        raise PreparationError("pinned source SDK identity marker is missing or mismatched")
    library_dir = (expected_prefix / LIBDIR).resolve(strict=True)
    if not library_dir.is_dir() or not _inside(library_dir, expected_prefix):
        raise PreparationError("pinned SDK library directory is invalid")
    # glibc accepts both ':' and ';' as separators, and expands $ORIGIN,
    # $LIB and $PLATFORM in LD_LIBRARY_PATH.
    if any(character in str(library_dir) for character in (":", ";", "$", "\n", "\r")):
        raise PreparationError("SDK library directory contains dynamic loader metacharacters")

    bridge_real = bridge.resolve(strict=True)
    expected_bridge = (workspace / "target/release/bridge-server").resolve(strict=True)
    if bridge_real != expected_bridge or not _inside(bridge_real, workspace):
        raise PreparationError("terminal test executable is not the workspace bridge-server")
    if not bridge_real.is_file() or not os.access(bridge_real, os.X_OK):
        raise PreparationError("workspace bridge-server is not an executable file")
    if not _inside(wrapper.parent.resolve(strict=True), runner_temp):
        raise PreparationError("terminal wrapper must be created under RUNNER_TEMP")
    if not _inside(evidence.resolve(strict=True), runner_temp):
        raise PreparationError("terminal evidence must be written under RUNNER_TEMP")

    libraries = sorted(library_dir.glob("*.so*"))
    if any(not (library_dir / name).is_file() for name in RUNTIME_LIBRARIES):
        raise PreparationError("pinned SDK is missing a required GStreamer runtime library")
    for library in libraries:
        if not library.is_file() or library.resolve(strict=True).parent != library_dir:
            raise PreparationError(f"SDK library resolves outside its pinned directory: {library}")

    # Exclusive creation prevents replacing a preexisting path or following a
    # preexisting symlink. The calling script creates the parent with mktemp.
    with wrapper.open("x", encoding="utf-8") as destination:
        destination.write(_wrapper_source(bridge_real, library_dir))
    wrapper.chmod(0o700)
    files = [
        _file_record("bridge-server", bridge_real),
        _file_record("launcher", wrapper),
        _file_record("launcher-generator", pathlib.Path(__file__).resolve()),
        _file_record("sdk-lock", pathlib.Path(__file__).with_name("gstreamer-sdk-lock.json").resolve()),
        _file_record("sdk-builder", pathlib.Path(__file__).with_name("build_gstreamer_linux.py").resolve()),
        _file_record("sdk-validator", pathlib.Path(__file__).with_name("gstreamer_sdk.py").resolve()),
        _file_record("sdk-build-requirements", pathlib.Path(__file__).with_name("gstreamer-linux-meson-requirements.txt").resolve()),
        _file_record("sdk-identity-marker", marker),
        *(_file_record("sdk-library", library) for library in libraries),
    ]
    manifest = {
        "sdk_version": lock["version"],
        "sdk_identity": identity,
        "sdk_prefix": str(expected_prefix),
        "sdk_library_dir": str(library_dir),
        "files": files,
    }
    (evidence / "terminal-broker-provenance.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return manifest


def probe_linkage(bridge: pathlib.Path, library_dir: pathlib.Path, evidence: pathlib.Path) -> None:
    ldd = shutil.which("ldd", path="/usr/bin:/bin")
    if ldd is None:
        raise PreparationError("ldd is required for the Linux terminal broker probe")
    base_env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C"}
    results = []
    for name, environment in (
        ("sanitized-system", base_env),
        ("pinned-sdk", dict(base_env, LD_LIBRARY_PATH=str(library_dir))),
    ):
        result = subprocess.run([ldd, "-r", str(bridge)], env=environment, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=30)
        (evidence / f"terminal-broker-ldd-{name}.txt").write_text(
            f"exit_code={result.returncode}\nstdout:\n{result.stdout}stderr:\n{result.stderr}", encoding="utf-8"
        )
        results.append(result)
    pinned = results[1]
    diagnostics = pinned.stdout + pinned.stderr
    if pinned.returncode or "not found" in diagnostics or "undefined symbol:" in diagnostics:
        raise PreparationError("terminal broker linker probe failed with the pinned SDK; see evidence")
    matched = set()
    for line in pinned.stdout.splitlines():
        match = re.match(r"^\s*(lib(?:gst|gstreamer)\S+)\s+=>\s+(.+?)\s+\(0x[0-9a-fA-F]+\)\s*$", line)
        if not match:
            continue
        name, resolved = match.groups()
        if pathlib.Path(resolved).resolve(strict=True).parent != library_dir:
            raise PreparationError(f"{name} resolved outside the pinned SDK; see evidence")
        matched.add(name)
    if "libgstreamer-1.0.so.0" not in matched:
        raise PreparationError("terminal broker did not resolve GStreamer from the pinned SDK; see evidence")


def verify_manifest(manifest_path: pathlib.Path) -> None:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    library_dir = pathlib.Path(manifest["sdk_library_dir"])
    recorded_libraries = {record["path"] for record in manifest["files"] if record["kind"] == "sdk-library"}
    if {str(path) for path in library_dir.glob("*.so*")} != recorded_libraries:
        raise PreparationError("recorded terminal broker SDK library set changed")
    for record in manifest["files"]:
        path = pathlib.Path(record["path"])
        if str(path.resolve(strict=True)) != record["resolved_path"] or _digest(path) != record["sha256"]:
            raise PreparationError(f"recorded terminal broker input changed: {path}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--verify", type=pathlib.Path)
    parser.add_argument("--workspace", type=pathlib.Path)
    parser.add_argument("--runner-temp", type=pathlib.Path)
    parser.add_argument("--bridge", type=pathlib.Path)
    parser.add_argument("--wrapper", type=pathlib.Path)
    parser.add_argument("--evidence", type=pathlib.Path)
    args = parser.parse_args()
    try:
        if args.verify:
            verify_manifest(args.verify)
            return
        if not all((args.workspace, args.runner_temp, args.bridge, args.wrapper, args.evidence)):
            parser.error("preparation requires workspace, runner-temp, bridge, wrapper and evidence")
        sdk_prefix = source_sdk_prefix(os.environ)
        if not sdk_prefix:
            raise PreparationError("the source SDK builder must export its configured prefix")
        if not sys.platform.startswith("linux"):
            raise PreparationError("the actual terminal broker linker probe requires Linux")
        manifest = prepare(args.workspace, args.runner_temp, pathlib.Path(sdk_prefix), args.bridge, args.wrapper, args.evidence)
        probe_linkage(args.bridge, pathlib.Path(manifest["sdk_library_dir"]), args.evidence)
        print("CI terminal broker: pinned SDK and sanitized-system linker results recorded")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, PreparationError) as error:
        print(f"CI terminal broker setup failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
