#!/usr/bin/env python3
"""Exercise optional portable-atomic in a disposable, offline host target."""
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
CORE_SHA = '7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d'
PORTABLE_SHA = '05c8b63e8d9609db387f0324918f81d68fe27748f084ef092fb35954d0539a85'
PORTABLE_CACHE = Path.home() / '.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/portable-atomic-1.15.0.crate'
PORTABLE_SRC = Path.home() / '.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/portable-atomic-1.15.0'
sha = lambda data: hashlib.sha256(data).hexdigest()


def check_free():
    for path in (HERE, Path(tempfile.gettempdir())):
        free = shutil.disk_usage(path).free
        print(f'free bytes at {path}: {free}', flush=True)
        if free < 8 * 1024**3:
            raise RuntimeError('free space below 8 GiB; no further compilation')


def run(command, cwd, env, expected=0, contains=None):
    print('$', shlex.join(str(arg) for arg in command), flush=True)
    result = subprocess.run(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, timeout=60)
    print(result.stdout, end='', flush=True)
    print('exit:', result.returncode, flush=True)
    if result.returncode != expected or (contains and contains not in result.stdout):
        raise RuntimeError(f'unexpected outcome: expected exit {expected}, text {contains!r}')


def main():
    check_free()
    core = (HERE / 'futures-core-0.3.32.crate').read_bytes()
    portable = PORTABLE_CACHE.read_bytes()
    assert sha(core) == CORE_SHA and sha(portable) == PORTABLE_SHA
    app_lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    app_package = next(p for p in app_lock['package'] if (p['name'], p['version']) == ('portable-atomic', '1.15.0'))
    assert app_package['checksum'] == PORTABLE_SHA
    verified = []
    with tarfile.open(fileobj=io.BytesIO(portable), mode='r:gz') as archive:
        for member in archive:
            prefix = 'portable-atomic-1.15.0/'
            assert member.name.startswith(prefix)
            relative = member.name[len(prefix):]
            if member.isfile() and (relative in ('Cargo.toml', 'build.rs') or relative.startswith('src/')):
                assert '..' not in PurePosixPath(relative).parts
                data = archive.extractfile(member).read()
                assert (PORTABLE_SRC / relative).read_bytes() == data, relative
                verified.append(relative)
    assert 'Cargo.toml' in verified and 'build.rs' in verified and any(p.startswith('src/') for p in verified)
    cached_src = {p.relative_to(PORTABLE_SRC).as_posix() for p in (PORTABLE_SRC / 'src').rglob('*') if p.is_file()}
    assert cached_src == {p for p in verified if p.startswith('src/')}
    print('futures-core archive SHA-256:', CORE_SHA, flush=True)
    print('portable-atomic 1.15.0 archive SHA-256:', PORTABLE_SHA, flush=True)
    print(f'portable-atomic cached build-source files matched: {len(verified)}', flush=True)
    temp_path = None
    try:
        with tempfile.TemporaryDirectory(prefix='futures-core-portable-') as tmp:
            temp_path = Path(tmp)
            core_dir = temp_path / 'core'; project = temp_path / 'probe'
            project.mkdir(); (project / 'tests').mkdir(); (project / 'src').mkdir()
            with tarfile.open(fileobj=io.BytesIO(core), mode='r:gz') as archive:
                prefix = 'futures-core-0.3.32/'
                for member in archive:
                    assert member.isfile() and member.name.startswith(prefix)
                    relative = member.name[len(prefix):]
                    assert relative and '..' not in PurePosixPath(relative).parts
                    target = core_dir / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(archive.extractfile(member).read())
            (project / 'Cargo.toml').write_text('''[package]
name = "futures-core-portable-probe"
version = "0.0.0"
edition = "2021"

[dependencies]
futures-core = { path = "../core", features = ["portable-atomic"] }
portable-atomic = { version = "=1.15.0", default-features = false, features = ["require-cas"] }
''')
            (project / 'src/lib.rs').write_text('// Isolated probe harness; no application code.\n')
            (project / 'tests/behavior_probe.rs').write_bytes((HERE / 'behavior-probe.rs').read_bytes())
            env = dict(os.environ, CARGO_TARGET_DIR=str(temp_path / 'target'), CARGO_INCREMENTAL='0',
                       CARGO_BUILD_JOBS='1', CARGO_PROFILE_DEV_DEBUG='0', CARGO_PROFILE_TEST_DEBUG='0',
                       CARGO_NET_OFFLINE='true')
            run(['cargo', '+1.94.0', '--version'], project, env)
            run(['cargo', '+1.94.0', 'generate-lockfile', '--offline'], project, env)
            lock = tomllib.loads((project / 'Cargo.lock').read_text())
            locked = next(p for p in lock['package'] if (p['name'], p['version']) == ('portable-atomic', '1.15.0'))
            assert locked['checksum'] == PORTABLE_SHA
            command = ['cargo', '+1.94.0', 'metadata', '--offline', '--locked', '--format-version', '1']
            metadata = json.loads(subprocess.check_output(command, cwd=project, env=env, timeout=60))
            target = next(p for p in metadata['packages'] if (p['name'], p['version']) == ('futures-core', '0.3.32'))
            node = next(n for n in metadata['resolve']['nodes'] if n['id'] == target['id'])
            assert 'portable-atomic' in node['features']
            assert any('portable-atomic@1.15.0' in edge['pkg'] for edge in node['deps'])
            print('temporary Cargo.lock SHA-256:', sha((project / 'Cargo.lock').read_bytes()), flush=True)
            print('resolved futures-core features:', json.dumps(node['features']), flush=True)
            print('resolved futures-core dependency edges:', json.dumps(node['deps']), flush=True)
            print('WARNING: panic diagnostics assert observed liveness failure, not correctness.', flush=True)
            check_free()
            run(['cargo', '+1.94.0', 'test', '--offline', '--locked', '--test', 'behavior_probe', '--',
                 '--nocapture', '--test-threads=1'], project, env, contains='7 passed; 0 failed')
            atomic = core_dir / 'src/task/__internal/atomic_waker.rs'
            source = atomic.read_bytes(); old = b'fetch_or(WAKING, AcqRel)'
            assert source.count(old) == 1
            atomic.write_bytes(source.replace(old, b'fetch_or(WAITING, AcqRel)', 1))
            check_free()
            run(['cargo', '+1.94.0', 'test', '--offline', '--locked', '--test', 'behavior_probe',
                 'wake_during_register_is_forwarded_after_unlock', '--', '--exact', '--nocapture'],
                project, env, expected=101, contains='wake_during_register_is_forwarded_after_unlock ... FAILED')
            files = [p for p in (temp_path / 'target').rglob('*') if p.is_file()]
            print(f'isolated target files: {len(files)}, total {sum(p.stat().st_size for p in files)} bytes', flush=True)
            all_files = [p for p in temp_path.rglob('*') if p.is_file()]
            print(f'all temporary files: {len(all_files)}, total {sum(p.stat().st_size for p in all_files)} bytes', flush=True)
    finally:
        if temp_path is not None:
            print(f'temporary directory removed: {not temp_path.exists()}', flush=True)
    assert temp_path is not None and not temp_path.exists()


if __name__ == '__main__':
    main()
