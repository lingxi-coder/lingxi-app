#!/usr/bin/env python3
"""Check exact source exports against real cached dependencies, with a red control."""
import hashlib
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
ARCHIVE = HERE / 'wasm-bindgen-futures-0.4.72.crate'
CHECKSUM = '9473dbd2991ae90b6291c3c32c30c6187ac49aa32f9905d1cce280ec1e110b0f'


def run(command, env, *, expected_error=None):
    print('+', shlex.join(str(x) for x in command), flush=True)
    result = subprocess.run(command, env=env, text=True, capture_output=True)
    print(result.stdout + result.stderr, end='', flush=True)
    if expected_error is None:
        if result.returncode:
            raise RuntimeError(f'command failed: {result.returncode}')
    elif result.returncode == 0 or expected_error not in result.stderr:
        raise RuntimeError(f'negative control did not fail with {expected_error}')
    else:
        print(f'negative control detected: {expected_error}')


def main():
    if hashlib.sha256(ARCHIVE.read_bytes()).hexdigest() != CHECKSUM:
        raise RuntimeError('archive checksum mismatch')
    with tempfile.TemporaryDirectory(prefix='lingxi-wasm-futures-probe-') as temporary:
        tmp = Path(temporary)
        with tarfile.open(ARCHIVE, 'r:gz') as tar:
            tar.extractall(tmp, filter='data')
        package = tmp / 'wasm-bindgen-futures-0.4.72'
        manifest = package / 'Cargo.toml'
        # The released manifest disables auto-discovery, so add only our
        # compile-only example target; the original library source is unchanged.
        manifest.write_text(manifest.read_text() + '\n[[example]]\nname = "api_probe"\npath = "examples/api_probe.rs"\n')
        (package / 'examples').mkdir()
        (package / 'examples/api_probe.rs').write_bytes((HERE / 'api-probe.rs').read_bytes())
        env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_BUILD_JOBS='1')
        common = ['cargo', '+1.94.0', 'check', '--manifest-path', str(manifest),
                  '--target-dir', str(tmp / 'target'), '--locked', '--frozen']
        run([*common, '--lib', '--no-default-features'], env)
        run([*common, '--example', 'api_probe', '--all-features'], env)
        source = package / 'src/lib.rs'
        original = source.read_text()
        old = 'future_to_promise_typed, spawn_local, JsFuture'
        if original.count(old) != 1:
            raise RuntimeError('unexpected source for negative control')
        source.write_text(original.replace(old, 'future_to_promise_typed, spawn_local'))
        run([*common, '--example', 'api_probe', '--all-features'], env,
            expected_error='unresolved import `wasm_bindgen_futures::JsFuture`')
        print('real dependency API type-check and re-export negative control passed')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f'wasm-bindgen-futures probes failed: {error}', file=sys.stderr)
        sys.exit(1)
