#!/usr/bin/env python3
"""Verify codec environment tests reject poisoned-parent regressions in CI."""
from pathlib import Path
import subprocess
import sys

source = Path('packages/remote-control-client/src/isolated_codec.rs')
before = source.read_bytes()
original = before.decode('utf8')
command = [
    'cargo', 'test', '--locked', '-p', 'remote-control-client', '--features',
    'native-media', '--test', 'codec_worker',
    'hostile_parent_environment_is_not_inherited_by_the_codec_worker',
    '--', '--exact', '--test-threads=1', '--nocapture', '--color', 'never',
]


def mutate(replacements):
    value = original
    for old, new in replacements:
        if value.count(old) != 1:
            raise RuntimeError(f'expected exactly one mutation target: {old!r}')
        value = value.replace(old, new, 1)
    source.write_text(value, encoding='utf8')


def expect_failure(label, expected):
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=600)
    print(result.stdout, flush=True)
    if result.returncode == 0 or '1 failed;' not in result.stdout or expected not in result.stdout:
        raise RuntimeError(f'{label}: negative control did not fail for the expected reason')
    print(f'Negative control detected: {label}', flush=True)


try:
    mutate([
        ('command.env(key, private_home);',
         'if let Some(value) = std::env::var_os(key) { command.env(key, value); }'),
        ('command.env(key, private_temp);',
         'if let Some(value) = std::env::var_os(key) { command.env(key, value); }'),
    ])
    expect_failure('workspace HOME/TMP inherited by codec', "HOME is not the worker's private home")
    if sys.platform == 'win32':
        mutate([
            ('command.env("SystemRoot", &system.windows_dir);',
             'command.env("SystemRoot", std::env::var_os("SystemRoot").unwrap());'),
            ('command.env("WINDIR", &system.windows_dir);',
             'command.env("WINDIR", std::env::var_os("WINDIR").unwrap());'),
        ])
        expect_failure('workspace SystemRoot inherited by codec', 'SystemRoot is not the host system root')
finally:
    source.write_bytes(before)
    if source.read_bytes() != before:
        raise RuntimeError('failed to restore candidate source after negative controls')
