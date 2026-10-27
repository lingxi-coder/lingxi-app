#!/usr/bin/env python3
"""Exercise the Android workflow's rootfs-to-Gradle wiring without a native build."""

import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[4]
WORKFLOW = ROOT / ".github/workflows/clients.yml"
GRADLE = ROOT / "apps/android/native/app/build.gradle.kts"


def job_block(name):
    lines = WORKFLOW.read_text(encoding="utf-8").splitlines()
    start = lines.index(f"  {name}:")
    end = next(
        (i for i in range(start + 1, len(lines)) if re.match(r"^  [A-Za-z0-9_-]+:", lines[i])),
        len(lines),
    )
    return lines[start:end]


def run_script(lines, name):
    start = lines.index(f"      - name: {name}")
    run = next(i for i in range(start + 1, len(lines)) if lines[i] == "        run: |")
    end = next((i for i in range(run + 1, len(lines)) if lines[i] and not lines[i].startswith("          ")), len(lines))
    return "\n".join(line[10:] for line in lines[run + 1:end])


class AndroidRootfsCIWiringTests(unittest.TestCase):
    def test_fetched_evidence_is_passed_to_each_gradle_job(self):
        gradle = GRADLE.read_text(encoding="utf-8")
        declaration = re.search(
            r'val mobileLinuxRootfsEvidenceDir = providers\.gradleProperty\("([^"]+)"\)', gradle
        )
        self.assertIsNotNone(declaration, "Gradle must declare its rootfs evidence property")
        property_name = declaration.group(1)
        self.assertIn("inputs.dir(mobileLinuxRootfsEvidenceDir)", gradle)
        self.assertIn('"--input", mobileLinuxRootfsEvidenceDir.get().absolutePath', gradle)
        for job in ("android", "android-emulator"):
            with self.subTest(job=job), tempfile.TemporaryDirectory(prefix="android ci rootfs ") as temporary:
                root = Path(temporary)
                workspace = root / "workspace"
                android = workspace / "apps/android/native"
                android.mkdir(parents=True)
                binary_dir = root / "bin"
                binary_dir.mkdir()
                # These stubs leave the actual workflow shell and its argument
                # expansion intact, while avoiding downloads and native builds.
                fake_python = binary_dir / "python3"
                fake_python.write_text("""#!/bin/bash
set -eu
case "$1" in
  scripts/mobile-linux/fetch-sdk-rootfs.py)
    test "$2" = --output
    mkdir -p "$3"
    touch "$3/verified"
    ;;
  scripts/lib/mobile_linux_source.py|scripts/lib/runtime_source.py) echo /fixture/source ;;
  apps/android/native/scripts/stage-mobile-linux-assets.py) : ;;
  *) echo "unexpected Python invocation: $*" >&2; exit 1 ;;
esac
""", encoding="utf-8")
                fake_python.chmod(0o755)
                gradlew = android / "gradlew"
                gradlew.write_text("#!/bin/bash\nprintf '%s\\n' \"$@\" > \"$GRADLE_TRACE\"\n", encoding="utf-8")
                gradlew.chmod(0o755)
                trace = root / "gradle-args"
                runner_temp = root / "runner temp"
                env = {**os.environ, "PATH": f"{binary_dir}:{os.environ['PATH']}",
                       "RUNNER_TEMP": str(runner_temp), "GRADLE_TRACE": str(trace)}
                lines = job_block(job)
                fetch = run_script(lines, "Fetch verified SDK release rootfs assets")
                subprocess.run(["bash", "-e", "-c", fetch], cwd=workspace, env=env, check=True)
                evidence = runner_temp / "sdk-rootfs"
                self.assertTrue((evidence / "verified").is_file())
                if job == "android":
                    command = run_script(lines, "Gradle — Store/Full tests and APKs")
                    expected_tasks = (":app:assemblePlayDebug", ":app:assembleDirectDebug")
                else:
                    command = next(line.strip().removeprefix("script: ") for line in lines if line.strip().startswith("script: "))
                    expected_tasks = (":app:connectedPlayDebugAndroidTest",)
                shell = "bash" if job == "android" else "sh"
                subprocess.run([shell, "-e", "-c", command], cwd=android, env=env, check=True)
                args = trace.read_text(encoding="utf-8").splitlines()
                properties = [arg for arg in args if arg.startswith(f"-P{property_name}=")]
                self.assertEqual(properties, [f"-P{property_name}={evidence}"])
                for task in expected_tasks:
                    self.assertIn(task, args)


if __name__ == "__main__":
    unittest.main()
