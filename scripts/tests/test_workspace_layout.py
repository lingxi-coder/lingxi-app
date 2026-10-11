#!/usr/bin/env python3
"""Exercise product build entrypoints after moving the Cargo workspace root."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class WorkspaceLayoutTests(unittest.TestCase):
    def test_app_manifests_preserve_local_dependencies_and_sdk_boundaries(self) -> None:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
        expected = {
            "tools/ios-use": "tool-ios-use",
            "packages/config-requirements": "config-requirements",
            "apps/ios/ffi": "ios-framework",
            "apps/android/ffi": "android-aar",
            "apps/bridge-server": "bridge-server",
            "apps/cli/host": "cli",
            "apps/cli/tui-core": "tui-core",
            "apps/cli/tui": "tui",
        }
        self.assertEqual(set(workspace["workspace"]["members"]), set(expected))
        manifests = {(ROOT / path / "Cargo.toml").resolve() for path in expected}
        for relative, name in expected.items():
            with self.subTest(member=relative):
                directory = ROOT / relative
                manifest = tomllib.loads((directory / "Cargo.toml").read_text())
                self.assertEqual(manifest["package"]["name"], name)
                self.assertTrue((directory / "src").is_dir())
                for section in (manifest, *manifest.get("target", {}).values()):
                    for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                        for dependency in section.get(kind, {}).values():
                            if isinstance(dependency, dict) and "path" in dependency:
                                target = (directory / dependency["path"] / "Cargo.toml").resolve()
                                self.assertIn(target, manifests, f"local dependency escapes product members: {target}")
                                if relative.startswith("packages/"):
                                    self.assertFalse(target.is_relative_to(ROOT / "apps"))
        # Folder consolidation must not turn independent SDKs into path/submodule dependencies.
        self.assertNotIn("patch", workspace)
        sdk_dependencies = [value for value in workspace["workspace"]["dependencies"].values()
                            if isinstance(value, dict) and "git" in value]
        self.assertTrue(sdk_dependencies)
        for dependency in sdk_dependencies:
            self.assertTrue(dependency["git"].startswith("https://github.com/"))
            self.assertRegex(dependency["rev"], r"^[0-9a-f]{40}$")
            self.assertNotIn("path", dependency)
        for relative in ("apps/ios/native/project.yml", "apps/android/native/settings.gradle.kts",
                         "apps/android/native/app/build.gradle.kts", "apps/electron/package.json",
                         "packages/bridge-client/package.json", "resources/translations/generate.py",
                         "apps/setup.sh", "scripts/check-all.sh", "build-support/git_metadata.rs"):
            self.assertTrue((ROOT / relative).is_file(), relative)
        desktop = json.loads((ROOT / "apps/electron/package.json").read_text())
        dependency = desktop["dependencies"]["@lingxi/bridge-client"]
        self.assertTrue(dependency.startswith("file:"))
        self.assertEqual((ROOT / "apps/electron" / dependency.removeprefix("file:")).resolve(),
                         ROOT / "packages/bridge-client")

    def test_setup_bootstraps_packages_from_an_unrelated_working_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            host = root / "host"
            tools = root / "bin"
            tools.mkdir()
            for relative in ("apps/electron", "apps/android/native", "apps/ios/native", "packages/bridge-client"):
                (host / relative).mkdir(parents=True)
            shutil.copy2(ROOT / "apps/setup.sh", host / "apps/setup.sh")
            commands = {
                "cargo": "#!/bin/sh\nmkdir -p target/debug\nprintf '#!/bin/sh\\n' > target/debug/bridge-server\nchmod +x target/debug/bridge-server\n",
                "npm": "#!/bin/sh\nprintf '%s|%s\\n' \"$PWD\" \"$*\" >> \"$BOOTSTRAP_TRACE\"\nif [ \"$*\" = 'run build' ]; then mkdir -p dist; touch dist/index.js; fi\n",
                "node": "#!/bin/sh\nexit 0\n",
                "uname": "#!/bin/sh\nprintf 'Linux\\n'\n",
            }
            for name, source in commands.items():
                executable = tools / name
                executable.write_text(source)
                executable.chmod(0o755)
            trace = root / "bootstrap.log"
            environment = dict(os.environ, PATH=f"{tools}:/usr/bin:/bin", BOOTSTRAP_TRACE=str(trace),
                               ANDROID_NDK_HOME="", ANDROID_NDK_ROOT="", NDK_HOME="", ANDROID_HOME="")
            result = subprocess.run(["bash", str(host / "apps/setup.sh")], cwd=root,
                                    env=environment, capture_output=True, text=True, check=True)
            self.assertEqual(trace.read_text().splitlines(), [
                f"{host / 'packages/bridge-client'}|install",
                f"{host / 'packages/bridge-client'}|run build",
                f"{host / 'apps/electron'}|install",
            ])
            self.assertIn(str(host / "apps/android/native"), result.stdout)
            self.assertIn(str(host / "apps/ios/native"), result.stdout)
            self.assertTrue((host / "target/debug/bridge-server").is_file())

    def test_npm_staging_resolves_distribution_sources_from_any_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            vendor = root / "vendor"
            target = vendor / "aarch64-unknown-linux-musl/release"
            target.mkdir(parents=True)
            (target / "lingxi-cli").write_bytes(b"test CLI payload")
            for package in ("lingxi", "lingxi-linux-arm64"):
                with self.subTest(package=package):
                    staging = root / package
                    subprocess.run([sys.executable, str(ROOT / "packaging/npm/scripts/build_npm_package.py"),
                                    "--package", package, "--release-version", "0.1.0",
                                    "--staging-dir", str(staging), "--vendor-src", str(vendor)],
                                   cwd=root, capture_output=True, check=True)
                    manifest = json.loads((staging / "package.json").read_text())
                    self.assertEqual(manifest["name"], package)
                    self.assertEqual(manifest["version"], "0.1.0")
                    if package == "lingxi":
                        self.assertTrue((staging / "bin/lingxi.js").is_file())
                    else:
                        self.assertEqual((staging / "vendor/aarch64-unknown-linux-musl/bin/lingxi").read_bytes(),
                                         b"test CLI payload")


if __name__ == "__main__":
    _ = unittest.main()
