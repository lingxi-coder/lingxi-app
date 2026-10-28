"""Regression tests for the checked-in protocol generation gate."""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import bridge_protocol_generator as generator


class ProtocolGeneratorCheckTests(unittest.TestCase):
    def test_detects_stale_or_missing_output_and_writes_exact_generated_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "protocol.generated.ts"
            with patch.object(generator, "ROOT", Path(tmp)), patch.object(generator, "OUTPUT", output), patch.object(
                generator, "generated_bytes", return_value=b"generated\n"
            ):
                with self.assertRaisesRegex(SystemExit, "stale"):
                    generator.check_or_write(Path(tmp), check=True)
                generator.check_or_write(Path(tmp), check=False)
                generator.check_or_write(Path(tmp), check=True)
                output.write_bytes(b"generated\r\n")
                generator.check_or_write(Path(tmp), check=True)
                output.write_bytes(b"generated but edited\n")
                with self.assertRaisesRegex(SystemExit, "stale"):
                    generator.check_or_write(Path(tmp), check=True)

    def test_invokes_locked_generator_on_resolved_runtime(self):
        with patch.object(generator.subprocess, "run") as run:
            run.return_value.stdout = b"typescript"
            self.assertEqual(generator.generated_bytes(Path("/resolved/runtime")), b"typescript")
            command = run.call_args.args[0]
            self.assertEqual(command[:3], ["cargo", "run", "--quiet"])
            self.assertIn("--locked", command)
            self.assertEqual(command[-2:], ["--", "/resolved/runtime"])
            self.assertTrue(run.call_args.kwargs["check"])
            self.assertNotEqual(Path(run.call_args.kwargs["cwd"]), generator.ROOT)
            self.assertIn("RUSTUP_TOOLCHAIN", run.call_args.kwargs["env"])

    def test_generator_tests_use_same_isolated_locked_cargo_path(self):
        with patch.object(generator.subprocess, "run") as run:
            generator.run_tests()
            command = run.call_args.args[0]
            self.assertEqual(command[:3], ["cargo", "test", "--quiet"])
            self.assertIn("--locked", command)
            self.assertNotEqual(Path(run.call_args.kwargs["cwd"]), generator.ROOT)


if __name__ == "__main__":
    unittest.main()
