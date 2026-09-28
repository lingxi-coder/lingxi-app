#!/usr/bin/env python3
"""Identity regressions independent of the network and local Cargo cache."""

import copy
from pathlib import Path
import unittest

from runtime_source import inspect_metadata


class RuntimeSourceTests(unittest.TestCase):
    def setUp(self):
        self.revision = "a" * 40
        self.repository = "https://github.com/lingxi-coder/harness-runtime.git"
        self.dependency = {"git": self.repository, "rev": self.revision}
        self.source = f"git+{self.repository}?rev={self.revision}#{self.revision}"
        self.inventory = {"repository": self.repository, "packages": ["harness-runtime", "protocol"], "vendored_packages": ["git2"]}
        self.metadata = {"packages": [
            {"name": name, "source": self.source, "manifest_path": f"/tmp/locked-harness/crates/{name}/Cargo.toml", "dependencies": []}
            for name in ("harness-runtime", "protocol")
        ]}

    def inspect(self):
        return inspect_metadata(self.metadata, self.dependency, self.inventory)

    def test_accepts_one_pinned_source(self):
        result = self.inspect()
        self.assertEqual(result["root"], str(Path("/tmp/locked-harness").resolve()))
        self.assertEqual(result["revision"], self.revision)

    def test_accepts_renamed_runtime_crate_directory(self):
        self.metadata["packages"][0]["manifest_path"] = "/tmp/locked-harness/crates/runtime/Cargo.toml"
        self.assertEqual(self.inspect()["root"], str(Path("/tmp/locked-harness").resolve()))

    def test_rejects_unknown_runtime_crate_directory(self):
        self.metadata["packages"][0]["manifest_path"] = "/tmp/locked-harness/crates/other/Cargo.toml"
        with self.assertRaisesRegex(ValueError, "unexpected.*layout"):
            self.inspect()

    def test_rejects_unpinned_or_different_url(self):
        for field, value in (("rev", "main"), ("git", self.repository.removesuffix(".git"))):
            with self.subTest(field=field):
                dependency = dict(self.dependency, **{field: value})
                with self.assertRaises(ValueError):
                    inspect_metadata(self.metadata, dependency, self.inventory)

    def test_rejects_local_copy(self):
        self.metadata["packages"][1]["source"] = None
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_duplicate_identity(self):
        self.metadata["packages"].append(copy.deepcopy(self.metadata["packages"][1]))
        with self.assertRaisesRegex(ValueError, "multiple Cargo identities"):
            self.inspect()

    def test_rejects_alternate_revision(self):
        self.metadata["packages"][1]["source"] = self.source.replace(self.revision, "b" * 40)
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_manifest_outside_checkout(self):
        self.metadata["packages"][1]["manifest_path"] = "/tmp/other/protocol/Cargo.toml"
        with self.assertRaisesRegex(ValueError, "outside"):
            self.inspect()

    def test_rejects_inactive_host_path_dependency(self):
        self.metadata["packages"].append({"name": "ios-framework", "dependencies": [
            {"name": "protocol", "path": "/tmp/old-lingxi/protocol", "target": "cfg(target_os = ios)"},
        ]})
        with self.assertRaisesRegex(ValueError, "local dependency"):
            self.inspect()

    def test_rejects_host_path_into_the_cargo_checkout(self):
        self.metadata["packages"].append({"name": "ios-framework", "dependencies": [
            {"name": "protocol", "path": "/tmp/locked-harness/crates/protocol"},
        ]})
        with self.assertRaisesRegex(ValueError, "local dependency"):
            self.inspect()


if __name__ == "__main__":
    unittest.main()
