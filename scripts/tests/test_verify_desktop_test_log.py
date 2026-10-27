import importlib.util
import json
import os
import subprocess
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "ci" / "verify_desktop_test_log.py"
spec = importlib.util.spec_from_file_location("verify_desktop_test_log", SCRIPT)
assert spec is not None and spec.loader is not None
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


def sample_log(suite: str) -> str:
    passes = sorted(verify.REQUIRED[suite])
    skips = verify.LINUX_SKIPS if suite == "electron" else {}
    lines = [f"✔ {name} (1.25ms)" for name in passes]
    lines += [f"﹣ {name} (0.05ms) # {reason}" for name, reason in skips.items()]
    lines += [
        f"ℹ tests {len(passes) + len(skips)}",
        "ℹ suites 0",
        f"ℹ pass {len(passes)}",
        "ℹ fail 0",
        "ℹ cancelled 0",
        f"ℹ skipped {len(skips)}",
        "ℹ todo 0",
    ]
    return "\n".join(lines)


class VerifyDesktopTestLog(unittest.TestCase):
    def test_complete_sdk_and_linux_electron_runs_pass(self) -> None:
        for suite in ("sdk", "electron"):
            with self.subTest(suite=suite):
                self.assertEqual(verify.verify_log(sample_log(suite), suite), [])

    def test_actual_node_spec_reporter_is_understood(self) -> None:
        names = json.dumps(sorted(verify.REQUIRED["electron"]))
        skips = json.dumps(list(verify.LINUX_SKIPS.items()))
        program = (
            "import { test } from 'node:test';"
            f"for (const name of {names}) test(name, () => {{}});"
            f"for (const [name, reason] of {skips}) test(name, {{ skip: reason }}, () => {{}});"
        )
        env = os.environ.copy()
        env["NO_COLOR"] = "1"
        env.pop("FORCE_COLOR", None)
        env.pop("NODE_OPTIONS", None)
        result = subprocess.run(
            ["node", "--input-type=module", "-e", program],
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            env=env,
        )
        self.assertEqual(verify.verify_log(result.stdout, "electron"), [])

    def test_required_test_must_really_pass(self) -> None:
        name = next(iter(verify.REQUIRED["electron"]))
        missing = sample_log("electron").replace(f"✔ {name} (1.25ms)\n", "")
        self.assertTrue(any("required test did not pass" in e for e in verify.verify_log(missing, "electron")))
        skipped = sample_log("electron").replace(f"✔ {name} (1.25ms)", f"﹣ {name} (1.25ms) # SKIP")
        self.assertTrue(any("required test did not pass" in e for e in verify.verify_log(skipped, "electron")))

    def test_unknown_skip_and_wrong_skip_reason_fail(self) -> None:
        unknown = sample_log("electron") + "\n﹣ unexpected skipped integration (0.1ms) # SKIP\n"
        self.assertTrue(any("unexpected skipped test" in e for e in verify.verify_log(unknown, "electron")))
        wrong_reason = sample_log("electron").replace("# the isolated Electron 43 smoke runs on macOS", "# Electron dev binary is unavailable")
        self.assertTrue(any("unexpected skipped test" in e for e in verify.verify_log(wrong_reason, "electron")))

    def test_failure_or_cancellation_fails(self) -> None:
        for field in ("fail", "cancelled", "todo"):
            with self.subTest(field=field):
                log = sample_log("sdk").replace(f"ℹ {field} 0", f"ℹ {field} 1")
                self.assertTrue(any(f"Node reported 1 {field}" in e for e in verify.verify_log(log, "sdk")))

    def test_missing_or_inconsistent_summary_fails(self) -> None:
        missing = sample_log("sdk").replace("ℹ fail 0\n", "")
        self.assertTrue(any("summary: fail" in e for e in verify.verify_log(missing, "sdk")))
        inconsistent = sample_log("electron").replace("ℹ skipped 12", "ℹ skipped 11")
        self.assertTrue(any("skipped summary" in e for e in verify.verify_log(inconsistent, "electron")))

    def test_expected_platform_skips_and_reasons_are_visible(self) -> None:
        name, reason = next(iter(verify.LINUX_SKIPS.items()))
        missing = sample_log("electron").replace(f"﹣ {name} (0.05ms) # {reason}\n", "")
        self.assertTrue(any("expected Linux platform skip missing" in e for e in verify.verify_log(missing, "electron")))


if __name__ == "__main__":
    unittest.main()
