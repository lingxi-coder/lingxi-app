#!/usr/bin/env python3
"""Cross-repository package identity and resource containment regressions."""

import copy
from pathlib import Path
import unittest

from mobile_linux_source import inspect_sdk_metadata


class MobileLinuxSourceTests(unittest.TestCase):
    def setUp(self):
        self.revision = "b" * 40
        self.repository = "https://github.com/lingxi-coder/mobile-linux-runtime.git"
        self.dependency = {"git": self.repository, "rev": self.revision}
        self.source = f"git+{self.repository}?rev={self.revision}#{self.revision}"
        self.inventory = {"repository": self.repository,
                          "packages": ["mobile-linux-api", "platform-pty"],
                          "vendored_packages": []}
        self.metadata = {"packages": [
            {"name": name, "source": self.source,
             "manifest_path": f"/tmp/locked-sdk/crates/{name}/Cargo.toml", "dependencies": []}
            for name in self.inventory["packages"]
        ]}

    def inspect(self):
        return inspect_sdk_metadata(self.metadata, self.dependency, self.inventory)

    def test_resolves_exact_source(self):
        self.assertEqual(self.inspect()["root"], str(Path("/tmp/locked-sdk").resolve()))

    def test_rejects_old_harness_pty_source(self):
        self.metadata["packages"][1]["source"] = self.source.replace("mobile-linux-runtime", "harness-runtime")
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_local_override(self):
        self.metadata["packages"][1]["source"] = None
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_duplicate_identity(self):
        self.metadata["packages"].append(copy.deepcopy(self.metadata["packages"][1]))
        with self.assertRaisesRegex(ValueError, "multiple Cargo identities"):
            self.inspect()

    def test_rejects_other_revision(self):
        self.metadata["packages"][1]["source"] = self.source.replace(self.revision, "c" * 40)
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_missing_anchor(self):
        self.metadata["packages"] = self.metadata["packages"][1:]
        with self.assertRaisesRegex(ValueError, "does not contain"):
            self.inspect()

    def test_rejects_resource_outside_checkout(self):
        self.metadata["packages"][1]["manifest_path"] = "/tmp/local-pty/Cargo.toml"
        with self.assertRaisesRegex(ValueError, "outside"):
            self.inspect()

    def test_rejects_inactive_alternate_edge(self):
        self.metadata["packages"].append({"name": "ios-framework", "dependencies": [
            {"name": "platform-pty", "source": "git+https://github.com/elsewhere/sdk", "target": "cfg(target_os = ios)"},
        ]})
        with self.assertRaisesRegex(ValueError, "another source"):
            self.inspect()

    def test_rejects_host_path_even_inside_locked_checkout(self):
        self.metadata["packages"].append({"name": "ios-framework", "dependencies": [
            {"name": "platform-pty", "path": "/tmp/locked-sdk/crates/platform-pty",
             "target": "cfg(target_os = ios)"},
        ]})
        with self.assertRaisesRegex(ValueError, "local dependency"):
            self.inspect()

    def test_accepts_internal_path_only_for_pinned_package(self):
        self.metadata["packages"][0]["dependencies"] = [
            {"name": "platform-pty", "path": "/tmp/locked-sdk/crates/platform-pty"},
        ]
        self.assertEqual(self.inspect()["revision"], self.revision)

    def test_rejects_branch_and_local_account_alias_in_manifest(self):
        for key, value in (("rev", "main"), ("git", "ssh://git@github.com-lingxi-coder/lingxi-coder/mobile-linux-runtime.git")):
            with self.subTest(key=key), self.assertRaises(ValueError):
                inspect_sdk_metadata(self.metadata, dict(self.dependency, **{key: value}), self.inventory)


if __name__ == "__main__":
    unittest.main()
