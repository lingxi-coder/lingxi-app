#!/usr/bin/env python3
"""Install the pinned upstream GStreamer SDKs required by CI build lanes."""
from __future__ import annotations

import argparse
import errno
import hashlib
import http.client
import json
import os
import pathlib
import plistlib
import re
import shutil
import socket
import ssl
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from fnmatch import fnmatch


LOCK_PATH = pathlib.Path(__file__).with_name("gstreamer-sdk-lock.json")
PLUGIN_NAMES = (
    "app", "audioconvert", "audioresample", "coreelements", "opus",
    "rtp", "rtpmanager", "vpx", "videoconvertscale",
)
RUNTIME_LIBRARIES = (
    "libgstreamer-1.0.so.0", "libgstapp-1.0.so.0", "libgstbase-1.0.so.0",
    "libgstaudio-1.0.so.0", "libgstvideo-1.0.so.0", "libglib-2.0.so.0",
    "libgobject-2.0.so.0", "libgio-2.0.so.0",
)
GSTREAMER_RUNTIME_LIBRARIES = RUNTIME_LIBRARIES[:5]
SYSTEM_RUNTIME_MODULES = {
    "libglib-2.0.so.0": "glib-2.0",
    "libgobject-2.0.so.0": "gobject-2.0",
    "libgio-2.0.so.0": "gio-2.0",
}
PKG_CONFIG_MODULES = (
    "gstreamer-1.0", "gstreamer-app-1.0", "gstreamer-audio-1.0",
    "gstreamer-video-1.0", "gstreamer-base-1.0",
)
SYSTEM_LINUX_LIBRARIES = (
    "libc.so.*", "libm.so.*", "libpthread.so.*", "librt.so.*",
    "libdl.so.*", "libresolv.so.*", "libanl.so.*", "libutil.so.*",
    "libnss_*.so.*", "ld-linux*.so*", "linux-vdso.so.*",
)
ANDROID_MODULES = (
    "glib-2.0", "gobject-2.0", "gio-2.0", "gmodule-2.0",
    "gstreamer-1.0", "gstreamer-base-1.0", "gstreamer-app-1.0",
    "gstreamer-audio-1.0", "gstreamer-video-1.0", "gstapp",
    "gstaudioconvert", "gstaudioresample", "gstcoreelements", "gstopus",
    "gstrtp", "gstrtpmanager", "gstvpx", "gstvideoconvertscale",
)
ANDROID_ABIS = {
    "arm64": ("arm64", "arm64-v8a", "aarch64"),
    "x86_64": ("x86_64",),
}
IOS_SLICES = (
    "ios-arm64", "ios-arm64_x86_64-simulator",
)
MAX_ARCHIVE_BYTES = 8 * 1024**3
MAX_DOWNLOAD_BYTES = 2 * 1024**3
DOWNLOAD_ATTEMPTS = 3


class SetupError(RuntimeError):
    """A deterministic SDK provisioning or validation failure."""


def load_lock(path: pathlib.Path = LOCK_PATH) -> dict:
    lock = json.loads(path.read_text(encoding="utf-8"))
    if lock.get("version") != "1.28.8":
        raise SetupError("GStreamer SDK lock must pin version 1.28.8")
    artifacts = lock.get("artifacts")
    if not isinstance(artifacts, dict):
        raise SetupError("GStreamer SDK lock has no artifact table")
    for name, artifact in artifacts.items():
        url = artifact.get("url", "")
        filename = artifact.get("filename", "")
        digest = artifact.get("sha256", "")
        official_url = url.startswith("https://gstreamer.freedesktop.org/") or (
            name == "linux-source"
            and url.startswith("https://gitlab.freedesktop.org/gstreamer/gstreamer/-/archive/")
        )
        if not official_url:
            raise SetupError(f"{name} artifact is not hosted by an allowed official GStreamer upstream URL")
        if not filename or pathlib.Path(filename).name != filename:
            raise SetupError(f"{name} artifact filename must be a plain file name")
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise SetupError(f"{name} artifact is missing a pinned SHA-256")
    expected = {"android", "ios", "macos-runtime", "macos-devel", "windows", "linux-source", "linux-orc"}
    if set(artifacts) != expected:
        raise SetupError(f"GStreamer SDK lock entries differ from the supported targets: {set(artifacts)}")
    return lock


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_artifact(path: pathlib.Path, artifact: dict) -> None:
    actual = sha256_file(path)
    if actual != artifact["sha256"]:
        raise SetupError(
            f"SHA-256 mismatch for {artifact['filename']}: expected {artifact['sha256']}, got {actual}"
        )


class _DownloadFailure(Exception):
    def __init__(self, reason: str, *, retryable: bool = False) -> None:
        super().__init__(reason)
        self.retryable = retryable


def _transient_network_error(error: BaseException) -> bool:
    if isinstance(error, urllib.error.URLError):
        error = error.reason
    if isinstance(error, ssl.SSLCertVerificationError):
        return False
    if isinstance(error, (TimeoutError, ConnectionError, socket.gaierror, socket.herror,
                          ssl.SSLEOFError, http.client.IncompleteRead, http.client.RemoteDisconnected)):
        return True
    return isinstance(error, OSError) and error.errno in {
        errno.EHOSTUNREACH, errno.ENETUNREACH, errno.ENETDOWN, errno.ENETRESET,
        errno.ECONNRESET, errno.ECONNABORTED, errno.ECONNREFUSED, errno.EPIPE, errno.ETIMEDOUT,
    }


def _download_headers(response, details: dict) -> int | None:
    details["status"] = response.getcode()
    content_type = response.headers.get("Content-Type")
    if content_type:
        content_type = content_type.split(";", 1)[0].strip().lower()
        details["content_type"] = content_type if re.fullmatch(
            r"[a-z0-9!#$&^_.+-]+/[a-z0-9!#$&^_.+-]+", content_type
        ) else "invalid"
    lengths = response.headers.get_all("Content-Length", [])
    if not lengths:
        return None
    if len(lengths) != 1 or not re.fullmatch(r"[0-9]+", lengths[0].strip()):
        details["content_length"] = "invalid"
        raise _DownloadFailure("invalid Content-Length")
    digits = lengths[0].strip().lstrip("0") or "0"
    if len(digits) > len(str(MAX_DOWNLOAD_BYTES)):
        details["content_length"] = "over-limit"
        raise _DownloadFailure(f"SDK artifact exceeds the {MAX_DOWNLOAD_BYTES}-byte download limit")
    length = int(digits)
    details["content_length"] = length
    if length == 0:
        raise _DownloadFailure("invalid Content-Length")
    if length > MAX_DOWNLOAD_BYTES:
        raise _DownloadFailure(f"SDK artifact exceeds the {MAX_DOWNLOAD_BYTES}-byte download limit")
    return length


def _download_once(artifact: dict, destination: pathlib.Path, details: dict) -> None:
    request = urllib.request.Request(
        artifact["url"], headers={"User-Agent": "LingXi-CI-GStreamer-SDK/1.0"}
    )
    try:
        response = urllib.request.urlopen(request, timeout=60)
    except urllib.error.HTTPError as error:
        try:
            _download_headers(error, details)
        finally:
            error.close()
        status = error.code
        raise _DownloadFailure(
            f"HTTP {status}", retryable=status in (408, 429) or 500 <= status <= 599
        ) from error
    except (urllib.error.URLError, OSError, http.client.HTTPException) as error:
        raise _DownloadFailure("network connection failure", retryable=_transient_network_error(error)) from error

    partial = None
    try:
        length = _download_headers(response, details)
        try:
            with tempfile.NamedTemporaryFile(
                mode="wb", prefix=f".{destination.name}.", suffix=".part",
                dir=destination.parent, delete=False,
            ) as output:
                partial = pathlib.Path(output.name)
                while True:
                    try:
                        block = response.read(1024 * 1024)
                    except http.client.IncompleteRead as error:
                        details["bytes"] += len(error.partial)
                        details["digest"].update(error.partial)
                        if details["bytes"] > MAX_DOWNLOAD_BYTES:
                            raise _DownloadFailure(
                                f"SDK artifact exceeds the {MAX_DOWNLOAD_BYTES}-byte download limit"
                            ) from error
                        raise _DownloadFailure("short read", retryable=True) from error
                    except (urllib.error.URLError, OSError, http.client.HTTPException) as error:
                        raise _DownloadFailure(
                            "network read failure", retryable=_transient_network_error(error)
                        ) from error
                    if not block:
                        break
                    details["bytes"] += len(block)
                    details["digest"].update(block)
                    if details["bytes"] > MAX_DOWNLOAD_BYTES:
                        raise _DownloadFailure(f"SDK artifact exceeds the {MAX_DOWNLOAD_BYTES}-byte download limit")
                    if length is not None and details["bytes"] > length:
                        raise _DownloadFailure("response exceeds Content-Length")
                    if output.write(block) != len(block):
                        raise _DownloadFailure("local disk write was incomplete")
        except OSError as error:
            raise _DownloadFailure("local disk error") from error
        if length is not None and details["bytes"] != length:
            raise _DownloadFailure("short read", retryable=True)
        try:
            details["file_sha256"] = sha256_file(partial)
        except OSError as error:
            raise _DownloadFailure("local disk error") from error
        if details["file_sha256"] != artifact["sha256"]:
            raise _DownloadFailure("SHA-256 mismatch", retryable=True)
        try:
            partial.replace(destination)
        except OSError as error:
            raise _DownloadFailure("local disk error") from error
        partial = None
    finally:
        try:
            response.close()
        except OSError:
            pass
        if partial is not None:
            try:
                partial.unlink(missing_ok=True)
            except OSError as error:
                raise _DownloadFailure("local disk cleanup error") from error


def download_artifact(artifact: dict, download_dir: pathlib.Path) -> pathlib.Path:
    download_dir.mkdir(parents=True, exist_ok=True)
    destination = download_dir / artifact["filename"]
    if destination.exists():
        verify_artifact(destination, artifact)
        return destination

    for attempt in range(1, DOWNLOAD_ATTEMPTS + 1):
        details = {"status": "-", "content_length": "-", "content_type": "-",
                   "bytes": 0, "digest": hashlib.sha256()}
        failure = None
        try:
            _download_once(artifact, destination, details)
        except _DownloadFailure as error:
            failure = error
        reason = str(failure) if failure else "verified"
        print(
            f"gstreamer-sdk: download {artifact['filename']} attempt={attempt}/{DOWNLOAD_ATTEMPTS} "
            f"status={details['status']} content-length={details['content_length']} "
            f"content-type={details['content_type']} bytes={details['bytes']} "
            f"sha256={details.get('file_sha256', details['digest'].hexdigest())} reason={reason}",
            file=sys.stderr,
        )
        if failure is None:
            return destination
        if not failure.retryable or attempt == DOWNLOAD_ATTEMPTS:
            raise SetupError(
                f"failed to fetch verified SDK artifact {artifact['filename']} after {attempt} attempt(s): {failure}"
            ) from failure
        time.sleep(attempt)
    raise AssertionError("unreachable download retry state")


def _inside(root: pathlib.Path, candidate: pathlib.Path) -> bool:
    try:
        candidate.relative_to(root)
        return True
    except ValueError:
        return False


def safe_extract_tar(
    archive: pathlib.Path,
    destination: pathlib.Path,
    *,
    ios_platform_only: bool = False,
    android_platform_only: bool = False,
    resume_existing: bool = False,
) -> None:
    """Extract a verified SDK archive without allowing paths outside destination."""
    destination.mkdir(parents=True, exist_ok=True)
    resolved_root = destination.resolve()
    with tarfile.open(archive, "r:*") as source:
        members = source.getmembers()
        total_size = sum(member.size for member in members if member.isfile())
        if total_size > MAX_ARCHIVE_BYTES:
            raise SetupError(f"SDK archive expands beyond the {MAX_ARCHIVE_BYTES}-byte limit")
        for member in members:
            member_path = pathlib.PurePosixPath(member.name)
            if "\\" in member.name or "\\" in member.linkname:
                raise SetupError(f"unsupported Windows-style path in SDK archive: {member.name}")
            if member_path.is_absolute() or ".." in member_path.parts:
                raise SetupError(f"unsafe SDK archive path: {member.name}")
            if member.isdev() or member.isfifo():
                raise SetupError(f"unsupported special file in SDK archive: {member.name}")
            if member.issym() or member.islnk():
                link_path = pathlib.PurePosixPath(member.linkname)
                if link_path.is_absolute():
                    raise SetupError(f"unsafe SDK archive link: {member.name} -> {member.linkname}")
                anchor = member_path.parent if member.issym() else pathlib.PurePosixPath()
                normalized_link = pathlib.PurePosixPath(os.path.normpath(str(anchor / link_path)))
                if ".." in normalized_link.parts or normalized_link.is_absolute():
                    raise SetupError(f"unsafe SDK archive link: {member.name} -> {member.linkname}")
            output = (destination / pathlib.Path(*member_path.parts)).resolve(strict=False)
            if not _inside(resolved_root, output):
                raise SetupError(f"unsafe SDK archive output: {member.name}")
        # Validate every member above, including skipped platforms. Filtering
        # never turns an unsafe archive path or link into an accepted archive.
        if not ios_platform_only and not android_platform_only and not resume_existing:
            try:
                source.extractall(destination, members=members, filter="data")
            except TypeError:  # Python 3.11 before the data filter backport.
                source.extractall(destination, members=members)
            return
        for member in members:
            parts = pathlib.PurePosixPath(member.name).parts
            if ios_platform_only and len(parts) > 1 and parts[0] == "GStreamer.xcframework":
                if parts[1] not in (*IOS_SLICES, "Info.plist"):
                    continue
            if android_platform_only and parts and parts[0] not in ("arm64", "x86_64"):
                continue
            output = destination / pathlib.Path(*parts)
            if resume_existing and member.isfile() and output.exists():
                if output.is_symlink() or not output.is_file() or output.stat().st_size > member.size:
                    raise SetupError(f"invalid staged SDK file: {member.name}")
                # Existing bytes must match the verified archive before reuse.
                # Append only the missing suffix, avoiding a second multi-GB
                # copy when resuming a disk-constrained interrupted extraction.
                incoming = source.extractfile(member)
                if incoming is None:
                    raise SetupError(f"cannot read SDK archive member: {member.name}")
                with incoming, output.open("rb") as existing:
                    while block := existing.read(1024 * 1024):
                        if incoming.read(len(block)) != block:
                            raise SetupError(f"staged SDK file differs from verified archive: {member.name}")
                    with output.open("ab") as target:
                        shutil.copyfileobj(incoming, target, length=1024 * 1024)
                continue
            try:
                source.extract(member, destination, filter="data")
            except TypeError:  # Python 3.11 before the data filter backport.
                source.extract(member, destination)


def _module_pc_files(sdk: pathlib.Path) -> bool:
    return all(
        any((sdk / subdir / f"{module}.pc").is_file() for subdir in ("lib/pkgconfig", "lib/gstreamer-1.0/pkgconfig"))
        for module in ANDROID_MODULES
    )


def select_android_root(extracted: pathlib.Path) -> pathlib.Path:
    """Find the upstream universal root and require both ABI SDK slices."""
    candidates = {extracted.resolve()}
    for pkgconfig in extracted.rglob("pkgconfig"):
        if pkgconfig.is_dir() and pkgconfig.parent.name == "lib":
            slice_root = pkgconfig.parent.parent.resolve()
            if slice_root.parent.is_dir():
                candidates.add(slice_root.parent.resolve())
    matches = []
    for candidate in candidates:
        slices = {}
        for key, aliases in ANDROID_ABIS.items():
            sdk = next((candidate / alias for alias in aliases if (candidate / alias / "lib/pkgconfig").is_dir()), None)
            if sdk is None and (candidate / "lib/pkgconfig").is_dir():
                sdk = candidate
            if sdk is not None and _module_pc_files(sdk):
                slices[key] = sdk
        if set(slices) == set(ANDROID_ABIS) and slices["arm64"] != slices["x86_64"]:
            matches.append(candidate)
    unique = sorted(set(matches))
    if len(unique) != 1:
        raise SetupError(f"expected one universal Android SDK root with arm64 and x86_64 slices, found {unique}")
    return unique[0]


def select_ios_xcframework(extracted: pathlib.Path) -> pathlib.Path:
    matches = [path for path in extracted.rglob("GStreamer.xcframework") if path.is_dir()]
    if len(matches) != 1:
        raise SetupError(f"expected one official GStreamer.xcframework, found {matches}")
    framework = matches[0]
    try:
        metadata = plistlib.loads((framework / "Info.plist").read_bytes())
        slices = {row["LibraryIdentifier"] for row in metadata.get("AvailableLibraries", [])}
    except (OSError, KeyError, plistlib.InvalidFileException) as error:
        raise SetupError(f"invalid GStreamer XCFramework metadata: {error}") from error
    if not set(IOS_SLICES).issubset(slices):
        raise SetupError(f"official iOS SDK does not include required slices: {slices}")
    for slice_name in IOS_SLICES:
        slice_root = framework / slice_name
        if not (slice_root / "libGStreamer.a").is_file() or not (slice_root / "Headers/gst/gst.h").is_file():
            raise SetupError(f"GStreamer XCFramework slice is incomplete: {slice_name}")
    return framework.resolve()


def prepare_archive_sdk(
    kind: str,
    root: pathlib.Path,
    artifact: dict,
    *,
    ios_platform_only: bool = False,
    android_platform_only: bool = False,
    resume_staging: pathlib.Path | None = None,
) -> pathlib.Path:
    if (ios_platform_only and kind != "ios") or (android_platform_only and kind != "android"):
        raise SetupError("SDK platform filter does not match the selected platform")
    root = root.expanduser().resolve()
    downloads = root.parent / "downloads"
    archive = download_artifact(artifact, downloads)
    marker = root / ".lingxi-gstreamer-sdk-sha256"
    if root.exists():
        if marker.is_file() and marker.read_text(encoding="ascii").strip() == artifact["sha256"]:
            selected = select_android_root(root / "archive") if kind == "android" else select_ios_xcframework(root / "archive")
            return selected
        raise SetupError(f"SDK output directory exists without a matching lock marker: {root}")
    root.parent.mkdir(parents=True, exist_ok=True)
    if resume_staging is not None:
        if not (ios_platform_only or android_platform_only) or resume_staging.is_symlink():
            raise SetupError("staging reuse requires platform-only extraction")
        staging_root = resume_staging.resolve(strict=True)
        if staging_root.parent != root.parent or not staging_root.name.startswith(f".{root.name}-"):
            raise SetupError("staging reuse must name this SDK's temporary sibling directory")
    else:
        staging_root = pathlib.Path(tempfile.mkdtemp(prefix=f".{root.name}-", dir=root.parent))
    try:
        staging = staging_root / "archive"
        staging.mkdir(exist_ok=resume_staging is not None)
        if staging.is_symlink() or staging.resolve().parent != staging_root:
            raise SetupError("SDK staging archive directory must stay inside its staging root")
        safe_extract_tar(archive, staging, ios_platform_only=ios_platform_only,
            android_platform_only=android_platform_only, resume_existing=resume_staging is not None)
        if kind == "android":
            select_android_root(staging)
        else:
            select_ios_xcframework(staging)
        (staging_root / ".lingxi-gstreamer-sdk-sha256").write_text(artifact["sha256"] + "\n", encoding="ascii")
        staging_root.replace(root)
    finally:
        if staging_root.exists() and resume_staging is None:
            shutil.rmtree(staging_root)
    return select_android_root(root / "archive") if kind == "android" else select_ios_xcframework(root / "archive")


def _append_github_file(variable: str, value: str) -> None:
    path = os.environ.get(variable)
    if not path:
        raise SetupError(f"{variable} is not set; run this command from a GitHub Actions job")
    if "\n" in value or "\r" in value:
        raise SetupError(f"cannot export a multiline {variable} value")
    with pathlib.Path(path).open("a", encoding="utf-8", newline="\n") as target:
        target.write(value + "\n")


def export_environment(values: dict[str, str], paths: tuple[str, ...] = ()) -> None:
    for key, value in values.items():
        _append_github_file("GITHUB_ENV", f"{key}={value}")
    for path in paths:
        _append_github_file("GITHUB_PATH", path)


def _run_pkg_config(
    executable: str,
    pkg_config_path: str,
    *,
    expected_plugins: tuple[str, ...] = (),
    expected_version: str | None = None,
    minimum_version: str | None = None,
) -> str:
    environment = os.environ.copy()
    environment["PKG_CONFIG_PATH"] = ""
    environment["PKG_CONFIG_LIBDIR"] = pkg_config_path
    modules = " ".join(PKG_CONFIG_MODULES)
    result = subprocess.run(
        [executable, "--exists", *PKG_CONFIG_MODULES],
        env=environment,
        text=True,
        capture_output=True,
    )
    if result.returncode:
        raise SetupError(f"GStreamer SDK pkg-config modules are unavailable: {modules}: {result.stderr.strip()}")
    version_result = subprocess.run(
        [executable, "--modversion", "gstreamer-1.0"],
        env=environment,
        text=True,
        capture_output=True,
        check=True,
    )
    version = version_result.stdout.strip()
    parsed = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?", version)
    if not parsed:
        raise SetupError(f"GStreamer SDK reported an invalid version: {version!r}")
    version_tuple = tuple(map(int, parsed.group(1, 2, 3)))
    if expected_version is not None and version != expected_version:
        raise SetupError(f"GStreamer SDK version {version} does not match pinned {expected_version}")
    if minimum_version is not None:
        minimum = re.fullmatch(r"(\d+)\.(\d+)(?:\.(\d+))?", minimum_version)
        if not minimum:
            raise SetupError(f"invalid GStreamer minimum version: {minimum_version}")
        minimum_tuple = tuple(map(int, (minimum.group(1), minimum.group(2), minimum.group(3) or "0")))
        if version_tuple < minimum_tuple:
            raise SetupError(f"GStreamer SDK version {version} is below required {minimum_version}")
    plugin_dir = subprocess.run(
        [executable, "--variable=pluginsdir", "gstreamer-1.0"],
        env=environment,
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    if not pathlib.Path(plugin_dir).is_dir():
        raise SetupError(f"GStreamer plugin directory is missing: {plugin_dir}")
    missing = [name for name in expected_plugins if not (pathlib.Path(plugin_dir) / name).is_file()]
    if missing:
        raise SetupError(f"GStreamer SDK is missing required plugins in {plugin_dir}: {missing}")
    return plugin_dir


def install_macos(lock: dict, cache_root: pathlib.Path) -> None:
    prefix = pathlib.Path("/Library/Frameworks/GStreamer.framework/Versions/1.0")
    pkg_config_path = str(prefix / "lib/pkgconfig")
    pkg_config = str(prefix / "bin/pkg-config")
    plugin_files = tuple(f"libgst{name}.dylib" for name in PLUGIN_NAMES)
    try:
        _run_pkg_config(
            pkg_config, pkg_config_path, expected_plugins=plugin_files,
            expected_version=lock["version"],
        )
    except (SetupError, FileNotFoundError, subprocess.CalledProcessError):
        for name in ("macos-runtime", "macos-devel"):
            package = download_artifact(lock["artifacts"][name], cache_root / "downloads")
            subprocess.run(["sudo", "/usr/sbin/installer", "-pkg", str(package), "-target", "/"], check=True)
    _run_pkg_config(
        pkg_config, pkg_config_path, expected_plugins=plugin_files,
        expected_version=lock["version"],
    )
    export_environment(
        {
            "LINGXI_GSTREAMER_SDK_MACOS": str(prefix),
            "PKG_CONFIG": pkg_config,
            "PKG_CONFIG_PATH": pkg_config_path,
            "PKG_CONFIG_LIBDIR": pkg_config_path,
        },
        (str(prefix / "bin"),),
    )


def install_windows(lock: dict, root: pathlib.Path) -> None:
    root = root.expanduser().resolve()
    marker = root / ".lingxi-gstreamer-sdk-version"
    if not root.exists():
        root.mkdir(parents=True)
        installer = download_artifact(lock["artifacts"]["windows"], root.parent / "downloads")
        subprocess.run(
            [str(installer), "/VERYSILENT", "/NORESTART", f"/DIR={root}", "/TYPE=devel", "/CURRENTUSER"],
            check=True,
        )
    elif not marker.is_file() or marker.read_text(encoding="ascii").strip() != lock["version"]:
        raise SetupError(f"Windows GStreamer SDK root is unmarked or stale; use a fresh CI temp path: {root}")
    pkg_config = root / "bin/pkg-config.exe"
    pkg_config_path = str(root / "lib/pkgconfig")
    plugin_files = tuple(f"gst{name}.dll" for name in PLUGIN_NAMES)
    if not pkg_config.is_file():
        raise SetupError(f"GStreamer development installer did not provide pkg-config.exe: {pkg_config}")
    plugin_dir = _run_pkg_config(
        str(pkg_config), pkg_config_path, expected_plugins=plugin_files,
        # The pinned MSVC installer adds its packaging revision to pkg-config's
        # version. Keep that fourth component exact as well as the release.
        expected_version=f"{lock['version']}.1",
    )
    marker.write_text(lock["version"] + "\n", encoding="ascii")
    export_environment(
        {
            "LINGXI_GSTREAMER_SDK_WINDOWS": str(root),
            "GSTREAMER_1_0_ROOT_MSVC_X86_64": str(root),
            "PKG_CONFIG": str(pkg_config),
            "PKG_CONFIG_PATH": pkg_config_path,
            "PKG_CONFIG_LIBDIR": pkg_config_path,
            "GST_PLUGIN_PATH": plugin_dir,
            "GST_PLUGIN_SYSTEM_PATH": plugin_dir,
        },
        (str(root / "bin"),),
    )


def _ldd_dependencies(binary: pathlib.Path) -> list[tuple[str, pathlib.Path]]:
    result = subprocess.run(["ldd", str(binary)], text=True, capture_output=True)
    if result.returncode:
        raise SetupError(f"ldd failed for GStreamer runtime file {binary}: {result.stderr.strip()}")
    dependencies = []
    for line in result.stdout.splitlines():
        if "not found" in line:
            raise SetupError(f"GStreamer runtime file has an unresolved dependency: {binary}: {line.strip()}")
        arrow = re.match(r"^\s*(\S+)\s+=>\s+(/\S+)", line)
        direct = re.match(r"^\s*(/\S+)\s+\(", line)
        if arrow:
            name, value = arrow.groups()
        elif direct:
            value = direct.group(1)
            name = pathlib.Path(value).name
        else:
            continue
        if not pathlib.Path(value).is_file():
            raise SetupError(f"ldd dependency is not a file: {value}")
        dependencies.append((name, pathlib.Path(value).resolve()))
    return dependencies


def _is_platform_library(name: str) -> bool:
    return any(fnmatch(name, pattern) for pattern in SYSTEM_LINUX_LIBRARIES)


def _package_owners(path: pathlib.Path) -> set[str]:
    result = subprocess.run(["dpkg-query", "-S", str(path)], text=True, capture_output=True)
    if result.returncode:
        return set()
    packages = set()
    for line in result.stdout.splitlines():
        owner = line.partition(": ")[0]
        package = owner.split(":", 1)[0]
        if package:
            packages.add(package)
    return packages


def stage_linux_sdk(root: pathlib.Path) -> None:
    if not sys.platform.startswith("linux"):
        raise SetupError("Linux GStreamer staging can only run on Linux")
    pkg_config = shutil.which("pkg-config")
    if not pkg_config:
        raise SetupError("pkg-config is required; install it and configure the pinned GStreamer SDK first")
    env = os.environ.copy()
    configured_pc_dir = env.get("PKG_CONFIG_LIBDIR") or env.get("PKG_CONFIG_PATH")
    if not configured_pc_dir:
        raise SetupError("PKG_CONFIG_LIBDIR or PKG_CONFIG_PATH must point to the pinned Linux GStreamer SDK")
    _run_pkg_config(pkg_config, configured_pc_dir, minimum_version="1.28")
    for module in PKG_CONFIG_MODULES:
        subprocess.run([pkg_config, "--exists", module], env=env, check=True)
    library_dir = pathlib.Path(subprocess.run(
        [pkg_config, "--variable=libdir", "gstreamer-1.0"], env=env, text=True, capture_output=True, check=True
    ).stdout.strip()).resolve()
    plugin_dir = pathlib.Path(subprocess.run(
        [pkg_config, "--variable=pluginsdir", "gstreamer-1.0"], env=env, text=True, capture_output=True, check=True
    ).stdout.strip()).resolve()
    if not library_dir.is_dir() or plugin_dir.parent != library_dir or plugin_dir.name != "gstreamer-1.0":
        raise SetupError(f"unexpected Linux GStreamer SDK layout: lib={library_dir}, plugins={plugin_dir}")
    marker = root / ".lingxi-gstreamer-linux-sdk"
    if root.exists():
        staged_library_dir = root / "lib" / library_dir.name
        complete = all((staged_library_dir / name).is_file() for name in RUNTIME_LIBRARIES)
        complete = complete and all(
            (staged_library_dir / "gstreamer-1.0" / f"libgst{name}.so").is_file()
            for name in PLUGIN_NAMES
        )
        complete = complete and (root / "share/licenses").is_dir()
        if marker.is_file() and marker.read_text(encoding="utf-8").strip() == str(plugin_dir) and complete:
            export_environment({"LINGXI_GSTREAMER_SDK_LINUX": str(root)})
            return
        raise SetupError(f"Linux SDK output directory exists without a matching marker: {root}")
    root.mkdir(parents=True)
    multiarch = library_dir.name
    staged_library_dir = root / "lib" / multiarch
    staged_plugin_dir = staged_library_dir / "gstreamer-1.0"
    staged_plugin_dir.mkdir(parents=True)

    runtime_roots: dict[str, pathlib.Path] = {}
    for name in GSTREAMER_RUNTIME_LIBRARIES:
        source = library_dir / name
        if not source.is_file():
            raise SetupError(f"required GStreamer runtime library is missing: {source}")
        runtime_roots[name] = source.resolve()
    for name, module in SYSTEM_RUNTIME_MODULES.items():
        source = library_dir / name
        if not source.is_file():
            system_library_dir = pathlib.Path(subprocess.run(
                [pkg_config, "--variable=libdir", module],
                env=env,
                text=True,
                capture_output=True,
                check=True,
            ).stdout.strip()).resolve()
            source = system_library_dir / name
        if not source.is_file():
            raise SetupError(f"required GStreamer runtime dependency is missing: {source}")
        runtime_roots[name] = source.resolve()

    queue: list[pathlib.Path] = list(runtime_roots.values())
    for name in PLUGIN_NAMES:
        source = plugin_dir / f"libgst{name}.so"
        if not source.is_file():
            raise SetupError(f"required GStreamer plugin is missing: {source}")
        shutil.copy2(source.resolve(), staged_plugin_dir / source.name)
        queue.append(source.resolve())

    copied: dict[str, pathlib.Path] = {}
    owner_packages: set[str] = set()
    processed: set[pathlib.Path] = set()
    while queue:
        source = queue.pop().resolve()
        if source in processed:
            continue
        processed.add(source)
        owner_packages.update(_package_owners(source))
        for name, dependency in _ldd_dependencies(source):
            if _is_platform_library(name):
                continue
            previous = copied.get(name)
            if previous is not None and previous != dependency:
                raise SetupError(f"Linux GStreamer SDK contains conflicting {name}: {previous} and {dependency}")
            if previous is None:
                copied[name] = dependency
                queue.append(dependency)

    # Preserve the expected GStreamer sonames as package paths and include the
    # transitive non-glibc dependencies required by the verified plugin set.
    for name, source in runtime_roots.items():
        previous = copied.get(name)
        if previous is not None and previous != source:
            raise SetupError(f"Linux GStreamer SDK contains conflicting {name}: {previous} and {source}")
        copied[name] = source
    missing_runtime = [name for name in RUNTIME_LIBRARIES if name not in copied]
    if missing_runtime:
        raise SetupError(f"GStreamer runtime dependency closure is missing required libraries: {missing_runtime}")
    for name, source in copied.items():
        if not re.fullmatch(r"lib.+\.so(?:\.\d+)*", name):
            continue
        destination = staged_library_dir / name
        if not destination.exists():
            shutil.copy2(source, destination)

    notices = root / "share/licenses"
    for package in sorted(owner_packages):
        copyright_file = pathlib.Path("/usr/share/doc") / package / "copyright"
        if not copyright_file.is_file():
            continue
        destination = notices / package / "copyright"
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(copyright_file, destination)
    source_prefix = os.environ.get("LINGXI_GSTREAMER_SDK_LINUX_SOURCE")
    if source_prefix:
        source_notices = pathlib.Path(source_prefix) / "share/licenses"
        if source_notices.is_dir():
            shutil.copytree(source_notices, notices / "GStreamer", dirs_exist_ok=True)
    if not notices.exists() or not any(path.is_file() for path in notices.rglob("*")):
        raise SetupError("could not stage GStreamer runtime license notices")

    marker.write_text(str(plugin_dir) + "\n", encoding="utf-8")
    export_environment({"LINGXI_GSTREAMER_SDK_LINUX": str(root)})


def _default_root(kind: str) -> pathlib.Path:
    runner_temp = pathlib.Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir()))
    return runner_temp / "gstreamer-sdk" / kind


def _require_runner_temp_root(root: pathlib.Path) -> pathlib.Path:
    resolved = root.expanduser().resolve()
    runner_temp = pathlib.Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir())).resolve()
    if not _inside(runner_temp, resolved):
        raise SetupError(f"SDK output must stay under RUNNER_TEMP ({runner_temp}): {resolved}")
    return resolved


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=("android", "ios", "macos", "windows", "linux"))
    parser.add_argument("--root", type=pathlib.Path)
    parser.add_argument("--ios-platform-only", action="store_true", help="extract required iOS device/simulator slices without unused tvOS slices")
    parser.add_argument("--android-platform-only", action="store_true", help="extract required Android arm64/x86_64 slices without unused 32-bit ABIs")
    parser.add_argument("--resume-staging", type=pathlib.Path, help="verify and finish an interrupted platform-only SDK staging directory")
    args = parser.parse_args()
    lock = load_lock()
    root = _require_runner_temp_root(args.root or _default_root(args.platform))
    if args.ios_platform_only and args.platform != "ios":
        parser.error("iOS extraction options require the ios platform")
    if args.android_platform_only and args.platform != "android":
        parser.error("Android extraction options require the android platform")
    if args.resume_staging and not (args.ios_platform_only or args.android_platform_only):
        parser.error("--resume-staging requires a platform-only extraction option")
    resume_staging = _require_runner_temp_root(args.resume_staging) if args.resume_staging else None
    if args.platform in ("android", "ios"):
        sdk_root = prepare_archive_sdk(args.platform, root, lock["artifacts"][args.platform],
            ios_platform_only=args.ios_platform_only, android_platform_only=args.android_platform_only,
            resume_staging=resume_staging)
        variable = "LINGXI_GSTREAMER_SDK_ANDROID" if args.platform == "android" else "LINGXI_GSTREAMER_SDK_IOS"
        export_environment({variable: str(sdk_root)})
    elif args.platform == "macos":
        install_macos(lock, root)
    elif args.platform == "windows":
        install_windows(lock, root)
    else:
        stage_linux_sdk(root)


if __name__ == "__main__":
    try:
        main()
    except (SetupError, OSError, subprocess.CalledProcessError) as error:
        print(f"gstreamer-sdk: ERROR: {error}", file=sys.stderr)
        raise SystemExit(1) from error
