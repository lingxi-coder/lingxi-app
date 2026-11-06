import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'mobile-linux/prepare-sdk-release.py'
spec = importlib.util.spec_from_file_location('prepare_release', SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleasePreparationTests(unittest.TestCase):
    def rootfs_fixture(self, directory):
        rootfs, sdk = directory / 'rootfs', directory / 'sdk'
        source_archive = directory / 'source/rootfs.tar.gz'
        files = {
            source_archive: b'pinned toolchain archive',
            rootfs / 'resources/alpine-rootfs.zip': b'prepared iOS fakefs image',
            rootfs / 'resources/libvdso.so.elf': b'vdso',
            sdk / 'native/ios/sources.json': b'{}',
        }
        for path, data in files.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (rootfs / 'resources/RootfsPatch.bundle').mkdir()
        pins = sdk / 'docs/toolchains/runtime-pins.json'
        pins.parent.mkdir(parents=True)
        pins.write_text(json.dumps({'rootfs_version': 'user2'}))
        archive = {'sha256': release.sha(source_archive),
                   'size_bytes': source_archive.stat().st_size}
        evidence = sdk / 'docs/mobile-linux/releases/user2/arm64-v8a/rootfs-manifest.json'
        evidence.parent.mkdir(parents=True)
        evidence.write_text(json.dumps({'archive': archive}))
        source = {'source': 'git+sdk#pinned', 'revision': 'pinned'}
        manifest = {'sdk_source': source, 'rootfs_version': 'user2',
                    'source_archive_sha256': archive['sha256'],
                    'source_manifest_sha256': release.sha(sdk / 'native/ios/sources.json'),
                    'rootfs_zip_sha256': release.sha(rootfs / 'resources/alpine-rootfs.zip')}
        receipt = {'schema_version': 1, 'profile': 'toolchain', 'architecture': 'aarch64',
                   'archive_sha256': archive['sha256'], 'source_pins_sha256': release.sha(pins)}
        (rootfs / 'manifest.json').write_text(json.dumps(manifest))
        (rootfs / 'archive-verification.json').write_text(json.dumps(receipt))
        return rootfs, sdk, source, source_archive

    def test_toolchain_rootfs_uses_pinned_profile_verifier(self):
        with tempfile.TemporaryDirectory() as temporary:
            rootfs, sdk, source, source_archive = self.rootfs_fixture(Path(temporary))
            with patch.object(release.subprocess, 'run') as run:
                manifest = release.validate_ios_rootfs(rootfs, sdk, source, source_archive)
            self.assertEqual(source, manifest['sdk_source'])
            command = run.call_args.args[0]
            self.assertEqual(str(sdk / 'scripts/rootfs/verify-rootfs-profile.py'), command[1])
            self.assertEqual('toolchain', command[command.index('--profile') + 1])
            self.assertEqual(str(source_archive), command[command.index('--archive') + 1])
            self.assertFalse((rootfs / 'rootfs.tar.gz').exists())
            self.assertTrue(run.call_args.kwargs['check'])

    def test_stale_profile_or_source_is_rejected_before_verifier(self):
        cases = [('manifest.json', 'sdk_source', {'revision': 'other'}),
                 ('manifest.json', 'source_archive_sha256', 'base-minirootfs'),
                 ('archive-verification.json', 'profile', 'base'),
                 ('archive-verification.json', 'architecture', 'x86_64'),
                 ('archive-verification.json', 'source_pins_sha256', 'other-pins')]
        for name, key, value in cases:
            with self.subTest(key=key), tempfile.TemporaryDirectory() as temporary:
                rootfs, sdk, source, source_archive = self.rootfs_fixture(Path(temporary))
                path = rootfs / name
                document = json.loads(path.read_text())
                document[key] = value
                path.write_text(json.dumps(document))
                with patch.object(release.subprocess, 'run') as run:
                    with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                        release.validate_ios_rootfs(rootfs, sdk, source, source_archive)
                    run.assert_not_called()

    def test_missing_native_resource_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            rootfs, sdk, source, source_archive = self.rootfs_fixture(Path(temporary))
            (rootfs / 'resources/libvdso.so.elf').unlink()
            with patch.object(release.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'native resources are missing'):
                    release.validate_ios_rootfs(rootfs, sdk, source, source_archive)
                run.assert_not_called()

    def test_bundled_archive_cannot_replace_verified_sdk_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            rootfs, sdk, source, source_archive = self.rootfs_fixture(Path(temporary))
            (rootfs / 'rootfs.tar.gz').write_bytes(source_archive.read_bytes())
            source_archive.write_bytes(b'corrupted external source')
            with patch.object(release.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                    release.validate_ios_rootfs(rootfs, sdk, source, source_archive)
                run.assert_not_called()

    def test_sdk_profile_verification_failure_propagates(self):
        with tempfile.TemporaryDirectory() as temporary:
            rootfs, sdk, source, source_archive = self.rootfs_fixture(Path(temporary))
            error = release.subprocess.CalledProcessError(1, ['sdk-profile-verifier'])
            with patch.object(release.subprocess, 'run', side_effect=error):
                with self.assertRaises(release.subprocess.CalledProcessError):
                    release.validate_ios_rootfs(rootfs, sdk, source, source_archive)

    def test_changed_rootfs_bytes_are_rejected(self):
        for name in ('rootfs.tar.gz', 'resources/alpine-rootfs.zip'):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                rootfs, sdk, source, source_archive = self.rootfs_fixture(Path(temporary))
                path = source_archive if name == 'rootfs.tar.gz' else rootfs / name
                path.write_bytes(b'changed')
                with patch.object(release.subprocess, 'run') as run:
                    with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                        release.validate_ios_rootfs(rootfs, sdk, source, source_archive)
                    run.assert_not_called()

    def test_corrupt_cache_is_rejected_before_network_or_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            digest = hashlib.sha256(b'expected').hexdigest()
            (cache / (digest + '.zip')).write_bytes(b'altered')
            with patch.object(release.urllib.request, 'build_opener', side_effect=AssertionError('network')):
                with self.assertRaisesRegex(ValueError, 'cached release digest mismatch'):
                    release.download({'name': 'sdk.zip', 'sha256': digest}, 'https://example.com', cache, None)

    def test_pinned_cache_needs_no_network(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            payload = b'expected'
            digest = hashlib.sha256(payload).hexdigest()
            expected = cache / (digest + '.zip')
            expected.write_bytes(payload)
            with patch.object(release.urllib.request, 'build_opener', side_effect=AssertionError('network')):
                self.assertEqual(expected, release.download({'name': 'sdk.zip', 'sha256': digest}, 'https://example.com', cache, None))

    def test_failed_promotion_restores_existing_generated_tree(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, output = root / 'source', root / 'output'
            source.mkdir(); output.mkdir()
            (source / 'new').write_text('new')
            (output / 'old').write_text('preserved')
            rename = Path.rename
            def failing_rename(path, destination):
                if path.name == 'new':
                    raise OSError('promotion failed')
                return rename(path, destination)
            with patch.object(Path, 'rename', failing_rename):
                with self.assertRaisesRegex(OSError, 'promotion failed'):
                    release.publish(source, output)
            self.assertEqual('preserved', (output / 'old').read_text())
            self.assertFalse((output / 'new').exists())

    def test_symlinked_output_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'source'; source.mkdir()
            link = root / 'link'; link.symlink_to(source, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'symlinked generated destination'):
                release.publish(source, link)


if __name__ == '__main__':
    unittest.main()
