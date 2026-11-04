#!/usr/bin/env python3
"""Run bounded native probes on the exact archive, with automatic cleanup."""
import hashlib
import io
from pathlib import Path, PurePosixPath
import shlex
import shutil
import subprocess
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
CHECKSUM = '037711b3d59c33004d3856fbdc83b99d4ff37a24768fa1be9ce3538a1cde4393'
SOURCE_FILES = {'src/lib.rs', 'src/arc_wake.rs', 'src/future_obj.rs', 'src/noop_waker.rs',
                'src/spawn.rs', 'src/waker.rs', 'src/waker_ref.rs'}
MIN_FREE = 8 * 1024**3


def require_space(*paths):
    for path in paths:
        free = shutil.disk_usage(path).free
        print(f'free bytes at {path}: {free}', flush=True)
        if free <= MIN_FREE:
            raise RuntimeError(f'free space at or below 8 GiB: {free}; no further compilation')


def run(command, expected=0, contains=None):
    print('$', shlex.join(str(x) for x in command), flush=True)
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=45)
    print(result.stdout, end='', flush=True)
    print('exit:', result.returncode, flush=True)
    if result.returncode != expected or (contains is not None and contains not in result.stdout):
        raise RuntimeError(f'expected exit {expected} and text {contains!r}')


def main():
    require_space(HERE, Path(tempfile.gettempdir()))
    archive = (HERE / 'futures-task-0.3.32.crate').read_bytes()
    assert hashlib.sha256(archive).hexdigest() == CHECKSUM
    print('release archive SHA-256:', CHECKSUM, flush=True)
    run(['rustc', '+1.94.0', '--version', '--verbose'])
    with tempfile.TemporaryDirectory(prefix='futures-task-probe-') as tmp:
        temp = Path(tmp)
        prefix = 'futures-task-0.3.32/'
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
            require_space(HERE, temp)
            command = ['rustc', '+1.94.0', '--edition=2018', '--crate-name', 'futures_task',
                       '--crate-type', 'rlib', '-C', 'debuginfo=0']
            for feature in features:
                command += ['--cfg', f'feature="{feature}"']
            run(command + [src, '-o', output])
            print(f'compiled artifact {output.name}: {output.stat().st_size} bytes', flush=True)

        bare = temp / 'libbare.rlib'
        compile_lib(bare)
        compile_lib(temp / 'liballoc.rlib', 'alloc')
        lib = temp / 'libfutures_task.rlib'
        compile_lib(lib, 'alloc', 'std')
        compile_lib(temp / 'libinert.rlib', 'alloc', 'std', 'unstable', 'cfg-target-has-atomic')

        negatives = [
            ('no_alloc', bare, 'use futures_task::waker; fn main() {}\n', 'no `waker` in the root'),
            ('non_send_future', lib, 'use futures_task::FutureObj; use std::rc::Rc; fn main() { let rc=Rc::new(1); let f=async move { drop(rc); }; let _ = FutureObj::new(Box::new(f)); }\n', 'cannot be sent between threads safely'),
            ('borrowed_waker_escape', lib, 'use futures_task::{ArcWake,waker_ref}; use std::sync::Arc; struct N; impl ArcWake for N { fn wake_by_ref(_: &Arc<Self>) {} } fn main() { let w; { let n=Arc::new(N); w=waker_ref(&n); } w.wake_by_ref(); }\n', 'does not live long enough'),
            ('non_static_arcwake', lib, 'use futures_task::{ArcWake,waker_ref}; use std::sync::Arc; struct N<\'a>(&\'a u8); impl ArcWake for N<\'_> { fn wake_by_ref(_: &Arc<Self>) {} } fn main() { let v=1; let n=Arc::new(N(&v)); let w=waker_ref(&n); w.wake_by_ref(); }\n', 'does not live long enough'),
            ('local_future_send', lib, 'use futures_task::LocalFutureObj; fn main() { let o=LocalFutureObj::new(Box::new(async {})); std::thread::spawn(move || drop(o)); }\n', 'cannot be sent between threads safely'),
        ]
        for name, library, text, message in negatives:
            file = temp / f'{name}.rs'
            file.write_text(text)
            require_space(HERE, temp)
            run(['rustc', '+1.94.0', '--edition=2018', '--extern', f'futures_task={library}', file,
                 '-o', temp / name], expected=1, contains=message)

        probe = temp / 'probe'
        require_space(HERE, temp)
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_task={lib}', HERE / 'behavior-probe.rs', '-o', probe])
        print(f'compiled artifact {probe.name}: {probe.stat().st_size} bytes', flush=True)
        run([probe, '--nocapture', '--test-threads=1'], contains='8 passed; 0 failed')

        # A behavioral negative control, not a memory-safety test: only the
        # disposable extracted source is changed, to suppress the default wake.
        source_file = temp / 'src/arc_wake.rs'
        source = source_file.read_bytes()
        old = b'Self::wake_by_ref(&self)'
        assert source.count(old) == 1
        source_file.write_bytes(source.replace(old, b'{}', 1))
        compile_lib(lib, 'alloc', 'std')
        mutant = temp / 'mutant-probe'
        require_space(HERE, temp)
        run(['rustc', '+1.94.0', '--edition=2018', '--test', '-C', 'debuginfo=0',
             '--extern', f'futures_task={lib}', HERE / 'behavior-probe.rs', '-o', mutant])
        print(f'compiled artifact {mutant.name}: {mutant.stat().st_size} bytes', flush=True)
        run([mutant, '--exact', 'owned_waker_counts_and_consuming_wake', '--nocapture'],
            expected=101, contains='owned_waker_counts_and_consuming_wake ... FAILED')
        files = [p for p in temp.rglob('*') if p.is_file()]
        print(f'temporary files: {len(files)}, total {sum(p.stat().st_size for p in files)} bytes', flush=True)
    print(f'temporary directory removed: {not Path(tmp).exists()}', flush=True)
    assert not Path(tmp).exists()
    print('Expected outcomes observed; host tests do not prove unsafe-code soundness.', flush=True)


if __name__ == '__main__':
    main()
