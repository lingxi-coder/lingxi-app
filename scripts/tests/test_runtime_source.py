#!/usr/bin/env python3
"""Identity regressions independent of the network and local Cargo cache."""

import copy
from pathlib import Path
import unittest

import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from runtime_source import inspect_metadata


class RuntimeSourceTests(unittest.TestCase):
    def setUp(self):
        self.revision = "a" * 40
        self.repository = "https://github.com/lingxi-coder/harness-runtime.git"
        self.dependency = {"git": self.repository, "rev": self.revision}
        self.source = f"git+{self.repository}?rev={self.revision}#{self.revision}"
        self.inventory = {"repository": self.repository, "packages": ["harness-runtime", "core", "platform-common", "platform-android", "platform-ios"], "vendored_packages": ["git2"]}
        layouts = {"harness-runtime": "runtime", "core": "core",
                   "platform-common": "platforms/common", "platform-android": "platforms/android",
                   "platform-ios": "platforms/ios"}
        self.metadata = {"packages": [
            {"name": name, "source": self.source,
             "manifest_path": f"/tmp/locked-harness/crates/{layout}/Cargo.toml", "dependencies": []}
            for name, layout in layouts.items()
        ]}

    def inspect(self):
        return inspect_metadata(self.metadata, self.dependency, self.inventory)

    def test_accepts_one_pinned_source(self):
        result = self.inspect()
        self.assertEqual(result["root"], str(Path("/tmp/locked-harness").resolve()))
        self.assertEqual(result["revision"], self.revision)

    def test_rejects_obsolete_runtime_layout(self):
        self.metadata["packages"][0]["manifest_path"] = "/tmp/locked-harness/crates/harness-runtime/Cargo.toml"
        with self.assertRaisesRegex(ValueError, "unexpected.*source layout"):
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
        self.metadata["packages"][1]["manifest_path"] = "/tmp/other/core/Cargo.toml"
        with self.assertRaisesRegex(ValueError, "outside"):
            self.inspect()

    def test_rejects_inactive_host_path_dependency(self):
        self.metadata["packages"].append({"name": "ios-framework", "dependencies": [
            {"name": "core", "path": "/tmp/old-lingxi/core", "target": "cfg(target_os = ios)"},
        ]})
        with self.assertRaisesRegex(ValueError, "local dependency"):
            self.inspect()

    def test_rejects_consolidated_platform_source_drift(self):
        for index in (2, 3, 4):
            metadata = copy.deepcopy(self.metadata)
            metadata["packages"][index]["source"] = None
            with self.subTest(package=metadata["packages"][index]["name"]):
                with self.assertRaisesRegex(ValueError, "different source"):
                    inspect_metadata(metadata, self.dependency, self.inventory)

    def test_rejects_inactive_alternate_platform_edge(self):
        self.metadata["packages"].append({"name": "android-aar", "dependencies": [
            {"name": "platform-android", "source": "git+https://github.com/elsewhere/runtime",
             "target": "cfg(target_os = android)"},
        ]})
        with self.assertRaisesRegex(ValueError, "another source"):
            self.inspect()

    def test_accepts_a_patched_crates_io_request_for_a_vendored_crate(self):
        # `local-apps` lives in another repository and cannot name this checkout's git2 by path.
        self.inventory["vendored_packages"] = ["git2"]
        self.metadata["packages"].append({"name": "git2", "source": self.source,
            "manifest_path": "/tmp/locked-harness/third_party/git2-rs/git2/Cargo.toml", "dependencies": []})
        self.metadata["packages"].append({"name": "local-apps", "dependencies": [
            {"name": "git2", "source": "registry+https://github.com/rust-lang/crates.io-index"},
        ]})
        self.assertEqual(self.inspect()["revision"], self.revision)

    def test_rejects_a_crates_io_request_for_a_crate_that_is_not_vendored(self):
        self.metadata["packages"].append({"name": "local-apps", "dependencies": [
            {"name": "core", "source": "registry+https://github.com/rust-lang/crates.io-index"},
        ]})
        with self.assertRaisesRegex(ValueError, "another source"):
            self.inspect()

    def test_an_unpatched_copy_of_a_vendored_crate_is_still_a_different_identity(self):
        self.inventory["vendored_packages"] = ["git2"]
        self.metadata["packages"].append({"name": "git2",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/tmp/registry/git2/Cargo.toml", "dependencies": []})
        with self.assertRaisesRegex(ValueError, "different source"):
            self.inspect()

    def test_rejects_host_path_into_the_cargo_checkout(self):
        self.metadata["packages"].append({"name": "ios-framework", "dependencies": [
            {"name": "core", "path": "/tmp/locked-harness/crates/core"},
        ]})
        with self.assertRaisesRegex(ValueError, "local dependency"):
            self.inspect()


if __name__ == "__main__":
    unittest.main()
