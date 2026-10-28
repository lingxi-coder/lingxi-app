#!/usr/bin/env python3
"""Check CI debug signing and Gradle scheduling without a native build."""

import hashlib
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
SIGNING_SCRIPT = HERE / "prepare-ci-debug-keystore.sh"
KEY_PROPERTY = "ORG_GRADLE_PROJECT_lingxiDebugKeystore"

spec = importlib.util.spec_from_file_location("rootfs_ci", HERE / "test-ci-rootfs-evidence.py")
rootfs_ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rootfs_ci)


class AndroidCIBuildTests(unittest.TestCase):
    def test_temporary_keys_can_sign_and_are_unique(self):
        with tempfile.TemporaryDirectory(prefix="android ci signing ") as temporary:
            root = Path(temporary)
            github_env = root / "github env"
            env = {**os.environ, "RUNNER_TEMP": str(root), "GITHUB_ENV": str(github_env)}
            keys = []
            certificates = []
            for _ in range(2):
                subprocess.run(["bash", str(SIGNING_SCRIPT)], env=env, check=True, capture_output=True)
                line = github_env.read_text(encoding="utf-8").splitlines()[-1]
                name, value = line.split("=", 1)
                self.assertEqual(name, KEY_PROPERTY)
                key = Path(value)
                self.assertTrue(key.is_relative_to(root))
                self.assertTrue(key.is_file())
                self.assertEqual(key.stat().st_mode & 0o077, 0)
                keys.append(key)
                certificate = subprocess.run(
                    ["keytool", "-exportcert", "-keystore", str(key), "-storetype", "PKCS12",
                     "-storepass", "android", "-alias", "androiddebugkey"],
                    check=True, capture_output=True,
                ).stdout
                certificates.append(hashlib.sha256(certificate).digest())
            self.assertNotEqual(keys[0], keys[1])
            self.assertNotEqual(certificates[0], certificates[1])

            unsigned = root / "unsigned.jar"
            signed = root / "signed.jar"
            with zipfile.ZipFile(unsigned, "w") as archive:
                archive.writestr("test.txt", "CI signing test")
            subprocess.run(
                ["jarsigner", "-keystore", str(keys[0]), "-storetype", "PKCS12",
                 "-storepass", "android", "-keypass", "android", "-signedjar", str(signed),
                 str(unsigned), "androiddebugkey"], check=True, capture_output=True,
            )
            subprocess.run(["jarsigner", "-verify", str(signed)], check=True, capture_output=True)

    def test_both_android_jobs_prepare_keys_and_keep_all_gradle_targets(self):
        gradle = (ROOT / "apps/android/native/app/build.gradle.kts").read_text(encoding="utf-8")
        self.assertIn('providers.gradleProperty("lingxiDebugKeystore")', gradle)
        self.assertIn('rootProject.file(".signing/lingxi-5e4b2-debug.keystore")', gradle)
        for job in ("android", "android-emulator"):
            with self.subTest(job=job), tempfile.TemporaryDirectory(prefix="android ci gradle ") as temporary:
                root = Path(temporary)
                lines = rootfs_ci.job_block(job)
                setup_java = next(i for i, line in enumerate(lines) if "actions/setup-java@" in line)
                prepare = lines.index("      - name: Create temporary Android debug signing key")
                self.assertGreater(prepare, setup_java)
                self.assertEqual(lines[prepare + 1].strip(),
                                 "run: bash apps/android/native/scripts/prepare-ci-debug-keystore.sh")
                android = root / "apps/android/native"
                android.mkdir(parents=True)
                trace = root / "gradle-args"
                wrapper = android / "gradlew"
                wrapper.write_text("#!/bin/bash\nprintf '%s\\0' \"$@\" >> \"$GRADLE_TRACE\"\nprintf '\\n' >> \"$GRADLE_TRACE\"\n", encoding="utf-8")
                wrapper.chmod(0o755)
                env = {**os.environ, "RUNNER_TEMP": str(root / "runner temp"), "GRADLE_TRACE": str(trace)}
                if job == "android":
                    commands = [rootfs_ci.run_script(lines, "Gradle — Store/Full tests and APKs")]
                    expected = [
                        {":app:testPlayDebugUnitTest", ":app:testDirectDebugUnitTest",
                         ":app:assemblePlayDebug", ":app:assembleDirectDebug",
                         ":app:assemblePlayDebugAndroidTest", ":app:assembleDirectDebugAndroidTest"},
                    ]
                else:
                    commands = [rootfs_ci.run_script(lines, "Assemble emulator APKs")]
                    script = next(line.strip().removeprefix("script: ") for line in lines
                                  if line.strip().startswith("script: "))
                    self.assertEqual(script, 'bash scripts/run-ci-emulator-tests.sh "$RUNNER_TEMP/sdk-rootfs"')
                    expected = [{":app:assemblePlayDebug", ":app:assemblePlayDebugAndroidTest",
                                 ":app:assembleDirectDebug", ":app:assembleDirectDebugAndroidTest"},
                                {"--stop"}]
                for command in commands:
                    subprocess.run(["bash", "-e", "-c", command], cwd=android, env=env, check=True)
                calls = [set(part.decode() for part in line.split(b"\0") if part)
                         for line in trace.read_bytes().splitlines()]
                self.assertEqual(len(calls), len(expected))
                for call, tasks in zip(calls, expected):
                    self.assertTrue(tasks <= call)
                    if "--stop" not in call:
                        self.assertIn("--max-workers=1", call)
                        self.assertIn("-Pkotlin.daemon.jvmargs=-Xmx4g", call)
                        self.assertIn(f"-PlingxiRootfsEvidenceDir={env['RUNNER_TEMP']}/sdk-rootfs", call)


if __name__ == "__main__":
    unittest.main()
