import os
import signal
import subprocess
import sys
from pathlib import Path

PACKAGE_NAME = "lingxi"
PACKAGE_METADATA_FILENAME = "lingxi-package.json"


def bundled_package_dir() -> Path:
    path = Path(__file__).resolve().parent
    metadata_path = path / PACKAGE_METADATA_FILENAME
    if not metadata_path.is_file():
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its package metadata at {metadata_path}"
        )
    return path


def bundled_lingxi_path() -> Path:
    exe = "lingxi.exe" if os.name == "nt" else "lingxi"
    path = bundled_package_dir() / "bin" / exe
    if not path.is_file():
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its packaged binary at {path}"
        )
    return path


def bundled_path_dir() -> "Path | None":
    path = bundled_package_dir() / "lingxi-path"
    return path if path.is_dir() else None


def main() -> int:
    """Console entrypoint: exec the bundled lingxi binary with argv passthrough,
    forwarding termination signals and mirroring the child's exit code."""
    binary = bundled_lingxi_path()
    env = dict(os.environ)
    path_dir = bundled_path_dir()
    if path_dir is not None:
        sep = ";" if os.name == "nt" else ":"
        env["PATH"] = f"{path_dir}{sep}{env.get('PATH', '')}"
    env["LINGXI_MANAGED_BY_PIP"] = "1"

    proc = subprocess.Popen([str(binary), *sys.argv[1:]], env=env)

    def _forward(signum, _frame):
        try:
            proc.send_signal(signum)
        except ProcessLookupError:
            pass

    forwardable = [signal.SIGINT, signal.SIGTERM]
    if hasattr(signal, "SIGHUP"):
        forwardable.append(signal.SIGHUP)
    for sig in forwardable:
        try:
            signal.signal(sig, _forward)
        except (ValueError, OSError):
            pass

    code = proc.wait()
    # On POSIX a child killed by signal N reports as -N; mirror the shell/JS
    # launcher convention of 128+N so callers see the conventional exit status.
    return code if code >= 0 else 128 + (-code)


__all__ = [
    "PACKAGE_NAME",
    "bundled_lingxi_path",
    "bundled_package_dir",
    "bundled_path_dir",
    "main",
]
