#!/usr/bin/env python3
"""Run tiny host-only probes against the exact archive without Cargo builds."""
import hashlib
import io
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
CHECKSUM = 'cecba35d7ad927e23624b22ad55235f2239cfa44fd10428eecbeba6d6a717718'


def run(command, expected=0, contains=None):
    print('$', shlex.join(str(arg) for arg in command), flush=True)
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    print(result.stdout, end='', flush=True)
    print('exit:', result.returncode, flush=True)
    if result.returncode != expected or (contains and contains not in result.stdout):
        raise RuntimeError(f'unexpected probe outcome, expected exit {expected} and text {contains!r}')


def main():
    archive = (HERE / 'futures-io-0.3.32.crate').read_bytes()
    assert hashlib.sha256(archive).hexdigest() == CHECKSUM
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
        source = tar.extractfile('futures-io-0.3.32/src/lib.rs').read()
    print('release archive SHA-256:', CHECKSUM, flush=True)
    print('source SHA-256:', hashlib.sha256(source).hexdigest(), flush=True)
    run(['rustc', '+1.94.0', '--version'])
    with tempfile.TemporaryDirectory(prefix='futures-io-probe-') as tmp:
        temp = Path(tmp)
        src = temp / 'lib.rs'; src.write_bytes(source)
        def compile_lib(path, output, *features):
            command = ['rustc', '+1.94.0', '--edition=2018', '--crate-name', 'futures_io',
                       '--crate-type', 'rlib', '-C', 'debuginfo=0']
            for feature in features:
                command += ['--cfg', f'feature="{feature}"']
            run(command + [path, '-o', output])

        no_std = temp / 'libnostd.rlib'
        compile_lib(src, no_std)
        compile_lib(src, temp / 'libunstable.rlib', 'unstable')
        missing = temp / 'missing.rs'
        missing.write_text('use futures_io::AsyncRead; fn main() {}\n')
        run(['rustc', '+1.94.0', '--edition=2018', '--extern', f'futures_io={no_std}', missing,
             '-o', temp / 'missing'], expected=1, contains='no `AsyncRead` in the root')

        lib = temp / 'libfutures_io.rlib'
        compile_lib(src, lib, 'std')
        compile_lib(src, temp / 'liball.rlib', 'std', 'unstable')
        probe = temp / 'probe'
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_io={lib}', HERE / 'behavior-probe.rs', '-o', probe])
        run([probe, '--nocapture'], contains='5 passed; 0 failed')

        old = b'if !b.is_empty() {'
        assert source.count(old) == 2
        mutant = temp / 'mutant.rs'; mutant.write_bytes(source.replace(old, b'if b.is_empty() {', 1))
        compile_lib(mutant, lib, 'std')
        mutant_probe = temp / 'mutant-probe'
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_io={lib}', HERE / 'behavior-probe.rs', '-o', mutant_probe])
        run([mutant_probe, '--exact', 'default_vectored_read_selects_one_buffer_and_propagates', '--nocapture'],
            expected=101, contains='default_vectored_read_selects_one_buffer_and_propagates ... FAILED')
    print('All expected probe and negative-control outcomes observed.', flush=True)


if __name__ == '__main__':
    main()
