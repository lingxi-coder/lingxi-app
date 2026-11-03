#!/usr/bin/env python3
"""Reject unsafe native-support output overrides before resolving the SDK."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

script = Path(__file__).resolve().with_name('build-linux-runtime.sh')
with tempfile.TemporaryDirectory(prefix='lingxi-ios-native-stage-') as temporary:
    root = Path(temporary)
    existing = root / 'existing'
    existing.mkdir()
    sentinel = existing / 'preserve'
    sentinel.write_text('keep')
    unused = root / 'new-stage'
    cases = [
        (str(existing), str(root / 'cache'), 'new absolute path'),
        ('relative-output', str(root / 'cache'), 'new absolute path'),
        (str(unused), str(unused / 'cache'), 'cache cannot be inside'),
        (str(unused) + '/', str(unused / 'cache'), 'cache cannot be inside'),
        (str(unused), str(root / 'other/../new-stage/cache'), 'cache cannot be inside'),
    ]
    for output, cache, message in cases:
        environment = dict(os.environ, LINGXI_IOS_NATIVE_SUPPORT_OUTPUT=output,
                           LINGXI_IOS_NATIVE_SUPPORT_CACHE=cache)
        result = subprocess.run(['bash', str(script), '--simulator-only'],
                                env=environment, capture_output=True, text=True)
        assert result.returncode != 0 and message in result.stderr
        assert sentinel.read_text() == 'keep'
        assert not unused.exists()

    # Test a cache-only override against the default output in a disposable
    # checkout.  The existing files must survive before SDK discovery begins.
    isolated_script = root / 'repo/apps/ios/native/scripts/build-linux-runtime.sh'
    isolated_script.parent.mkdir(parents=True)
    shutil.copyfile(script, isolated_script)
    default_output = isolated_script.parent.parent / 'build/mobile-linux-sdk'
    cache = default_output / 'custom-cache'
    cache.mkdir(parents=True)
    cache_sentinel = cache / 'preserve'
    cache_sentinel.write_text('keep')
    environment = dict(os.environ, LINGXI_IOS_NATIVE_SUPPORT_CACHE=str(cache))
    environment.pop('LINGXI_IOS_NATIVE_SUPPORT_OUTPUT', None)
    result = subprocess.run(['bash', str(isolated_script), '--simulator-only'],
                            env=environment, capture_output=True, text=True)
    assert result.returncode != 0 and 'cache cannot be inside' in result.stderr
    assert cache_sentinel.read_text() == 'keep'

print('Native support staging rejects existing, relative and overlapping output paths, including cache-only overrides.')
