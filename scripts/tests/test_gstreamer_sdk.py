#!/usr/bin/env python3
"""Offline fixtures for the pinned GStreamer CI SDK contract."""
from __future__ import annotations

import importlib.util
import io
import os
import pathlib
import plistlib
import re
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch


sys.dont_write_bytecode = True
ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("gstreamer_sdk", ROOT / "scripts/ci/gstreamer_sdk.py")
assert SPEC and SPEC.loader
gstreamer_sdk = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gstreamer_sdk)
sys.path.insert(0, str(ROOT / "scripts/ci"))
BUILD_SPEC = importlib.util.spec_from_file_location(
    "build_gstreamer_linux", ROOT / "scripts/ci/build_gstreamer_linux.py"
)
assert BUILD_SPEC and BUILD_SPEC.loader
build_gstreamer_linux = importlib.util.module_from_spec(BUILD_SPEC)
BUILD_SPEC.loader.exec_module(build_gstreamer_linux)


def job_block(path: pathlib.Path, job_name: str) -> str:
    lines = path.read_text(encoding="utf-8").splitlines()
    start = lines.index(f"  {job_name}:")
    end = next(
        (index for index in range(start + 1, len(lines)) if re.match(r"^  [A-Za-z0-9_-]+:", lines[index])),
        len(lines),
    )
    return "\n".join(lines[start:end])


class GStreamerSDKTests(unittest.TestCase):
    def test_lock_pins_official_1288_artifacts(self) -> None:
        lock = gstreamer_sdk.load_lock()
        expected_digests = {
            "android": "f19c6f971a956675402452fce0fe772d66dbd0d28d70999618355813ea593a99",
            "ios": "d34ae6bd55b6cb9959734299536003d315deea1ab46f84182113692528d04fb5",
            "macos-runtime": "d371ab04a7632713626836c2fc234e863cc98484c74c473eba99b0fbd9b3a3fe",
            "macos-devel": "e6d380bc17be7eccf13e0154567d4718fc9321da822408c7ca0cd808b3d4a3f7",
            "windows": "e4c41ee6ee9c6c32a45d198463ec08d79bac3bd5fa58adc93d13fe4a08abc9d9",
            "linux-source": "fd146dc03ea55ccfe78c1a91d35b71f20089209e173163f98237aad9f5ec7540",
            "linux-orc": "7ec912ab59af3cc97874c456a56a8ae1eec520c385ec447e8a102b2bd122c90c",
        }
        self.assertEqual(set(lock["artifacts"]), set(expected_digests))
        for name, artifact in lock["artifacts"].items():
            expected_prefix = (
                "https://gitlab.freedesktop.org/gstreamer/gstreamer/-/archive/"
                if name == "linux-source"
                else "https://gstreamer.freedesktop.org/"
            )
            self.assertTrue(artifact["url"].startswith(expected_prefix))
            self.assertEqual(artifact["sha256"], expected_digests[name])

    def test_download_verifier_rejects_changed_content(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "sdk.tar.xz"
            path.write_bytes(b"fixture archive")
            artifact = {"filename": path.name, "sha256": "0" * 64}
            with self.assertRaisesRegex(gstreamer_sdk.SetupError, "SHA-256 mismatch"):
                gstreamer_sdk.verify_artifact(path, artifact)

    def test_archive_extraction_rejects_parent_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            archive = pathlib.Path(directory) / "unsafe.tar.xz"
            destination = pathlib.Path(directory) / "extract"
            with tarfile.open(archive, "w:xz") as output:
                member = tarfile.TarInfo("../escape.txt")
                member.size = 4
                output.addfile(member, io.BytesIO(b"oops"))
            with self.assertRaisesRegex(gstreamer_sdk.SetupError, "unsafe SDK archive path"):
                gstreamer_sdk.safe_extract_tar(archive, destination)
            self.assertFalse((pathlib.Path(directory) / "escape.txt").exists())

    def test_android_root_requires_both_verified_abi_slices(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "android-sdk"
            for abi in ("arm64", "x86_64"):
                pkgconfig = root / abi / "lib/pkgconfig"
                pkgconfig.mkdir(parents=True)
                for module in gstreamer_sdk.ANDROID_MODULES:
                    (pkgconfig / f"{module}.pc").touch()
            self.assertEqual(gstreamer_sdk.select_android_root(root), root.resolve())
            (root / "x86_64").rename(root / "x86_64-missing")
            with self.assertRaisesRegex(gstreamer_sdk.SetupError, "arm64 and x86_64"):
                gstreamer_sdk.select_android_root(root)

    def test_android_root_does_not_reuse_one_slice_for_both_targets(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "android-sdk"
            pkgconfig = root / "lib/pkgconfig"
            pkgconfig.mkdir(parents=True)
            for module in gstreamer_sdk.ANDROID_MODULES:
                (pkgconfig / f"{module}.pc").touch()
            with self.assertRaisesRegex(gstreamer_sdk.SetupError, "arm64 and x86_64"):
                gstreamer_sdk.select_android_root(root)

    def test_ios_xcframework_fixture_requires_device_and_simulator_slices(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            framework = pathlib.Path(directory) / "GStreamer.xcframework"
            framework.mkdir()
            with (framework / "Info.plist").open("wb") as metadata:
                plistlib.dump(
                    {"AvailableLibraries": [{"LibraryIdentifier": name} for name in gstreamer_sdk.IOS_SLICES]},
                    metadata,
                )
            for name in gstreamer_sdk.IOS_SLICES:
                slice_root = framework / name
                (slice_root / "Headers/gst").mkdir(parents=True)
                (slice_root / "libGStreamer.a").touch()
                (slice_root / "Headers/gst/gst.h").touch()
            self.assertEqual(gstreamer_sdk.select_ios_xcframework(pathlib.Path(directory)), framework.resolve())

    def test_github_environment_exports_sdk_path_and_pkg_config_search_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            github_env = root / "github-env"
            github_path = root / "github-path"
            github_env.touch()
            github_path.touch()
            with patch.dict(os.environ, {"GITHUB_ENV": str(github_env), "GITHUB_PATH": str(github_path)}):
                gstreamer_sdk.export_environment(
                    {"LINGXI_GSTREAMER_SDK_IOS": str(root / "GStreamer.xcframework"), "PKG_CONFIG_PATH": str(root / "lib/pkgconfig")},
                    (str(root / "bin"),),
                )
            self.assertEqual(
                github_env.read_text(encoding="utf-8").splitlines(),
                [f"LINGXI_GSTREAMER_SDK_IOS={root / 'GStreamer.xcframework'}", f"PKG_CONFIG_PATH={root / 'lib/pkgconfig'}"],
            )
            self.assertEqual(github_path.read_text(encoding="utf-8").splitlines(), [str(root / "bin")])

    def test_desktop_pkg_config_probe_is_confined_to_the_configured_sdk(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            pkgconfig = root / "lib/pkgconfig"
            plugin_dir = root / "lib/gstreamer-1.0"
            pkgconfig.mkdir(parents=True)
            plugin_dir.mkdir(parents=True)
            (plugin_dir / "libgstapp.dylib").touch()
            observed = []

            def fake_run(command, *, env, text, capture_output, check=False):
                observed.append((command, env))
                stdout = "1.28.8" if "--modversion" in command else str(plugin_dir)
                return type("Result", (), {"returncode": 0, "stdout": stdout, "stderr": ""})()

            with patch.object(gstreamer_sdk.subprocess, "run", side_effect=fake_run):
                result = gstreamer_sdk._run_pkg_config(
                    "/sdk/bin/pkg-config", str(pkgconfig), expected_plugins=("libgstapp.dylib",)
                )
            self.assertEqual(result, str(plugin_dir))
            self.assertEqual(len(observed), 3)
            for _, environment in observed:
                self.assertEqual(environment["PKG_CONFIG_PATH"], "")
                self.assertEqual(environment["PKG_CONFIG_LIBDIR"], str(pkgconfig))

    def test_macos_installs_pinned_packages_when_existing_framework_is_stale(self) -> None:
        lock = gstreamer_sdk.load_lock()
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            installs = []
            artifacts = {}
            for artifact in lock["artifacts"].values():
                path = base / artifact["filename"]
                path.touch()
                artifacts[artifact["filename"]] = path

            def fake_run(command, **kwargs):
                if command[0] == "sudo":
                    installs.append(command)
                    return type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})()
                return type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})()

            with patch.object(gstreamer_sdk, "download_artifact", side_effect=lambda artifact, _: artifacts[artifact["filename"]]):
                with patch.object(gstreamer_sdk.subprocess, "run", side_effect=fake_run):
                    with patch.object(
                        gstreamer_sdk,
                        "_run_pkg_config",
                        side_effect=[gstreamer_sdk.SetupError("GStreamer SDK version 1.20.3 does not match pinned 1.28.8"), "/fixture/plugins"],
                    ) as probe:
                        with patch.object(gstreamer_sdk, "export_environment"):
                            gstreamer_sdk.install_macos(lock, base / "cache")

            self.assertEqual(len(installs), 2)
            self.assertEqual(probe.call_count, 2)
            self.assertTrue(any("gstreamer-1.0-1.28.8-universal.pkg" in call[3] for call in installs))
            self.assertTrue(any("gstreamer-1.0-devel-1.28.8-universal.pkg" in call[3] for call in installs))

    def test_windows_rejects_unmarked_or_stale_sdk_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "windows-sdk"
            root.mkdir()
            with self.assertRaisesRegex(gstreamer_sdk.SetupError, "unmarked or stale"):
                gstreamer_sdk.install_windows(gstreamer_sdk.load_lock(), root)

    def test_windows_installer_requires_the_exact_four_part_package_version(self) -> None:
        lock = gstreamer_sdk.load_lock()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / ".lingxi-gstreamer-sdk-version").write_text(lock["version"] + "\n", encoding="ascii")
            (root / "bin").mkdir()
            (root / "bin/pkg-config.exe").touch()
            plugin_dir = root / "lib/gstreamer-1.0"
            plugin_dir.mkdir(parents=True)
            for name in gstreamer_sdk.PLUGIN_NAMES:
                (plugin_dir / f"gst{name}.dll").touch()

            for version in ("1.28.8.1", "1.28.8.2", "1.28.9.1", "1.28.8", "1.28.8.1.1"):
                with self.subTest(version=version):
                    def fake_run(command, **kwargs):
                        stdout = version if "--modversion" in command else str(plugin_dir)
                        return type("Result", (), {"returncode": 0, "stdout": stdout, "stderr": ""})()

                    with patch.object(gstreamer_sdk.subprocess, "run", side_effect=fake_run), \
                            patch.object(gstreamer_sdk, "export_environment") as export:
                        if version == "1.28.8.1":
                            gstreamer_sdk.install_windows(lock, root)
                            export.assert_called_once()
                        else:
                            with self.assertRaises(gstreamer_sdk.SetupError):
                                gstreamer_sdk.install_windows(lock, root)
                            export.assert_not_called()

    def test_sdk_output_path_must_remain_under_runner_temp(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            with patch.dict(os.environ, {"RUNNER_TEMP": str(root)}):
                self.assertEqual(gstreamer_sdk._require_runner_temp_root(root / "gstreamer-sdk"), (root / "gstreamer-sdk").resolve())
                with self.assertRaisesRegex(gstreamer_sdk.SetupError, "must stay under RUNNER_TEMP"):
                    gstreamer_sdk._require_runner_temp_root(root.parent / "outside")

    def test_linux_source_build_enables_required_plugins_and_uses_only_offline_wraps(self) -> None:
        self.assertIn("-Dgstreamer-full-license=unknown", build_gstreamer_linux.MESON_OPTIONS)
        self.assertNotIn("-Dgst-full-license=unknown", build_gstreamer_linux.MESON_OPTIONS)
        for option in (
            "-Dgst-plugins-base:app=enabled",
            "-Dgst-plugins-base:audioconvert=enabled",
            "-Dgst-plugins-base:audioresample=enabled",
            "-Dgst-plugins-base:opus=enabled",
            "-Dgst-plugins-base:videoconvertscale=enabled",
            "-Dgst-plugins-good:rtp=enabled",
            "-Dgst-plugins-good:rtpmanager=enabled",
            "-Dgst-plugins-good:vpx=enabled",
        ):
            self.assertIn(option, build_gstreamer_linux.MESON_OPTIONS)
        self.assertIn("--wrap-mode=nodownload", (ROOT / "scripts/ci/build_gstreamer_linux.py").read_text(encoding="utf-8"))

    def test_linux_static_sdk_requires_archives_and_uses_their_own_metadata(self) -> None:
        lock = gstreamer_sdk.load_lock()
        self.assertNotEqual(build_gstreamer_linux.build_identity(lock), build_gstreamer_linux.build_identity(lock, True))
        self.assertNotIn("--default-library=both", build_gstreamer_linux.meson_options(False))
        self.assertIn("--default-library=both", build_gstreamer_linux.meson_options(True))
        with tempfile.TemporaryDirectory() as directory:
            prefix = pathlib.Path(directory)
            library_dir = prefix / build_gstreamer_linux.LIBDIR
            plugin_dir = library_dir / "gstreamer-1.0"
            plugin_pc = plugin_dir / "pkgconfig"
            plugin_pc.mkdir(parents=True)
            (library_dir / "pkgconfig").mkdir()
            (prefix / "share/licenses/GStreamer-1.28.8").mkdir(parents=True)
            for name in build_gstreamer_linux.RUNTIME_LIBRARIES:
                (library_dir / name).touch()
            for name in build_gstreamer_linux.PLUGIN_NAMES:
                (plugin_dir / f"libgst{name}.so").touch()

            with patch.object(build_gstreamer_linux, "_require_system_tool", return_value="pkg-config"), \
                    patch.object(build_gstreamer_linux, "_run_pkg_config"):
                with self.assertRaisesRegex(build_gstreamer_linux.SetupError, "missing static plugins"):
                    build_gstreamer_linux._validate_prefix(prefix, "1.28.8", True)

                for name in build_gstreamer_linux.PLUGIN_NAMES:
                    (plugin_dir / f"libgst{name}.a").touch()
                    (plugin_pc / f"gst{name}.pc").touch()
                (library_dir / "libgstreamer-1.0.a").touch()
                with self.assertRaisesRegex(build_gstreamer_linux.SetupError, "static core archives"):
                    build_gstreamer_linux._validate_prefix(prefix, "1.28.8", True)
                build_gstreamer_linux._remove_static_core_archives(prefix)
                self.assertFalse((library_dir / "libgstreamer-1.0.a").exists())
                self.assertTrue((plugin_dir / "libgstapp.a").exists())
                calls = []

                def fake_run(command, **kwargs):
                    calls.append(command)
                    stdout = str(plugin_pc) if "--variable=pcfiledir" in command else ""
                    return type("Result", (), {"stdout": stdout, "returncode": 0})()

                with patch.object(build_gstreamer_linux.subprocess, "run", side_effect=fake_run):
                    build_gstreamer_linux._validate_prefix(prefix, "1.28.8", True)
                self.assertEqual(sum("--static" in call for call in calls), len(build_gstreamer_linux.PLUGIN_NAMES))
                environment = build_gstreamer_linux._environment(prefix, True)
                self.assertIn(str(plugin_pc), environment["PKG_CONFIG_PATH"].split(os.pathsep))
                for suffix in ("", "_1_0"):
                    self.assertEqual(environment[f"GST_PLUGIN_PATH{suffix}"], "")
                    self.assertEqual(environment[f"GST_PLUGIN_SYSTEM_PATH{suffix}"], "")
                    self.assertEqual(environment[f"GST_REGISTRY{suffix}"], str(prefix / "static-plugin-registry.bin"))
                self.assertNotIn(str(plugin_pc), build_gstreamer_linux._environment(prefix)["PKG_CONFIG_PATH"].split(os.pathsep))

                with patch.object(build_gstreamer_linux.subprocess, "run", return_value=type("Result", (), {"stdout": "/unrelated", "returncode": 0})()):
                    with self.assertRaisesRegex(build_gstreamer_linux.SetupError, "outside the pinned SDK"):
                        build_gstreamer_linux._validate_prefix(prefix, "1.28.8", True)

    def test_linux_source_sdk_staging_resolves_system_glib_dependencies(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            source_prefix = base / "source-prefix"
            library_dir = source_prefix / "lib/x86_64-linux-gnu"
            plugin_dir = library_dir / "gstreamer-1.0"
            pkgconfig_dir = library_dir / "pkgconfig"
            dependency_dir = base / "system-libraries"
            pkgconfig_dir.mkdir(parents=True)
            plugin_dir.mkdir(parents=True)
            dependency_dir.mkdir()
            for name in gstreamer_sdk.GSTREAMER_RUNTIME_LIBRARIES:
                (library_dir / name).write_bytes(name.encode("ascii"))
            for name in gstreamer_sdk.PLUGIN_NAMES:
                (plugin_dir / f"libgst{name}.so").write_bytes(name.encode("ascii"))
            runtime_dependencies = []
            for name in ("libglib-2.0.so.0", "libgobject-2.0.so.0", "libgio-2.0.so.0"):
                path = dependency_dir / name
                path.write_bytes(name.encode("ascii"))
                runtime_dependencies.append((name, path.resolve()))
            (source_prefix / "share/licenses/upstream/COPYING").parent.mkdir(parents=True)
            (source_prefix / "share/licenses/upstream/COPYING").write_text("fixture notice\n", encoding="utf-8")

            def fake_run(command, *, env=None, **kwargs):
                if command[1] == "--variable=libdir":
                    result = dependency_dir if command[2] in {"glib-2.0", "gobject-2.0", "gio-2.0"} else library_dir
                    return type("Result", (), {"returncode": 0, "stdout": str(result), "stderr": ""})()
                if command[1] == "--variable=pluginsdir":
                    return type("Result", (), {"returncode": 0, "stdout": str(plugin_dir), "stderr": ""})()
                return type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})()

            output = base / "staged-sdk"
            with patch.object(gstreamer_sdk.sys, "platform", "linux"):
                with patch.object(gstreamer_sdk.shutil, "which", return_value="pkg-config"):
                    with patch.object(gstreamer_sdk.subprocess, "run", side_effect=fake_run):
                        with patch.object(gstreamer_sdk, "_run_pkg_config", return_value=str(plugin_dir)):
                            with patch.object(gstreamer_sdk, "_ldd_dependencies", return_value=runtime_dependencies):
                                with patch.object(gstreamer_sdk, "_package_owners", return_value=set()):
                                    with patch.object(gstreamer_sdk, "export_environment"):
                                        with patch.dict(os.environ, {
                                            "PKG_CONFIG_LIBDIR": str(pkgconfig_dir),
                                            "LINGXI_GSTREAMER_SDK_LINUX_SOURCE": str(source_prefix),
                                        }):
                                            gstreamer_sdk.stage_linux_sdk(output)

            staged_library_dir = output / "lib/x86_64-linux-gnu"
            for name in gstreamer_sdk.RUNTIME_LIBRARIES:
                self.assertTrue((staged_library_dir / name).is_file(), name)
            self.assertFalse((library_dir / "libglib-2.0.so.0").exists())
            self.assertEqual(
                (output / "share/licenses/GStreamer/upstream/COPYING").read_text(encoding="utf-8"),
                "fixture notice\n",
            )

    def test_workflows_provision_sdk_before_native_media_gates(self) -> None:
        ci = ROOT / ".github/workflows/ci.yml"
        unit = job_block(ci, "unit-tests")
        self.assertIn("runs-on: ubuntu-22.04", unit)
        self.assertIn("scripts/ci/build_gstreamer_linux.py", unit)
        self.assertIn("--static-plugins", unit)
        self.assertIn("gstreamer-linux-static-jammy-", unit)
        self.assertIn("gstreamer-sdk/linux-static", unit)
        self.assertLess(unit.index("Install pinned Meson"), unit.index("Build and validate the pinned GStreamer static SDK"))
        self.assertLess(unit.index("Build and validate the pinned GStreamer static SDK"), unit.index("cargo test --locked --workspace --all-features --no-fail-fast"))

        lint = job_block(ci, "lint")
        self.assertIn("runs-on: ubuntu-26.04", lint)
        self.assertIn("libgstreamer1.0-dev", lint)
        self.assertIn("libgstreamer-plugins-base1.0-dev", lint)
        self.assertIn("gstreamer1.0-plugins-good", lint)
        self.assertLess(lint.index("Install GStreamer development libraries"), lint.index("cargo clippy --locked --workspace"))

        clients = ROOT / ".github/workflows/clients.yml"
        for job_name, gate in (("android", "Build Store and Full native runtimes"), ("android-emulator", "Build Store native runtime"), ("ios", "Build XCFramework + Swift bindings")):
            block = job_block(clients, job_name)
            platform = "ios" if job_name == "ios" else "android"
            self.assertIn(f"scripts/ci/gstreamer_sdk.py {platform}", block)
            self.assertLess(block.index(f"scripts/ci/gstreamer_sdk.py {platform}"), block.index(gate))

        desktop = job_block(ROOT / ".github/workflows/desktop-packages.yml", "package")
        self.assertIn("runner: ubuntu-22.04", desktop)
        self.assertIn("actions/cache@v6", desktop)
        self.assertLess(desktop.index("Restore the pinned Linux GStreamer runtime SDK cache"), desktop.index("Build the pinned upstream Linux GStreamer SDK"))
        self.assertLess(desktop.index("Build the pinned upstream Linux GStreamer SDK"), desktop.index("Stage the Linux GStreamer package SDK"))
        for platform in ("linux", "macos", "windows"):
            self.assertIn(f"scripts/ci/gstreamer_sdk.py {platform}", desktop)
        cargo_gate = desktop.index("Check and test native Rust host")
        for setup in ("Stage the Linux GStreamer package SDK", "Install the pinned macOS GStreamer SDK", "Install the pinned Windows GStreamer SDK"):
            self.assertLess(desktop.index(setup), cargo_gate)


if __name__ == "__main__":
    unittest.main()
