#!/usr/bin/env python3
"""Planned tiny host probes; do not run while free disk is below 8 GiB."""
import hashlib
import io
from pathlib import Path, PurePosixPath
import shlex
import shutil
import subprocess
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
CHECKSUM = '7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d'
SOURCE_FILES = {
    'src/lib.rs', 'src/future.rs', 'src/stream.rs', 'src/task/mod.rs', 'src/task/poll.rs',
    'src/task/__internal/mod.rs', 'src/task/__internal/atomic_waker.rs',
}


def run(command, expected=0, contains=None):
    print('$', shlex.join(str(arg) for arg in command), flush=True)
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=30)
    print(result.stdout, end='', flush=True)
    print('exit:', result.returncode, flush=True)
    if result.returncode != expected or (contains and contains not in result.stdout):
        raise RuntimeError(f'unexpected probe outcome, expected exit {expected} and text {contains!r}')


def main():
    for path in (HERE, Path(tempfile.gettempdir())):
        available = shutil.disk_usage(path).free
        if available < 8 * 1024**3:
            raise RuntimeError(f'free space below 8 GiB at {path}: {available} bytes; probes not run')
    archive = (HERE / 'futures-core-0.3.32.crate').read_bytes()
    assert hashlib.sha256(archive).hexdigest() == CHECKSUM
    print('release archive SHA-256:', CHECKSUM, flush=True)
    print('WARNING: diagnostic_clone_panic_may_leave_register_stuck asserts a suspected failure, not correctness.', flush=True)
    run(['rustc', '+1.94.0', '--version'])
    with tempfile.TemporaryDirectory(prefix='futures-core-probe-') as tmp:
        temp = Path(tmp)
        prefix = 'futures-core-0.3.32/'
        with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
            for relative in sorted(SOURCE_FILES):
                assert '..' not in PurePosixPath(relative).parts
                member = tar.getmember(prefix + relative)
                assert member.isfile()
                target = temp / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(tar.extractfile(member).read())
        src = temp / 'src/lib.rs'
        def compile_lib(output, *features):
            command = ['rustc', '+1.94.0', '--edition=2018', '--crate-name', 'futures_core',
                       '--crate-type', 'rlib', '-C', 'debuginfo=0']
            for feature in features:
                command += ['--cfg', f'feature="{feature}"']
            run(command + [src, '-o', output])

        bare = temp / 'libbare.rlib'
        compile_lib(bare)
        compile_lib(temp / 'liballoc.rlib', 'alloc')
        missing = temp / 'missing.rs'
        missing.write_text('use futures_core::future::BoxFuture; fn main() {}\n')
        run(['rustc', '+1.94.0', '--edition=2018', '--extern', f'futures_core={bare}', missing,
             '-o', temp / 'missing'], expected=1, contains='no `BoxFuture` in `future`')
        lib = temp / 'libfutures_core.rlib'
        compile_lib(lib, 'alloc', 'std')
        probe = temp / 'probe'
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_core={lib}', HERE / 'behavior-probe.rs', '-o', probe])
        run([probe, '--nocapture', '--test-threads=1'], contains='6 passed; 0 failed')

        atomic = temp / 'src/task/__internal/atomic_waker.rs'
        source = atomic.read_bytes()
        old = b'fetch_or(WAKING, AcqRel)'
        assert source.count(old) == 1
        atomic.write_bytes(source.replace(old, b'fetch_or(WAITING, AcqRel)', 1))
        compile_lib(lib, 'alloc', 'std')
        mutant = temp / 'mutant-probe'
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_core={lib}', HERE / 'behavior-probe.rs', '-o', mutant])
        run([mutant, '--exact', 'wake_during_register_is_forwarded_after_unlock', '--nocapture'],
            expected=101, contains='wake_during_register_is_forwarded_after_unlock ... FAILED')
    print('Expected diagnostic outcomes observed; a passing panic diagnostic demonstrates the suspected liveness failure.', flush=True)


if __name__ == '__main__':
    main()
