#!/usr/bin/env python3
"""Exercise product build entrypoints after moving the Cargo workspace root."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class WorkspaceLayoutTests(unittest.TestCase):
    def test_local_app_builds_use_locked_sources_and_host_output_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            host, upstream, sdk, tools = (root / name for name in ("host", "harness", "sdk", "bin"))
            scripts = host / "scripts/local-apps"
            scripts.mkdir(parents=True)
            (host / "scripts/lib").mkdir()
            (upstream / "scripts/local-apps").mkdir(parents=True)
            sdk.mkdir()
            tools.mkdir()
            for name, source in (("runtime_source.py", upstream), ("mobile_linux_source.py", sdk)):
                _ = (host / "scripts/lib" / name).write_text(f"print({str(source)!r})\n")
            python = tools / "python3"
            _ = python.write_text(f"#!{sys.executable}\nimport os, sys\nos.execv({sys.executable!r}, [{sys.executable!r}, *sys.argv[1:]])\n")
            python.chmod(0o755)
            environment = dict(os.environ, PATH=f"{tools}:{os.environ['PATH']}")
            for name in ("build-local-app-rootfs.sh", "build-local-app-node-modules.sh"):
                with self.subTest(entrypoint=name):
                    entrypoint = scripts / name
                    _ = shutil.copy2(ROOT / "scripts/local-apps" / name, entrypoint)
                    _ = (upstream / "scripts/local-apps" / name).write_text("#!/bin/bash\nprintf '%s\\0' \"$@\"\n")
                    result = subprocess.run(["bash", str(entrypoint), "--arch", "aarch64"],
                                            cwd=root, env=environment, capture_output=True, check=True)
                    arguments = result.stdout.decode().rstrip("\0").split("\0")
                    options = dict(zip(arguments[::2], arguments[1::2], strict=True))
                    self.assertEqual(options["--sdk-root"], str(sdk))
                    self.assertEqual(options["--arch"], "aarch64")
                    self.assertEqual(options["--cache-dir"], str(host / "clients/ios/build/local-app-cache"))
                    if name == "build-local-app-rootfs.sh":
                        self.assertEqual(options["--output-dir"], str(host / "clients/ios/build/local-app-rootfs"))
                    else:
                        self.assertEqual(options["--rootfs"], str(host / "clients/ios/build/local-app-rootfs/aarch64/rootfs.tar.gz"))
                        self.assertEqual(options["--output-dir"], str(host / "clients/ios/build/local-app-node-modules/aarch64"))
                    self.assertFalse((upstream / "target").exists())
                    self.assertFalse((sdk / "target").exists())


if __name__ == "__main__":
    _ = unittest.main()
