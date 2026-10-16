import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'mobile-linux/prepare-sdk-release.py'
spec = importlib.util.spec_from_file_location('prepare_release', SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleasePreparationTests(unittest.TestCase):
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
