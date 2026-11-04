#!/usr/bin/env python3
"""Exercise real curated records against hostile snapshot/config/lock changes."""
import dataclasses
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("curated_audits", REPO / "scripts/check-curated-audits.py")
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)


class CuratedAuditRejectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        target = self.root / checker.SNAPSHOT_DIR
        target.parent.mkdir(parents=True)
        shutil.copytree(REPO / checker.SNAPSHOT_DIR, target)
        self.directory = target
        self.config = self.root / "supply-chain/config.toml"
        self.lock = self.root / "supply-chain/imports.lock"
        config = '[cargo-vet]\nversion = "0.10"\n'
        for name, url in checker.EXISTING_IMPORTS.items():
            config += f'\n[imports.{name}]\nurl = {json.dumps(url)}\n'
        for name, filename in checker.CURATED_IMPORTS.items():
            url = f"{checker.RAW_REPOSITORY}/{checker.SNAPSHOT_COMMIT}/{checker.SNAPSHOT_DIR.as_posix()}/{filename}"
            config += f'\n[imports.{name}]\nurl = {json.dumps(url)}\n'
        self.config.write_text(config)
        self.lock.write_text("[audits]\n")
        (self.root / "Cargo.lock").write_text("version = 4\n")
        self.anchors = checker.DEFAULT_ANCHORS

    def run_check(self):
        checker.validate(self.root, self.anchors)

    def update_snapshot_hash_in_test_policy(self, name):
        # Test semantic checks independently of the immutable digest checks.
        # The production CLI never accepts these synthetic trust anchors.
        path = self.directory / name
        new_hash = hashlib.sha256(path.read_bytes()).hexdigest()
        provenance_path = self.directory / "provenance.json"
        provenance = json.loads(provenance_path.read_text())
        provenance["files"][name]["sha256"] = new_hash
        provenance_path.write_text(json.dumps(provenance, indent=2) + "\n")
        self.anchors = dataclasses.replace(
            self.anchors,
            provenance_sha256=hashlib.sha256(provenance_path.read_bytes()).hexdigest(),
            snapshot_hashes={**self.anchors.snapshot_hashes, name: new_hash},
        )

    def test_reviewed_snapshots_and_immutable_config_pass(self):
        self.run_check()

    def test_snapshot_bytes_are_fixed_even_for_a_comment(self):
        path = self.directory / "zcash.toml"
        path.write_text(path.read_text() + "\n# changed\n")
        with self.assertRaisesRegex(checker.ValidationError, "snapshot SHA-256 mismatch"):
            self.run_check()

    def test_snapshot_wildcard_rejected_even_if_file_hashes_are_updated(self):
        path = self.directory / "isrg.toml"
        with path.open("a") as handle:
            handle.write('\n[[wildcard-audits.hmac]]\ncriteria = "safe-to-deploy"\nuser-id = 123\n')
        self.update_snapshot_hash_in_test_policy("isrg.toml")
        with self.assertRaisesRegex(checker.ValidationError, "only the audits table"):
            self.run_check()

    def test_forged_who_rejected_even_if_file_hashes_are_updated(self):
        path = self.directory / "isrg.toml"
        text = path.read_text()
        self.assertIn("David Cook <dcook@divviup.org>", text)
        path.write_text(text.replace("David Cook <dcook@divviup.org>", "Imposter <attacker@example.com>", 1))
        self.update_snapshot_hash_in_test_policy("isrg.toml")
        with self.assertRaisesRegex(checker.ValidationError, "differ from reviewed provenance"):
            self.run_check()

    def test_simultaneously_forged_provenance_is_not_a_new_trust_anchor(self):
        path = self.directory / "provenance.json"
        path.write_text(path.read_text().replace("David Cook", "Imposter", 1))
        with self.assertRaisesRegex(checker.ValidationError, "provenance SHA-256 mismatch"):
            self.run_check()

    def test_moving_branch_and_wrong_full_commit_are_rejected(self):
        original = self.config.read_text()
        for wrong in ("main", "a" * 40):
            with self.subTest(ref=wrong):
                self.config.write_text(original.replace(checker.SNAPSHOT_COMMIT, wrong))
                with self.assertRaisesRegex(checker.ValidationError, "pinned immutable snapshot URL"):
                    self.run_check()

    def test_unknown_direct_peer_and_custom_mapping_are_rejected(self):
        original = self.config.read_text()
        for extra in (
            '\n[imports.new-peer]\nurl = "https://example.com/main/audits.toml"\n',
            '\n[imports.curated-isrg.criteria-map]\ncustom = "safe-to-deploy"\n',
        ):
            with self.subTest(extra=extra):
                self.config.write_text(original + extra)
                with self.assertRaises(checker.ValidationError):
                    self.run_check()

    def test_only_the_explicit_builtin_criterion_is_accepted(self):
        path = self.directory / "isrg.toml"
        path.write_text(path.read_text().replace('criteria = "safe-to-deploy"', 'criteria = "safe-to-run"', 1))
        self.update_snapshot_hash_in_test_policy("isrg.toml")
        with self.assertRaisesRegex(checker.ValidationError, "only explicit safe-to-deploy"):
            self.run_check()

    def test_added_unreviewed_snapshot_is_rejected(self):
        (self.directory / "unexpected.toml").write_text("[audits]\n")
        with self.assertRaisesRegex(checker.ValidationError, "reviewed file set"):
            self.run_check()

    def test_lock_rejects_new_wildcard_and_forged_attestation(self):
        for body, reason in (
            ('[[audits.curated-isrg.wildcard-audits.hmac]]\ncriteria = "safe-to-deploy"\nuser-id = 42\n', "wildcard, trust"),
            ('[[audits.curated-isrg.audits.hmac]]\nwho = "Imposter"\ncriteria = "safe-to-deploy"\nversion = "0.12.1"\n', "unreviewed or duplicate"),
        ):
            with self.subTest(reason=reason):
                self.lock.write_text(body)
                with self.assertRaisesRegex(checker.ValidationError, reason):
                    self.run_check()

    def test_locked_violation_cannot_be_pruned_when_crate_is_present(self):
        (self.root / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "epaint"\nversion = "0.19.0"\n')
        with self.assertRaisesRegex(checker.ValidationError, "applicable violation missing"):
            self.run_check()
        provenance = json.loads((self.directory / "provenance.json").read_text())
        violation = next(x["record"] for x in provenance["records"] if x["crate"] == "epaint")
        body = '[[audits.curated-embark-studios.audits.epaint]]\n'
        body += "".join(f"{key} = {json.dumps(value)}\n" for key, value in violation.items())
        self.lock.write_text(body)
        self.run_check()

    def test_existing_peer_wildcard_remains_compatible(self):
        self.lock.write_text('[[audits.google.wildcard-audits.example]]\ncriteria = "safe-to-deploy"\nuser-id = 42\n')
        self.run_check()

    def test_each_known_git_fork_requires_its_name_policy(self):
        names = (
            "askama_derive", "askama_parser", "git2", "libgit2-sys", "libssh2-sys",
            "icu_collator", "lingxi-hyper", "lingxi-hyper-rustls", "lingxi-hyper-util",
            "lingxi-reqwest", "mbedtls-sys", "platform-pty", "syntect", "webrtc", "xcap",
        )
        original = self.config.read_text()
        for name in names:
            with self.subTest(package=name):
                revision = "a" * 40
                (self.root / "Cargo.lock").write_text(
                    f'version = 4\n[[package]]\nname = "{name}"\nversion = "1.0.0"\n'
                    f'source = "git+https://example.com/fork?rev={revision}#{revision}"\n'
                )
                self.config.write_text(original)
                with self.assertRaisesRegex(checker.ValidationError, "missing audit-as-crates-io policy"):
                    self.run_check()
                self.config.write_text(original + f'\n[policy."{name}"]\naudit-as-crates-io = true\n')
                self.run_check()

    def test_nix_policy_must_match_locked_version_and_full_git_revision(self):
        old_revision, new_revision = "a" * 40, "b" * 40
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "nix"\nversion = "0.31.3"\n'
            f'source = "git+https://example.com/fork?rev={new_revision}#{new_revision}"\n'
        )
        original = self.config.read_text()
        stale = f'\n[policy."nix:0.31.3@git:{old_revision}"]\naudit-as-crates-io = true\n'
        self.config.write_text(original + stale + '\n[policy.nix]\naudit-as-crates-io = true\n')
        with self.assertRaisesRegex(checker.ValidationError, "missing audit-as-crates-io policy"):
            self.run_check()
        current = f'\n[policy."nix:0.31.3@git:{new_revision}"]\naudit-as-crates-io = true\n'
        self.config.write_text(original + stale + current)
        self.run_check()
        locked = self.root / "Cargo.lock"
        locked.write_text(locked.read_text().replace('version = "0.31.3"', 'version = "0.31.4"'))
        with self.assertRaisesRegex(checker.ValidationError, "missing audit-as-crates-io policy"):
            self.run_check()
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "nix"\nversion = "0.31.3"\n'
            'source = "git+https://example.com/fork#short"\n'
        )
        with self.assertRaisesRegex(checker.ValidationError, "expected a full Git commit"):
            self.run_check()

    def test_registry_versions_do_not_require_git_fork_policies(self):
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "nix"\nversion = "0.29.0"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            '[[package]]\nname = "xcap"\nversion = "0.5.2"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
        )
        self.run_check()

    def test_exemptions_and_weakened_policy_are_rejected(self):
        original = self.config.read_text()
        for extra in ('\n[exemptions.hmac]\nversion = "0.12.1"\ncriteria = "safe-to-deploy"\n', '\n[policy.hmac]\ncriteria = []\n'):
            with self.subTest(extra=extra):
                self.config.write_text(original + extra)
                with self.assertRaises(checker.ValidationError):
                    self.run_check()


if __name__ == "__main__":
    unittest.main()
