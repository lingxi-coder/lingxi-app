#!/usr/bin/env python3
"""Offline regression fixtures for mobile GStreamer SDK path and cache safety."""
from __future__ import annotations

import importlib.util
import os
import pathlib
import plistlib
import shutil
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


sys.dont_write_bytecode = True
ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "mobile_gstreamer_cargo", ROOT / "scripts/lib/mobile_gstreamer_cargo.py"
)
assert SPEC and SPEC.loader
mobile_gstreamer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mobile_gstreamer)


def make_ios_xcframework(root: pathlib.Path) -> pathlib.Path:
    framework = root / "GStreamer.xcframework"
    framework.mkdir(parents=True)
    with (framework / "Info.plist").open("wb") as output:
        plistlib.dump(
            {
                "AvailableLibraries": [
                    {"LibraryIdentifier": identifier}
                    for identifier in ("ios-arm64", "ios-arm64_x86_64-simulator")
                ]
            },
            output,
        )
    device = framework / "ios-arm64"
    (device / "Headers/arm64").mkdir(parents=True)
    (device / "Headers/gst").mkdir(parents=True)
    (device / "libGStreamer.a").write_bytes(b"official SDK archive fixture")
    (device / "Headers/glib.h").write_text("/* fixture */\n", encoding="utf-8")
    (device / "Headers/arm64/glibconfig.h").write_text(
        "#define GLIB_MAJOR_VERSION 2\n"
        "#define GLIB_MINOR_VERSION 88\n"
        "#define GLIB_MICRO_VERSION 0\n",
        encoding="utf-8",
    )
    (device / "Headers/gst/gstversion.h").write_text(
        "#define GST_VERSION_MAJOR (1)\n"
        "#define GST_VERSION_MINOR (28)\n"
        "#define GST_VERSION_MICRO (8)\n",
        encoding="utf-8",
    )
    return framework


class MobileGStreamerCargoTests(unittest.TestCase):
    def test_android_abi_alias_symlink_cannot_escape_configured_sdk(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "android-sdk"
            outside = pathlib.Path(directory) / "outside-arm64"
            (outside / "lib/pkgconfig").mkdir(parents=True)
            root.mkdir()
            try:
                (root / "arm64").symlink_to(outside, target_is_directory=True)
            except OSError as error:
                self.skipTest(f"directory symlinks are unavailable: {error}")
            with self.assertRaises(SystemExit):
                mobile_gstreamer.resolve_android_sdk("aarch64-linux-android", str(root))

    def _assert_ios_link_rejected(self, link_kind: str) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            framework = make_ios_xcframework(base)
            outside = base / "outside"
            outside.mkdir()
            external_file = outside / "external"
            external_file.write_bytes(b"outside SDK")
            device = framework / "ios-arm64"
            try:
                if link_kind == "slice":
                    external_slice = outside / "ios-arm64"
                    (external_slice / "Headers/arm64").mkdir(parents=True)
                    (external_slice / "Headers/gst").mkdir(parents=True)
                    (external_slice / "libGStreamer.a").write_bytes(b"outside archive")
                    (external_slice / "Headers/glib.h").write_text("outside", encoding="utf-8")
                    (external_slice / "Headers/arm64/glibconfig.h").write_text(
                        "#define GLIB_MAJOR_VERSION 2\n", encoding="utf-8"
                    )
                    (framework / "ios-arm64").rename(framework / "ios-arm64-original")
                    (framework / "ios-arm64").symlink_to(external_slice, target_is_directory=True)
                elif link_kind == "archive":
                    (device / "libGStreamer.a").unlink()
                    (device / "libGStreamer.a").symlink_to(external_file)
                else:
                    (device / "Headers/glib.h").unlink()
                    (device / "Headers/glib.h").symlink_to(external_file)
            except OSError as error:
                self.skipTest(f"symlinks are unavailable: {error}")

            with patch.object(
                mobile_gstreamer.subprocess,
                "run",
                return_value=SimpleNamespace(returncode=0, stdout="arm64", stderr=""),
            ):
                with self.assertRaises(SystemExit):
                    mobile_gstreamer.ios_slice(framework, "aarch64-apple-ios")

    def test_ios_slice_symlink_cannot_escape_configured_xcframework(self) -> None:
        self._assert_ios_link_rejected("slice")

    def test_ios_static_archive_symlink_cannot_escape_configured_xcframework(self) -> None:
        self._assert_ios_link_rejected("archive")

    def test_ios_header_symlink_cannot_escape_configured_xcframework(self) -> None:
        self._assert_ios_link_rejected("header")

    def test_ios_targets_isolate_pkg_config_from_the_macos_host_sdk(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory).resolve()
            framework = make_ios_xcframework(base)
            device = framework / "ios-arm64"
            simulator = framework / "ios-arm64_x86_64-simulator"
            shutil.copytree(device, simulator)
            (simulator / "Headers/x86_64").mkdir()
            shutil.copy2(device / "Headers/arm64/glibconfig.h", simulator / "Headers/x86_64/glibconfig.h")
            macos = base / "macos-sdk"
            host_environment = {
                "LINGXI_GSTREAMER_SDK_MACOS": str(macos),
                "LINGXI_GSTREAMER_SDK_IOS": str(framework),
                "PKG_CONFIG": str(macos / "bin/pkg-config"),
                "PKG_CONFIG_PATH": str(macos / "lib/pkgconfig"),
                "PKG_CONFIG_LIBDIR": str(macos / "lib/pkgconfig"),
                "CARGO_TARGET_DIR": str(base / "cargo-target"),
            }
            commands = []
            pc_directories = set()

            def fake_run(command, *, env=None, **kwargs):
                if command[0] == "xcrun":
                    return SimpleNamespace(returncode=0, stdout="arm64 x86_64", stderr="")
                commands.append((command, env))
                return SimpleNamespace(returncode=0, stdout="", stderr="")

            with patch.dict(os.environ, host_environment), \
                    patch.object(mobile_gstreamer.subprocess, "run", side_effect=fake_run):
                for target, sdk in (
                    ("aarch64-apple-ios", device),
                    ("aarch64-apple-ios-sim", simulator),
                    ("x86_64-apple-ios", simulator),
                ):
                    with self.subTest(target=target):
                        command = ["cargo", "build", "--target", target]
                        mobile_gstreamer.run_mobile("ios", target, None, None, command)
                        actual_command, environment = commands[-1]
                        self.assertEqual(actual_command, command)
                        self.assertEqual(environment["PKG_CONFIG"], host_environment["PKG_CONFIG"])
                        self.assertEqual(environment["PKG_CONFIG_PATH"], "")
                        pc_dir = pathlib.Path(environment["PKG_CONFIG_LIBDIR"])
                        self.assertTrue(pc_dir.is_relative_to(base / "cargo-target/.mobile-gstreamer-pc"))
                        pc_directories.add(pc_dir)
                        for module in mobile_gstreamer.MODULES:
                            contents = (pc_dir / f"{module}.pc").read_text()
                            self.assertIn(f"prefix={sdk}\n", contents)
                            self.assertIn("Libs: -L${libdir} -lGStreamer\n", contents)
                            self.assertNotIn(str(macos), contents)
                        for name, value in host_environment.items():
                            self.assertEqual(os.environ[name], value)
            self.assertEqual(len(pc_directories), 3)

    def test_ios_cache_target_changes_when_static_archive_bytes_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            framework = make_ios_xcframework(base)
            archive = framework / "ios-arm64/libGStreamer.a"
            target = base / "cargo-target"
            commands = []

            def fake_run(command, *, env=None, **kwargs):
                if command[0] == "xcrun":
                    return SimpleNamespace(returncode=0, stdout="arm64", stderr="")
                commands.append((command, env))
                return SimpleNamespace(returncode=0, stdout="", stderr="")

            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}):
                with patch.object(mobile_gstreamer.subprocess, "run", side_effect=fake_run):
                    mobile_gstreamer.run_mobile("ios", "aarch64-apple-ios", str(framework), None, ["cargo", "check"])
                    first_target = commands[-1][1]["PKG_CONFIG_LIBDIR"]
                    archive.write_bytes(b"same GStreamer version, changed static archive bytes")
                    mobile_gstreamer.run_mobile("ios", "aarch64-apple-ios", str(framework), None, ["cargo", "check"])
                    second_target = commands[-1][1]["PKG_CONFIG_LIBDIR"]
            self.assertNotEqual(first_target, second_target)

    def test_ios_cache_target_changes_when_header_bytes_change_without_version_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            framework = make_ios_xcframework(base)
            header = framework / "ios-arm64/Headers/arm64/glibconfig.h"
            target = base / "cargo-target"
            commands = []

            def fake_run(command, *, env=None, **kwargs):
                if command[0] == "xcrun":
                    return SimpleNamespace(returncode=0, stdout="arm64", stderr="")
                commands.append((command, env))
                return SimpleNamespace(returncode=0, stdout="", stderr="")

            with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}):
                with patch.object(mobile_gstreamer.subprocess, "run", side_effect=fake_run):
                    mobile_gstreamer.run_mobile("ios", "aarch64-apple-ios", str(framework), None, ["cargo", "check"])
                    first_target = commands[-1][1]["PKG_CONFIG_LIBDIR"]
                    header.write_text(
                        "#define GLIB_MAJOR_VERSION 2\n"
                        "#define GLIB_MINOR_VERSION 88\n"
                        "#define GLIB_MICRO_VERSION 0\n"
                        "/* same reported version, changed SDK header */\n",
                        encoding="utf-8",
                    )
                    mobile_gstreamer.run_mobile("ios", "aarch64-apple-ios", str(framework), None, ["cargo", "check"])
                    second_target = commands[-1][1]["PKG_CONFIG_LIBDIR"]
            self.assertNotEqual(first_target, second_target)

    def test_android_pkg_config_identity_changes_when_static_archive_bytes_change(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = pathlib.Path(directory)
            universal = base / "android-sdk"
            sdk = universal / "arm64"
            pkgconfig = sdk / "lib/pkgconfig"
            pkgconfig.mkdir(parents=True)
            for module in mobile_gstreamer.MODULES:
                (pkgconfig / f"{module}.pc").write_text("Name: fixture\n", encoding="utf-8")
            archive = sdk / "lib/gstreamer-1.0/libgstapp.a"
            archive.parent.mkdir(parents=True)
            archive.write_bytes(b"first archive contents")
            ndk = base / "ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
            ndk.mkdir(parents=True)
            (ndk / "aarch64-linux-android24-clang").touch()
            target = base / "cargo-target"
            build_targets = []
            pkgconfig_identities = []

            def fake_run(command, *, env=None, **kwargs):
                build_targets.append(env["CARGO_TARGET_DIR"])
                pkgconfig_identities.append(env["PKG_CONFIG_LIBDIR"].split(os.pathsep)[-1])
                return SimpleNamespace(returncode=0, stdout="", stderr="")

            with patch.object(mobile_gstreamer.sys, "platform", "linux"):
                with patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}):
                    with patch.object(mobile_gstreamer.subprocess, "run", side_effect=fake_run):
                        mobile_gstreamer.run_mobile(
                            "android", "aarch64-linux-android", str(universal), str(base / "ndk"), ["cargo", "check"]
                        )
                        first_target = build_targets[-1]
                        first_identity = pkgconfig_identities[-1]
                        archive.write_bytes(b"same GStreamer version, changed Android archive contents")
                        mobile_gstreamer.run_mobile(
                            "android", "aarch64-linux-android", str(universal), str(base / "ndk"), ["cargo", "check"]
                        )
                        second_target = build_targets[-1]
                        second_identity = pkgconfig_identities[-1]
            self.assertEqual(first_target, str(target))
            self.assertEqual(first_target, second_target)
            self.assertIn(".mobile-gstreamer-pc/android/aarch64-linux-android/", first_identity)
            self.assertNotEqual(first_identity, second_identity)


if __name__ == "__main__":
    unittest.main()
