#!/usr/bin/env python3
"""Check the built private Local Apps service without using user data or credentials."""

from __future__ import annotations

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

REQUEST_ID = "ci-local-app-list"
LOADER_ENV = (
    "PATH", "PATHEXT", "SYSTEMROOT", "SystemRoot", "WINDIR",
    "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH", "DYLD_FALLBACK_FRAMEWORK_PATH",
    "GST_PLUGIN_PATH", "GST_PLUGIN_PATH_1_0", "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0", "GSTREAMER_1_0_ROOT_MSVC_X86_64",
    "LANG", "LC_ALL", "TZ",
)


def isolated_environment(root: Path) -> dict[str, str]:
    """Retain native-loader settings only; never forward credentials or real homes."""
    env = {name: os.environ[name] for name in LOADER_ENV if name in os.environ}
    home = root / "home"
    temporary = root / "tmp"
    appdata = root / "appdata"
    localappdata = root / "localappdata"
    for directory in (home, temporary, appdata, localappdata):
        directory.mkdir(parents=True)
    env.update({
        "HOME": str(home),
        "USERPROFILE": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "XDG_STATE_HOME": str(home / ".local" / "state"),
        "APPDATA": str(appdata),
        "LOCALAPPDATA": str(localappdata),
        "TMPDIR": str(temporary),
        "TMP": str(temporary),
        "TEMP": str(temporary),
        "RUST_BACKTRACE": "1",
    })
    return env


def stop_timed_out_process(process: subprocess.Popen[bytes]) -> str:
    """Bound cleanup; a POSIX session also contains children inheriting output."""
    errors = []
    if os.name == "posix":
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError as error:
            errors.append(f"process group termination failed: {error}")
    elif process.poll() is None:
        try:
            result = subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=5,
                check=False,
            )
            if result.returncode:
                errors.append(f"process tree termination failed: {result.stderr.decode('utf-8', 'replace')[-4096:]}")
        except (OSError, subprocess.TimeoutExpired) as error:
            errors.append(f"process tree termination failed: {error}")
    if process.poll() is None:
        try:
            process.kill()
        except OSError as error:
            errors.append(f"direct child termination failed: {error}")
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        errors.append("direct child did not exit after termination")
    return "; ".join(errors)


def probe(command: list[str], root: Path, cwd: Path, timeout: float = 30.0) -> None:
    """Run a correlated list request and require a clean exit and empty result."""
    config_home = root / "config"
    config_home.mkdir(parents=True)
    request = {"id": REQUEST_ID, "request": {"op": "list"}}
    # File-backed output avoids an inherited pipe keeping communicate() alive
    # indefinitely after the service exits, especially on Windows.
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr_file:
        try:
            process = subprocess.Popen(
                [*command, "--local-app-service-json", "--config-home", str(config_home)],
                cwd=cwd,
                env=isolated_environment(root),
                stdin=subprocess.PIPE,
                stdout=stdout,
                stderr=stderr_file,
                start_new_session=os.name == "posix",
            )
        except OSError as error:
            raise RuntimeError(f"could not start local app service: {error}") from error
        try:
            process.communicate((json.dumps(request) + "\n").encode(), timeout=timeout)
        except subprocess.TimeoutExpired as error:
            cleanup = stop_timed_out_process(process)
            stderr_file.seek(0)
            stderr = stderr_file.read()[-32768:].decode("utf-8", "replace")
            raise RuntimeError(f"local app service timed out after {timeout}s: stderr={stderr!r}; cleanup={cleanup or 'termination requested'}") from error
        except BaseException:
            stop_timed_out_process(process)
            raise
        stdout.seek(0)
        response = stdout.read().decode("utf-8", "replace")
        stderr_file.seek(0)
        stderr = stderr_file.read()[-32768:].decode("utf-8", "replace")
    if process.returncode != 0:
        raise RuntimeError(f"local app service exited {process.returncode}: stderr={stderr!r}; stdout={response[-4096:]!r}")
    try:
        replies = [json.loads(line) for line in response.splitlines()]
    except json.JSONDecodeError as error:
        raise RuntimeError(f"local app service returned invalid JSON: stdout={response[-4096:]!r}; stderr={stderr!r}") from error
    expected = {"id": REQUEST_ID, "ok": True, "value": []}
    if replies != [expected]:
        raise RuntimeError(f"local app service returned an unexpected list response: {replies!r}; stderr={stderr!r}")
    if stderr:
        print(f"local app service stderr: {stderr}", file=sys.stderr)


def show_native_libraries(binary: Path) -> None:
    if sys.platform == "darwin":
        command = ["/usr/bin/otool", "-L", str(binary)]
    elif sys.platform.startswith("linux"):
        command = ["ldd", str(binary)]
    else:
        # Windows resolves DLLs using the GStreamer SDK directory on PATH;
        # the actual subprocess execution below verifies loader startup.
        return
    print(f"Native dependencies for {binary}:", flush=True)
    result = subprocess.run(command, text=True, capture_output=True, check=False)
    print(result.stdout, end="")
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr)
    if result.returncode or "not found" in result.stdout:
        raise RuntimeError(f"native dependency inspection failed with exit {result.returncode}")


def main() -> None:
    workspace = Path.cwd()
    target = Path(os.environ.get("CARGO_TARGET_DIR") or "target").resolve()
    name = "bridge-server.exe" if sys.platform == "win32" else "bridge-server"
    release = target / "release" / name
    if not release.is_file():
        raise RuntimeError(f"built release bridge server is missing: {release}")
    binaries = [release]
    # The Electron fixture prefers debug when present. Check that actual
    # candidate too, so a stale debug executable cannot hide behind release.
    debug = target / "debug" / name
    if debug.is_file():
        binaries.append(debug)
    for binary in binaries:
        show_native_libraries(binary)
        with tempfile.TemporaryDirectory(prefix="local-app-service-probe-") as temporary:
            probe([str(binary)], Path(temporary).resolve(), workspace / "apps/electron")
        print(f"Local Apps JSON stdio probe passed: {binary}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as error:
        print(f"local app service probe failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
