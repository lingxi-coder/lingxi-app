#!/usr/bin/env python3
"""Exercise the release artifact handoff through installed npm and wheel packages.

The executable fixtures are shell scripts, so this checks archive and package
permissions without claiming to test a signed macOS binary or a real CLI build.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
ARTIFACT_TOOL = ROOT / "scripts/ci/release_artifact.py"
NPM_BUILDER = ROOT / "packaging/npm/scripts/build_npm_package.py"


class ReleaseArtifactInstallationTest(unittest.TestCase):
    def command(self, args: list[str], *, cwd: Path, env: dict[str, str]) -> str:
        result = subprocess.run(args, cwd=cwd, env=env, text=True, capture_output=True, timeout=180)
        self.assertEqual(result.returncode, 0, f"{args!r}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}")
        return result.stdout

    def make_artifact(self, root: Path, target: str, env: dict[str, str]) -> Path:
        source = root / "source" / target
        cli = source / "release/lingxi-cli"
        client = source / "credential-broker/bin/lingxi-credential-client"
        broker = source / "credential-broker/LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker"
        for path, label in ((cli, "release-cli"), (client, "release-client"), (broker, "release-broker")):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"#!/bin/sh\nprintf '{label}:%s\\n' \"$*\"\n", encoding="utf-8")
            path.chmod(0o755)
        plist = broker.parent.parent / "Info.plist"
        plist.write_text("fixture metadata\n", encoding="utf-8")
        plist.chmod(0o644)
        link = source / "credential-broker/active-broker"
        link.symlink_to("LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker")

        archive = root / f"{target}.tar"
        self.command([sys.executable, str(ARTIFACT_TOOL), "pack", "--source", str(source),
                      "--archive", str(archive)], cwd=root, env=env)
        # GitHub's artifact download resets the outer file to 0644. The modes
        # and symlinks inside the tar must survive the actual release helper.
        archive.chmod(0o644)
        self.assertEqual(stat.S_IMODE(archive.stat().st_mode), 0o644)
        destination = root / "vendor" / target
        self.command([sys.executable, str(ARTIFACT_TOOL), "unpack", "--archive", str(archive),
                      "--destination", str(destination)], cwd=root, env=env)
        for relative in ("release/lingxi-cli", "credential-broker/bin/lingxi-credential-client",
                         "credential-broker/LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker"):
            self.assertEqual(stat.S_IMODE((destination / relative).stat().st_mode), 0o755, relative)
        self.assertEqual(stat.S_IMODE((destination / "credential-broker/LingXiCredentialBroker.app/Contents/Info.plist").stat().st_mode), 0o644)
        restored_link = destination / "credential-broker/active-broker"
        self.assertTrue(restored_link.is_symlink())
        self.assertEqual(os.readlink(restored_link), os.readlink(link))
        return destination

    def test_downloaded_tar_produces_executable_installed_packages(self) -> None:
        if os.name != "posix":
            self.fail("This release packaging check requires the POSIX package-and-publish runner")
        with tempfile.TemporaryDirectory(prefix="lingxi-release-install-") as directory:
            root = Path(directory)
            env = dict(os.environ, UV_CACHE_DIR=str(root / "uv-cache"),
                       NPM_CONFIG_CACHE=str(root / "npm-cache"), UV_PYTHON_DOWNLOADS="never")
            node_info = json.loads(self.command(
                ["node", "-p", "JSON.stringify({platform:process.platform,arch:process.arch})"], cwd=root, env=env))
            targets = {
                ("linux", "x64"): ("x86_64-unknown-linux-musl", "lingxi-linux-x64"),
                ("linux", "arm64"): ("aarch64-unknown-linux-musl", "lingxi-linux-arm64"),
                ("darwin", "x64"): ("x86_64-apple-darwin", "lingxi-darwin-x64"),
                ("darwin", "arm64"): ("aarch64-apple-darwin", "lingxi-darwin-arm64"),
            }
            identity = (node_info["platform"], node_info["arch"])
            self.assertIn(identity, targets, f"unsupported test host: {identity}")
            host_target, host_package = targets[identity]
            mac_target, mac_package = targets[("darwin", "arm64")]
            extracted = {target: self.make_artifact(root, target, env) for target in {host_target, mac_target}}

            # Stage and pack the actual release npm packages, then install only
            # those local tarballs. No registry access or publishing is needed.
            tarballs = []
            for name in dict.fromkeys(("lingxi", host_package, mac_package)):
                tarball = root / f"{name}.tgz"
                self.command([sys.executable, str(NPM_BUILDER), "--package", name,
                              "--release-version", "0.1.0", "--staging-dir", str(root / "npm-stage" / name),
                              "--vendor-src", str(root / "vendor"), "--pack-output", str(tarball)], cwd=root, env=env)
                tarballs.append(str(tarball))
            npm_install = root / "npm-install"
            npm_install.mkdir()
            self.command(["npm", "install", "--offline", "--ignore-scripts", "--omit=optional",
                          "--no-audit", "--no-fund", "--force", *tarballs], cwd=npm_install, env=env)
            launcher = npm_install / "node_modules/lingxi/bin/lingxi.js"
            self.assertEqual(self.command(["node", str(launcher), "npm-check"], cwd=root, env=env).strip(),
                             "release-cli:npm-check")
            mac_broker = npm_install / "node_modules" / mac_package / "vendor" / mac_target / "credential-broker"
            self.assertEqual(self.command([str(mac_broker / "bin/lingxi-credential-client"), "npm-check"], cwd=root, env=env).strip(),
                             "release-client:npm-check")
            self.assertEqual(self.command([str(mac_broker / "LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker"), "npm-check"], cwd=root, env=env).strip(),
                             "release-broker:npm-check")

            # Use the actual wheel project and hook. The host-compatible tag
            # permits executing our shell fixture on Linux or macOS; this does
            # not assert that a foreign-platform Mach-O runs on the host.
            project = root / "wheel-project"
            shutil.copytree(ROOT / "packaging/pypi", project,
                            ignore=shutil.ignore_patterns("dist", "build", ".venv", "__pycache__"))
            package_root = project / "src/lingxi_cli_bin"
            binary_dir = package_root / "bin"
            binary_dir.mkdir(parents=True, exist_ok=True)
            shutil.copy2(extracted[mac_target] / "release/lingxi-cli", binary_dir / "lingxi")
            shutil.copytree(extracted[mac_target] / "credential-broker", package_root / "credential-broker",
                            dirs_exist_ok=True)
            (package_root / "lingxi-package.json").write_text(json.dumps({"target": mac_target}), encoding="utf-8")
            wheel_env = dict(env)
            wheel_env.pop("LINGXI_CLI_BIN_PLATFORM_TAG", None)
            wheels = root / "wheels"
            self.command(["uv", "build", "--wheel", "--python", sys.executable, "--out-dir", str(wheels)],
                         cwd=project, env=wheel_env)
            wheel_files = list(wheels.glob("*.whl"))
            self.assertEqual(len(wheel_files), 1)
            wheel_install = root / "wheel-install"
            self.command(["uv", "pip", "install", "--offline", "--python", sys.executable, "--target",
                          str(wheel_install), str(wheel_files[0])], cwd=root, env=wheel_env)
            wheel_env["PYTHONPATH"] = str(wheel_install)
            output = self.command([sys.executable, "-c", "import lingxi_cli_bin; raise SystemExit(lingxi_cli_bin.main())",
                                   "wheel-check"], cwd=root, env=wheel_env)
            self.assertEqual(output.strip(), "release-cli:wheel-check")
            wheel_broker = wheel_install / "lingxi_cli_bin/credential-broker"
            self.assertEqual(self.command([str(wheel_broker / "bin/lingxi-credential-client"), "wheel-check"], cwd=root, env=wheel_env).strip(),
                             "release-client:wheel-check")
            self.assertEqual(self.command([str(wheel_broker / "LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker"), "wheel-check"], cwd=root, env=wheel_env).strip(),
                             "release-broker:wheel-check")


if __name__ == "__main__":
    unittest.main()
