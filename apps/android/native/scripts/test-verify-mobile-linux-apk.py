#!/usr/bin/env python3
"""Architecture selection must never admit a stale second JNI library."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("verify_apk", Path(__file__).with_name("verify-mobile-linux-apk.py"))
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)


class ApkArchitectureTest(unittest.TestCase):
    def verify(self, packaged, requested=None):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            apk = root / "test.apk"
            with zipfile.ZipFile(apk, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for abi in packaged:
                    for name in verifier.native.SUPPORT | {"libandroid_aar.so"}:
                        payload = (abi + name).encode()
                        path = root / ("jniLibs" if name in verifier.native.SUPPORT else "host") / abi / name
                        path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_bytes(payload)
                        archive.writestr(f"lib/{abi}/{name}", payload)
            with patch.object(verifier.native, "verify"), patch.object(verifier, "staged_assets", return_value={}):
                verifier.verify_apk(apk, root, root / "host", root / "assets", requested)

    def test_single_abi_verifies_its_exact_native_payloads(self):
        self.verify(["arm64-v8a"], ["arm64-v8a"])

    def test_single_abi_rejects_stale_second_architecture(self):
        with self.assertRaisesRegex(ValueError, "ABIs differ"):
            self.verify(["arm64-v8a", "x86_64"], ["arm64-v8a"])

    def test_default_still_requires_both_architectures(self):
        self.verify(["arm64-v8a", "x86_64"])
        with self.assertRaises(FileNotFoundError):
            self.verify(["arm64-v8a"])

    def test_unknown_architecture_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "must contain"):
            self.verify(["arm64-v8a"], ["mips"])


if __name__ == "__main__":
    unittest.main()
