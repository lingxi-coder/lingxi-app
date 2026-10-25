#!/usr/bin/env python3
"""The staged native provenance must match the active Cargo-locked SDK."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("mobile_linux_native", Path(__file__).with_name("mobile-linux-native.py"))
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


class SdkSourceIdentityTest(unittest.TestCase):
    def test_accepts_exact_locked_source_and_revision(self):
        with tempfile.TemporaryDirectory() as folder:
            sidecar = Path(folder) / "sdk-source.json"
            sidecar.write_text('{"source":"git+https://example.invalid/sdk?rev=bfd#bfd","revision":"bfd"}\n')
            native.verify_sdk_source(sidecar, "git+https://example.invalid/sdk?rev=bfd#bfd", "bfd")

    def test_rejects_stale_source_sidecar(self):
        with tempfile.TemporaryDirectory() as folder:
            sidecar = Path(folder) / "sdk-source.json"
            sidecar.write_text('{"source":"git+https://example.invalid/sdk?rev=ae1#ae1","revision":"ae1"}\n')
            with self.assertRaisesRegex(ValueError, "differs from the Cargo-locked SDK"):
                native.verify_sdk_source(sidecar, "git+https://example.invalid/sdk?rev=bfd#bfd", "bfd")

    def test_rejects_sidecar_with_unexpected_fields(self):
        with tempfile.TemporaryDirectory() as folder:
            sidecar = Path(folder) / "sdk-source.json"
            sidecar.write_text('{"source":"source","revision":"revision","dirty":true}\n')
            with self.assertRaisesRegex(ValueError, "differs from the Cargo-locked SDK"):
                native.verify_sdk_source(sidecar, "source", "revision")


if __name__ == "__main__":
    unittest.main()
