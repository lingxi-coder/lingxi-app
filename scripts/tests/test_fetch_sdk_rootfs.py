#!/usr/bin/env python3
"""Reject altered release payloads before publishing them to build inputs."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "local-apps/fetch-sdk-rootfs.py"
spec = importlib.util.spec_from_file_location("fetch_sdk_rootfs", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class FetchRootfsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sdk = self.root / "sdk"
        self.output = self.root / "output"
        pins = self.sdk / "docs/toolchains/runtime-pins.json"
        pins.parent.mkdir(parents=True)
        pins.write_text(json.dumps({"alpine": {"version": "3.24.2"}}), encoding="utf-8")
        self.payload = b"verified release payload"
        for abi in ("arm64-v8a", "x86_64"):
            evidence = self.sdk / "docs/mobile-linux/releases/3.24.2" / abi
            evidence.mkdir(parents=True)
            (evidence / "rootfs-manifest.json").write_text(json.dumps({"archive": {
                "filename": "rootfs.tar.gz", "size_bytes": len(self.payload),
                "sha256": hashlib.sha256(self.payload).hexdigest(),
            }}), encoding="utf-8")
        self.addCleanup(patch.stopall)
        patch.object(module, "resolve_sdk", return_value={"root": str(self.sdk)}).start()

    def test_same_size_corruption_cannot_replace_previous_archive(self):
        target = self.output / "arm64-v8a/rootfs.tar.gz"
        target.parent.mkdir(parents=True)
        target.write_bytes(b"previous invalid cache")
        corrupt = bytes([self.payload[0] ^ 1]) + self.payload[1:]
        with patch.object(module.urllib.request, "urlopen", return_value=io.BytesIO(corrupt)), \
                patch.object(module.subprocess, "run") as verify:
            with self.assertRaisesRegex(ValueError, "differs from locked SDK digest"):
                module.fetch(self.output)
            verify.assert_not_called()
        self.assertEqual(target.read_bytes(), b"previous invalid cache")
        self.assertFalse(target.with_suffix(".download").exists())
        self.assertFalse((target.parent / "rootfs-manifest.json").exists())

    def test_valid_cached_archives_use_locked_evidence_without_network(self):
        for abi in ("arm64-v8a", "x86_64"):
            target = self.output / abi / "rootfs.tar.gz"
            target.parent.mkdir(parents=True)
            target.write_bytes(self.payload)
        before = {str(p.relative_to(self.sdk)): p.read_bytes() for p in self.sdk.rglob("*") if p.is_file()}
        with patch.object(module.urllib.request, "urlopen", side_effect=AssertionError("unexpected download")), \
                patch.object(module.subprocess, "run") as verify:
            module.fetch(self.output)
            self.assertEqual(verify.call_count, 2)
        after = {str(p.relative_to(self.sdk)): p.read_bytes() for p in self.sdk.rglob("*") if p.is_file()}
        self.assertEqual(before, after)
        for abi in ("arm64-v8a", "x86_64"):
            self.assertEqual((self.output / abi / "rootfs-manifest.json").read_bytes(),
                             (self.sdk / "docs/mobile-linux/releases/3.24.2" / abi / "rootfs-manifest.json").read_bytes())

    def test_output_cannot_mutate_sdk_source(self):
        with self.assertRaisesRegex(ValueError, "overlaps immutable SDK"):
            module.fetch(self.sdk / "generated")


if __name__ == "__main__":
    unittest.main()
