#!/usr/bin/env python3
"""Local App repository identity regressions, independent of the network and the Cargo cache."""

import copy
from pathlib import Path
import unittest

import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from local_app_source import inspect_local_app_metadata


class LocalAppSourceTests(unittest.TestCase):
    def setUp(self):
        self.revision = "d" * 40
        self.repository = "https://github.com/lingxi-coder/local-app-builder"
        self.dependency = {"git": self.repository, "rev": self.revision}
        self.source = f"git+{self.repository}?rev={self.revision}#{self.revision}"
        self.inventory = {"repository": self.repository,
                          "packages": ["local-app-builder-contracts", "local-app-builder-service", "local-apps"],
                          "vendored_packages": []}
        self.metadata = {"packages": [
            {"name": name, "source": self.source,
             "manifest_path": f"/tmp/locked-local-app/crates/{name}/Cargo.toml", "dependencies": []}
            for name in self.inventory["packages"]
        ]}

    def inspect(self):
        return inspect_local_app_metadata(self.metadata, self.dependency, self.inventory)

    def test_resolves_exact_source(self):
        self.assertEqual(self.inspect()["root"], str(Path("/tmp/locked-local-app").resolve()))

    def test_rejects_a_branch_pin(self):
        with self.assertRaises(ValueError):
            inspect_local_app_metadata(self.metadata, dict(self.dependency, rev="main"), self.inventory)

    def test_rejects_a_different_url_spelling(self):
        with self.assertRaises(ValueError):
            inspect_local_app_metadata(self.metadata, dict(self.dependency, git=self.repository + ".git"), self.inventory)

    def test_rejects_local_override(self):
        self.metadata["packages"][1]["source"] = None
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_another_revision_of_one_package(self):
        # The runtime repository pinning an older Local App revision than this workspace is exactly this.
        self.metadata["packages"][2]["source"] = self.source.replace(self.revision, "e" * 40)
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_the_runtime_repository_declaring_another_revision(self):
        self.metadata["packages"].append({"name": "harness-runtime", "dependencies": [
            {"name": "local-app-builder-service",
             "source": f"git+{self.repository}?rev={'e' * 40}"},
        ]})
        with self.assertRaisesRegex(ValueError, "another source"):
            self.inspect()

    def test_accepts_the_runtime_repository_declaring_the_same_revision(self):
        self.metadata["packages"].append({"name": "harness-runtime", "dependencies": [
            {"name": "local-app-builder-service", "source": f"git+{self.repository}?rev={self.revision}"},
        ]})
        self.assertEqual(self.inspect()["revision"], self.revision)

    def test_rejects_duplicate_identity(self):
        self.metadata["packages"].append(copy.deepcopy(self.metadata["packages"][1]))
        with self.assertRaisesRegex(ValueError, "multiple Cargo identities"):
            self.inspect()

    def test_rejects_missing_anchor(self):
        self.metadata["packages"] = self.metadata["packages"][1:]
        with self.assertRaisesRegex(ValueError, "does not contain"):
            self.inspect()


if __name__ == "__main__":
    unittest.main()
