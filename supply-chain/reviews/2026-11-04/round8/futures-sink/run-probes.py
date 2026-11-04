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
CHECKSUM = 'c39754e157331b013978ec91992bde1ac089843443c49cbc7f46150b0fad0893'


def run(command, expected=0, contains=None):
    print('$', shlex.join(str(arg) for arg in command), flush=True)
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    print(result.stdout, end='', flush=True)
    print('exit:', result.returncode, flush=True)
    if result.returncode != expected or (contains and contains not in result.stdout):
        raise RuntimeError(f'unexpected probe outcome, expected exit {expected} and text {contains!r}')


def main():
    archive = (HERE / 'futures-sink-0.3.32.crate').read_bytes()
    assert hashlib.sha256(archive).hexdigest() == CHECKSUM
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
        source = tar.extractfile('futures-sink-0.3.32/src/lib.rs').read()
    print('release archive SHA-256:', CHECKSUM, flush=True)
    print('source SHA-256:', hashlib.sha256(source).hexdigest(), flush=True)
    run(['rustc', '+1.94.0', '--version'])
    with tempfile.TemporaryDirectory(prefix='futures-sink-probe-') as tmp:
        temp = Path(tmp)
        src = temp / 'lib.rs'; src.write_bytes(source)
        def compile_lib(path, output, *features):
            command = ['rustc', '+1.94.0', '--edition=2018', '--crate-name', 'futures_sink',
                       '--crate-type', 'rlib', '-C', 'debuginfo=0']
            for feature in features:
                command += ['--cfg', f'feature="{feature}"']
            run(command + [path, '-o', output])

        no_std = temp / 'libnostd.rlib'
        compile_lib(src, no_std)
        compile_lib(src, temp / 'liballoc.rlib', 'alloc')
        missing = temp / 'missing.rs'
        missing.write_text('use futures_sink::Sink; fn check<T: Sink<u8>>() {} fn main() { check::<Vec<u8>>(); }\n')
        run(['rustc', '+1.94.0', '--edition=2018', '--extern', f'futures_sink={no_std}', missing,
             '-o', temp / 'missing'], expected=1, contains='the trait bound `Vec<u8>: futures_sink::Sink<u8>` is not satisfied')

        lib = temp / 'libfutures_sink.rlib'
        compile_lib(src, lib, 'alloc', 'std')
        probe = temp / 'probe'
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_sink={lib}', HERE / 'behavior-probe.rs', '-o', probe])
        run([probe, '--nocapture'], contains='4 passed; 0 failed')

        old = b'.push_back(item)'
        assert source.count(old) == 1
        mutant = temp / 'mutant.rs'; mutant.write_bytes(source.replace(old, b'.push_front(item)', 1))
        compile_lib(mutant, lib, 'alloc', 'std')
        mutant_probe = temp / 'mutant-probe'
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_sink={lib}', HERE / 'behavior-probe.rs', '-o', mutant_probe])
        run([mutant_probe, '--exact', 'in_memory_sinks_preserve_order_and_close_does_not_seal', '--nocapture'],
            expected=101, contains='in_memory_sinks_preserve_order_and_close_does_not_seal ... FAILED')
    print('All expected probe and negative-control outcomes observed.', flush=True)


if __name__ == '__main__':
    main()
