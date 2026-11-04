#!/usr/bin/env python3
"""The namespace export must follow the selected branding source exactly."""

import sys
import tempfile
import contextlib
import io
import json
import os
import re
import subprocess
from pathlib import Path
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import export_product_namespace
import product_identity
from export_product_namespace import CONSTANTS, namespace_from
from runtime_source import PACKAGE_LIST, resolve_runtime


class ProductNamespaceTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        subprocess.run(['git', 'init', '-q', str(root)], check=True)
        branding = root / "crates/branding"
        (branding / "src").mkdir(parents=True)
        self.source = branding / "src/lib.rs"
        self.repository = json.loads(PACKAGE_LIST.read_text(encoding='utf-8'))["repository"]
        revision = "a" * 40
        self.resolved = {"revision": revision, "root": str(root), "development": False,
                         "source": f"git+{self.repository}?rev={revision}#{revision}",
                         "packages": {"branding": str(branding / "Cargo.toml")}}
        self.values = {
            "envPrefix": '"PRODUCT_"',
            "configDirEnv": '"PRODUCT_CONFIG_DIR"',
            "dotDir": '".product"',
            "globalConfigFile": '".product.json"',
            "managedDirMacOS": '"/Library/Application Support/Product"',
            "managedDirWindows": 'r"C:\\Program Files\\Product"',
            "managedDirUnix": '"/etc/product"',
            "localAppPreparationMarker": '".product-prepared.json"',
            "localAppPreparationDataDirectory": '".product-prepare-data"',
            "remoteAttachmentPrefix": '".product-remote-attachment-"',
        }

    def write_source(self):
        self.source.write_text("".join(
            f"pub const {name}: &str = {self.values[key]};\n" for key, name in CONSTANTS.items()
        ), encoding='utf-8')
        self.commit_source()

    def commit_source(self):
        root = self.resolved['root']
        subprocess.run(['git', '-C', root, 'add', 'crates/branding/src/lib.rs'], check=True)
        subprocess.run(['git', '-C', root, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                        'commit', '-qm', 'branding fixture'], check=True)
        revision = subprocess.check_output(['git', '-C', root, 'rev-parse', 'HEAD'], text=True).strip()
        self.resolved['revision'] = revision
        self.resolved['source'] = f'git+{self.repository}?rev={revision}#{revision}'

    def test_exports_each_value_and_preserves_windows_raw_string(self):
        self.write_source()
        value = namespace_from(self.resolved)
        self.assertEqual(value["sourceRevision"], self.resolved["revision"])
        self.assertEqual(value["envPrefix"], "PRODUCT_")
        self.assertEqual(value["managedDirWindows"], "C:\\Program Files\\Product")
        self.assertEqual(value["localAppPreparationMarker"], ".product-prepared.json")

    def test_missing_or_ambiguous_definition_fails_closed(self):
        self.write_source()
        original = self.source.read_text(encoding='utf-8')
        self.source.write_text(original + 'pub const ENV_PREFIX: &str = "OTHER_";\n', encoding='utf-8')
        self.commit_source()
        with self.assertRaisesRegex(ValueError, "expected one literal definition"):
            namespace_from(self.resolved)
        self.source.write_text(original.replace('pub const ENV_PREFIX:', 'const ENV_PREFIX:'), encoding='utf-8')
        self.commit_source()
        with self.assertRaisesRegex(ValueError, "expected one literal definition"):
            namespace_from(self.resolved)

    def test_unsafe_component_and_relative_managed_directory_fail_closed(self):
        for key, invalid in (("dotDir", '"../outside"'), ("managedDirUnix", '"relative"'),
                             ("envPrefix", '"lowercase"')):
            with self.subTest(key=key):
                saved = self.values[key]
                self.values[key] = invalid
                self.write_source()
                with self.assertRaises(ValueError):
                    namespace_from(self.resolved)
                self.values[key] = saved

    def test_resource_is_replaced_only_after_successful_export(self):
        self.write_source()
        output = Path(self.temp.name) / 'generated' / 'product-namespace.json'
        args = ['export_product_namespace.py', '--output', str(output)]
        with patch.object(sys, 'argv', args), patch.object(export_product_namespace, 'resolve_runtime', return_value=self.resolved):
            self.assertEqual(export_product_namespace.main(), 0)
            original = output.read_text(encoding='utf-8')
            self.assertEqual(json.loads(original)['envPrefix'], 'PRODUCT_')
            self.source.write_text('', encoding='utf-8')
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(export_product_namespace.main(), 1)
            self.assertEqual(output.read_text(encoding='utf-8'), original)

    def test_rejects_real_declared_local_patch_before_reading_branding(self):
        root = Path(self.temp.name)
        repository = json.loads(PACKAGE_LIST.read_text(encoding='utf-8'))["repository"]
        workspace = root / 'workspace'
        workspace.mkdir()
        (workspace / '.cargo').mkdir()
        (workspace / 'Cargo.toml').write_text(
            f'[workspace.dependencies]\nharness-runtime = {{ git = "{repository}", rev = "{"a" * 40}" }}\n', encoding='utf-8'
        )
        (workspace / '.cargo/config.toml').write_text(
            f'[patch."{repository}"]\nharness-runtime = {{ path = "{root / "crates/runtime"}" }}\n', encoding='utf-8'
        )
        runtime_manifest = root / 'crates/runtime/Cargo.toml'
        runtime_manifest.parent.mkdir(parents=True)
        runtime_manifest.write_text('[package]\nname = "harness-runtime"\nversion = "0.0.0"\n', encoding='utf-8')
        (root / 'Cargo.toml').write_text('[workspace]\n', encoding='utf-8')
        cargo_metadata = {"packages": [
            {"name": "harness-runtime", "source": None, "manifest_path": str(runtime_manifest), "dependencies": []},
            {"name": "branding", "source": None, "manifest_path": self.resolved['packages']['branding'], "dependencies": []},
        ]}
        bindir = root / 'bin'
        bindir.mkdir()
        cargo = bindir / 'cargo'
        cargo.write_text(f'#!{sys.executable}\nprint({json.dumps(json.dumps(cargo_metadata))})\n', encoding='utf-8')
        cargo.chmod(0o755)
        with patch.dict(os.environ, {'PATH': f'{bindir}{os.pathsep}{os.environ.get("PATH", "")}' }):
            resolved = resolve_runtime(workspace)
        self.assertTrue(resolved['development'])
        self.assertEqual(resolved['revision'], 'a' * 40)
        with self.assertRaisesRegex(ValueError, 'local patches are not supported'):
            namespace_from(resolved)

    def test_rejects_noncanonical_or_invalid_git_identity(self):
        self.write_source()
        for updates in ({'source': 'path+/foreign'}, {'revision': 'a' * 39},
                        {'packages': {'branding': '/foreign/branding/Cargo.toml'}}):
            with self.subTest(updates=updates), self.assertRaises(ValueError):
                namespace_from({**self.resolved, **updates})

    def test_rejects_mutated_source_in_locked_checkout(self):
        self.write_source()
        self.source.write_text(self.source.read_text(encoding='utf-8') + '// uncommitted change\n', encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'differs from its locked Git revision'):
            namespace_from(self.resolved)

    def test_checkout_swap_after_verification_cannot_forge_export(self):
        self.write_source()
        original_read_text = Path.read_text
        reads = []

        def racing_read_text(path, *args, **kwargs):
            text = original_read_text(path, *args, **kwargs)
            if path.resolve() == self.source.resolve():
                reads.append(path)
                if len(reads) == 1:
                    altered = text.replace('"PRODUCT_"', '"SPOOFED_"')
                    altered = altered.replace('"PRODUCT_CONFIG_DIR"', '"SPOOFED_CONFIG_DIR"')
                    self.source.write_text(altered, encoding='utf-8')
            return text

        with patch.object(Path, 'read_text', racing_read_text):
            value = namespace_from(self.resolved)
        self.assertEqual(len(reads), 1, 'the verified checkout must not be read again per field')
        self.assertIn('SPOOFED_', self.source.read_text(encoding='utf-8'))
        self.assertEqual(value['sourceRevision'], self.resolved['revision'])
        self.assertEqual(value['envPrefix'], 'PRODUCT_')
        self.assertEqual(value['configDirEnv'], 'PRODUCT_CONFIG_DIR')

    def test_checkout_swap_between_fields_cannot_mix_sources(self):
        self.write_source()
        original = self.source.read_text(encoding='utf-8')
        pure_parser = product_identity.parse_branding_constant
        snapshots = []

        def racing_parser(name, snapshot):
            value = pure_parser(name, snapshot)
            snapshots.append(snapshot)
            if len(snapshots) == 1:
                changed = original.replace('"PRODUCT_CONFIG_DIR"', '"SPOOFED_CONFIG_DIR"')
                changed = changed.replace('".product.json"', '".spoofed.json"')
                self.source.write_text(changed, encoding='utf-8')
            return value

        with patch.object(export_product_namespace, 'parse_branding_constant', racing_parser):
            value = namespace_from(self.resolved)
        self.assertEqual(len(snapshots), len(CONSTANTS))
        self.assertTrue(all(snapshot is snapshots[0] for snapshot in snapshots))
        self.assertIn('SPOOFED_CONFIG_DIR', self.source.read_text(encoding='utf-8'))
        self.assertEqual(value['sourceRevision'], self.resolved['revision'])
        self.assertEqual(value['configDirEnv'], 'PRODUCT_CONFIG_DIR')
        self.assertEqual(value['globalConfigFile'], '.product.json')

    def test_product_identity_readers_use_utf8_with_cp936_default(self):
        root = Path(self.temp.name)
        gradle = root / 'apps/android/native/app/build.gradle.kts'
        project = root / 'apps/ios/native/project.yml'
        authorization = root / 'scripts/mobile-linux/check-authorizations.sh'
        for path in (gradle, project, authorization):
            path.parent.mkdir(parents=True, exist_ok=True)
        gradle.write_text('// 😀\nnamespace = "com.product.app"\napplicationId = "com.product.app"\n', encoding='utf-8')
        project.write_text('# 😀\nname: Product\ntargets:\n  Product:\n    type: application\n'
                           '    settings:\n      base:\n        PRODUCT_BUNDLE_IDENTIFIER: com.product.app\n', encoding='utf-8')
        authorization.write_text('# 😀\nenabled="${PRODUCT_RUNTIME:-0}"\n', encoding='utf-8')
        self.source.write_text('// 😀\npub const PRODUCT_NAME: &str = "Product";\n', encoding='utf-8')
        original_read_text = Path.read_text

        def read_text_with_cp936_default(path, encoding=None, errors=None):
            return original_read_text(path, encoding=encoding or 'cp936', errors=errors)

        product_identity.android_package.cache_clear()
        product_identity.ios_bundle_id.cache_clear()
        product_identity.authorization_enabled_env.cache_clear()
        try:
            with patch.object(product_identity, 'HOST', root), \
                 patch.object(product_identity, 'locked_branding_source', return_value=self.source), \
                 patch.object(Path, 'read_text', read_text_with_cp936_default):
                self.assertEqual(product_identity.android_package(), 'com.product.app')
                self.assertEqual(product_identity.ios_bundle_id(), 'com.product.app')
                self.assertEqual(product_identity.branding_constant('PRODUCT_NAME'), 'Product')
                self.assertEqual(product_identity.authorization_enabled_env(), 'PRODUCT_RUNTIME')
        finally:
            product_identity.android_package.cache_clear()
            product_identity.ios_bundle_id.cache_clear()
            product_identity.authorization_enabled_env.cache_clear()

    def test_export_git_blob_uses_utf8_with_cp936_default(self):
        self.write_source()
        self.source.write_text('// 😀\n' + self.source.read_text(encoding='utf-8'), encoding='utf-8')
        self.commit_source()
        original_read_text = Path.read_text

        def read_text_with_cp936_default(path, encoding=None, errors=None):
            return original_read_text(path, encoding=encoding or 'cp936', errors=errors)

        with patch.object(Path, 'read_text', read_text_with_cp936_default):
            value = namespace_from(self.resolved)
        self.assertEqual(value['sourceRevision'], self.resolved['revision'])
        self.assertEqual(value['envPrefix'], 'PRODUCT_')

    def test_ios_staging_uses_explicit_override_and_fails_on_invalid_override(self):
        root = Path(self.temp.name)
        script = Path(__file__).resolve().parents[2] / 'apps/ios/native/scripts/stage-product-namespace.sh'
        bindir = root / 'python-bin'
        bindir.mkdir()
        log = root / 'selected-python'
        for name in ('python3', 'python3.14', 'python3.13', 'python3.12', 'python3.11'):
            stub = bindir / name
            stub.write_text('#!/bin/sh\nexit 1\n', encoding='utf-8')
            stub.chmod(0o755)
        override = root / 'preferred-python'
        override.write_text('#!/bin/sh\nif [ "$1" = "-c" ]; then exit 0; fi\nprintf override > "$PROBE_LOG"\n', encoding='utf-8')
        override.chmod(0o755)
        env = {**os.environ, 'PATH': str(bindir), 'SRCROOT': str(root / 'native'),
               'TARGET_BUILD_DIR': str(root / 'build'), 'UNLOCALIZED_RESOURCES_FOLDER_PATH': 'App.app',
               'PROBE_LOG': str(log)}
        def run(*args):
            return subprocess.run(['/bin/bash', str(script), *args], env=env, capture_output=True, text=True)
        self.assertEqual(run(str(override)).returncode, 0)
        self.assertEqual(log.read_text(encoding='utf-8'), 'override')
        log.unlink()
        self.assertNotEqual(run('relative-python').returncode, 0)
        self.assertNotEqual(run(str(bindir / 'python3')).returncode, 0)
        self.assertFalse(log.exists())
        self.assertNotEqual(run().returncode, 0)
        fallback = bindir / 'python3.13'
        fallback.write_text('#!/bin/sh\nif [ "$1" = "-c" ]; then exit 0; fi\nprintf fallback > "$PROBE_LOG"\n', encoding='utf-8')
        fallback.chmod(0o755)
        self.assertEqual(run().returncode, 0)
        self.assertEqual(log.read_text(encoding='utf-8'), 'fallback')

    def test_ios_targets_forward_the_existing_python_override(self):
        project = (Path(__file__).resolve().parents[2] / 'apps/ios/native/project.yml').read_text(encoding='utf-8')
        existing = re.search(r'FIXTURE_PYTHON="(\$\{[^}]+\})"', project)
        self.assertIsNotNone(existing)
        staged = re.findall(r'script: /bin/bash "\$\{SRCROOT\}/scripts/stage-product-namespace\.sh" "([^"]+)"', project)
        self.assertEqual(staged, [existing.group(1), existing.group(1)])


if __name__ == "__main__":
    unittest.main()
