#!/usr/bin/env python3
"""Regression tests for the pinned Askama registry advisory sentinel."""

import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
script = Path(__file__).resolve().parents[1] / "checks/audit_askama_upstream.py"
spec = importlib.util.spec_from_file_location("audit_askama", script)
assert spec and spec.loader
audit_askama = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit_askama)
real_registry_archive = audit_askama.registry_archive


class AskamaAuditTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.archives = {}
        self.checksums = {}
        self.packages = []
        self.calls = []
        self.metadata_error = None
        self.audit_error = None
        checkout = self.root / "checkout"
        for name in ("askama", "askama_derive", "askama_parser"):
            (checkout / name / "src").mkdir(parents=True)
        (checkout / "askama/src/ascii_str.rs").write_text("ASCII source")
        (checkout / "askama/src/html.rs").write_text("HTML source")
        for name, target in (("askama_derive/src/ascii_str.rs", "askama/src/ascii_str.rs"),
                             ("askama_derive/src/html.rs", "askama/src/html.rs"),
                             ("askama_parser/src/ascii_str.rs", "askama/src/ascii_str.rs")):
            (checkout / name).write_bytes((checkout / target).read_bytes())
        for name in ("askama_derive", "askama_parser"):
            archive = self.root / f"{name}.crate"
            self.write_archive(archive, name)
            self.archives[name] = archive
            self.checksums[name] = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.packages.append({
                "name": name, "version": audit_askama.VERSION, "source": audit_askama.SOURCE,
                "manifest_path": str(self.root / "checkout" / name / "Cargo.toml"),
            })
        self.addCleanup(patch.stopall)
        patch.object(audit_askama, "ARCHIVES", self.checksums).start()
        patch.object(audit_askama, "registry_archive", side_effect=lambda name, directory: self.archives[name]).start()
        patch.object(audit_askama.subprocess, "run", side_effect=self.fake_run).start()

    def write_archive(self, path, name, revision=None, source_path=None):
        provenance = json.dumps({
            "git": {"sha1": revision or audit_askama.BASELINE_REVISION},
            "path_in_vcs": source_path or name,
        }).encode()
        with tarfile.open(path, "w:gz") as archive:
            member = tarfile.TarInfo(f"{name}-{audit_askama.VERSION}/.cargo_vcs_info.json")
            member.size = len(provenance)
            archive.addfile(member, io.BytesIO(provenance))

    def fake_run(self, command, **kwargs):
        self.calls.append(command)
        if command[:2] == ["cargo", "metadata"]:
            self.assertEqual(command[2:], ["--locked", "--all-features", "--format-version=1"])
            if self.metadata_error:
                raise subprocess.CalledProcessError(self.metadata_error, command)
            return subprocess.CompletedProcess(command, 0, stdout=json.dumps({"packages": self.packages}))
        self.assertEqual(command[:5], ["cargo", "audit", "--deny", "warnings", "--file"])
        directory = Path(kwargs["cwd"])
        lock = Path(command[5])
        self.assertEqual(lock, directory / "Cargo.lock")
        with lock.open("rb") as source:
            actual = tomllib.load(source)
        self.assertEqual(actual, {"version": 4, "package": [
            {"name": name, "version": audit_askama.VERSION, "source": audit_askama.REGISTRY_SOURCE, "checksum": checksum}
            for name, checksum in self.checksums.items()
        ]})
        with (directory / ".cargo/audit.toml").open("rb") as config:
            self.assertEqual(tomllib.load(config), {"advisories": {"ignore": []}})
        if self.audit_error:
            raise subprocess.CalledProcessError(self.audit_error, command)
        return subprocess.CompletedProcess(command, 0)

    def test_pinned_git_packages_and_two_registry_baselines(self):
        audit_askama.audit(self.root)
        self.assertEqual(len(self.calls), 2)

    def test_git_identity_count_version_source_and_layout_fail_closed(self):
        first, second = self.packages
        variants = [[], [first], [first, first, second],
                    [{**first, "version": "0.16.2"}, second],
                    [{**first, "source": audit_askama.REGISTRY_SOURCE}, second],
                    [{**first, "source": audit_askama.SOURCE.replace(audit_askama.REVISION, "a" * 40)}, second],
                    [{**first, "manifest_path": str(self.root / "checkout/other/Cargo.toml")}, second],
                    [first, {**second, "manifest_path": str(self.root / "other/askama_parser/Cargo.toml")}]]
        for packages in variants:
            with self.subTest(packages=packages):
                self.packages = packages
                self.calls.clear()
                with self.assertRaises(ValueError):
                    audit_askama.audit(self.root)
                self.assertEqual(len(self.calls), 1)

    def test_archive_hash_and_registry_provenance_fail_closed(self):
        name = "askama_parser"
        self.archives[name].write_bytes(b"corrupt archive")
        with self.assertRaisesRegex(ValueError, "checksum"):
            audit_askama.audit(self.root)
        self.assertEqual(len(self.calls), 1)
        self.write_archive(self.archives[name], name, revision="0" * 40)
        self.checksums[name] = hashlib.sha256(self.archives[name].read_bytes()).hexdigest()
        with self.assertRaisesRegex(ValueError, "Git baseline"):
            audit_askama.audit(self.root)
        self.assertEqual(len(self.calls), 2)

    def test_git_symlink_stub_fails_before_audit(self):
        for name in ("askama_derive/src/ascii_str.rs", "askama_derive/src/html.rs", "askama_parser/src/ascii_str.rs"):
            with self.subTest(name=name):
                link = self.root / "checkout" / name
                original = link.read_bytes()
                link.write_text("../../askama/src/link-target.rs")
                self.calls.clear()
                with self.assertRaisesRegex(ValueError, "did not resolve"):
                    audit_askama.audit(self.root)
                self.assertEqual(len(self.calls), 1)
                link.write_bytes(original)

    def test_checkout_only_does_not_fetch_or_run_audit(self):
        with patch.object(sys, "argv", ["audit_askama_upstream.py", "--checkout-only"]):
            self.assertEqual(audit_askama.main(), 0)
        self.assertEqual(len(self.calls), 1)

    def test_metadata_and_advisory_failures_propagate(self):
        for attribute, status, expected_calls in (("metadata_error", 2, 1), ("audit_error", 7, 2)):
            with self.subTest(stage=attribute):
                setattr(self, attribute, status)
                self.calls.clear()
                with self.assertRaises(subprocess.CalledProcessError) as failure:
                    audit_askama.audit(self.root)
                self.assertEqual(failure.exception.returncode, status)
                self.assertEqual(len(self.calls), expected_calls)
                setattr(self, attribute, None)

    def test_cli_propagates_audit_exit_code(self):
        with patch.object(audit_askama, "audit", side_effect=subprocess.CalledProcessError(7, ["cargo", "audit"])):
            with patch.object(sys, "argv", ["audit_askama_upstream.py"]):
                self.assertEqual(audit_askama.main(), 7)

    def test_registry_failure_does_not_fall_back(self):
        with patch.dict(os.environ, {"CARGO_HOME": str(self.root / "empty-cargo-home")}):
            with patch.object(audit_askama.urllib.request, "urlopen", side_effect=OSError("registry unavailable")):
                with self.assertRaisesRegex(OSError, "registry unavailable"):
                    real_registry_archive("askama_derive", self.root)


if __name__ == "__main__":
    unittest.main()
