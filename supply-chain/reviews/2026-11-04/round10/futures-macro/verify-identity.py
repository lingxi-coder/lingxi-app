#!/usr/bin/env python3
"""Verify this package against lock, registry, cache and the exact upstream Git tree."""
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import tarfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
LOCATOR = HERE.parents[1] / 'round8/futures-core'
CACHE = Path.home() / '.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/futures-macro-0.3.32'
CACHE_ARCHIVE = Path.home() / '.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f/futures-macro-0.3.32.crate'
NAME, VERSION = 'futures-macro', '0.3.32'
CHECKSUM = 'e835b70203e41293343137df5c0664546da5745f82ec9b84d40be8336958447b'
COMMIT = 'd9bba94c239daa1175a5bb2958f37a5c72db3f6a'
EXPECTED = {'.cargo_vcs_info.json', 'Cargo.lock', 'Cargo.toml', 'Cargo.toml.orig',
            'LICENSE-APACHE', 'LICENSE-MIT', 'src/executor.rs', 'src/join.rs',
            'src/lib.rs', 'src/select.rs', 'src/stream_select.rs'}
GENERATED = {'.cargo_vcs_info.json', 'Cargo.lock', 'Cargo.toml'}

def sha256(data):
    return hashlib.sha256(data).hexdigest()

def git_hash(kind, data):
    return hashlib.sha1(f'{kind} {len(data)}\0'.encode() + data).hexdigest()

def main():
    archive = (HERE / f'{NAME}-{VERSION}.crate').read_bytes()
    assert sha256(archive) == CHECKSUM == sha256(CACHE_ARCHIVE.read_bytes())
    lock_bytes = (ROOT / 'Cargo.lock').read_bytes()
    lock = tomllib.loads(lock_bytes.decode())
    packages = [p for p in lock['package'] if p['name'] == NAME and p['version'] == VERSION]
    assert len(packages) == 1
    assert packages[0]['source'] == 'registry+https://github.com/rust-lang/crates.io-index'
    assert packages[0]['checksum'] == CHECKSUM
    index_bytes = (HERE / 'crates-index.jsonl').read_bytes()
    index_entries = [json.loads(s) for s in index_bytes.splitlines()]
    matches = [e for e in index_entries if e['name'] == NAME and e['vers'] == VERSION]
    assert len(matches) == 1
    index = matches[0]
    assert index['cksum'] == CHECKSUM and index['features'] == {}
    assert index['rust_version'] == '1.71'
    assert [(d['name'], d['req'], d['features'], d['optional'], d['kind']) for d in index['deps']] == [
        ('proc-macro2', '^1.0.60', [], False, 'normal'),
        ('quote', '^1.0', [], False, 'normal'),
        ('syn', '^2.0.52', ['full'], False, 'normal')]

    # These raw locator files came from the earlier futures-core review; independently
    # recompute the commit and each Git tree, then this package's own blob identities.
    commit = json.loads((LOCATOR / 'upstream-commit.json').read_bytes())
    raw = (LOCATOR / 'upstream-commit.raw').read_bytes()
    tree = json.loads((LOCATOR / 'upstream-tree.json').read_bytes())
    assert commit['sha'] == COMMIT == git_hash('commit', raw)
    assert raw.split(b'\n', 1)[0] == f"tree {commit['tree']['sha']}".encode()
    assert tree['sha'] == COMMIT and tree['truncated'] is False
    paths = {e['path']: e for e in tree['tree']}
    assert len(paths) == len(tree['tree'])
    directories = {'': []}
    for path, item in paths.items():
        p = PurePosixPath(path)
        parent = str(p.parent) if str(p.parent) != '.' else ''
        directories.setdefault(parent, []).append((p.name, item))
        if item['type'] == 'tree':
            directories.setdefault(path, [])
    for directory in sorted(directories, key=lambda s: (s.count('/'), len(s)), reverse=True):
        entries = sorted(directories[directory], key=lambda p: p[0].encode() + (b'/' if p[1]['type'] == 'tree' else b''))
        data = b''.join(e['mode'].lstrip('0').encode() + b' ' + name.encode() + b'\0' + bytes.fromhex(e['sha']) for name, e in entries)
        expected = paths[directory]['sha'] if directory else commit['tree']['sha']
        assert git_hash('tree', data) == expected, directory

    files, data_by_path = {}, {}
    prefix = f'{NAME}-{VERSION}/'
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
        for member in tar:
            assert member.isfile() and member.name.startswith(prefix)
            relative = member.name[len(prefix):]
            assert relative and not PurePosixPath(relative).is_absolute() and '..' not in PurePosixPath(relative).parts
            assert relative not in files
            data = tar.extractfile(member).read()
            assert len(data) == member.size
            assert (CACHE / relative).read_bytes() == data, relative
            data_by_path[relative] = data
            item = {'bytes': len(data), 'mode': oct(member.mode), 'sha256': sha256(data), 'git_blob_sha1': git_hash('blob', data)}
            if relative in GENERATED:
                item['upstream'] = 'Cargo-generated; not asserted to be an upstream blob'
            else:
                upstream = 'futures-macro/' + ('Cargo.toml' if relative == 'Cargo.toml.orig' else relative)
                if relative in ('LICENSE-APACHE', 'LICENSE-MIT'):
                    link = paths[upstream]
                    assert link['mode'] == '120000' and link['sha'] == git_hash('blob', f'../{relative}'.encode())
                    item['upstream_symlink'] = upstream + ' -> ../' + relative
                    upstream = relative
                entry = paths[upstream]
                assert entry['type'] == 'blob' and entry['sha'] == item['git_blob_sha1'], relative
                assert int(entry['mode'], 8) & 0o777 == member.mode & 0o777, relative
                item['upstream_blob'] = upstream
            files[relative] = item
    assert set(files) == EXPECTED
    assert {p.relative_to(CACHE).as_posix() for p in CACHE.rglob('*') if p.is_file()} == EXPECTED | {'.cargo-ok'}
    vcs = json.loads(data_by_path['.cargo_vcs_info.json'])
    assert vcs == {'git': {'sha1': COMMIT}, 'path_in_vcs': NAME}
    manifest = tomllib.loads(data_by_path['Cargo.toml'].decode())
    assert manifest['package']['name'] == NAME and manifest['package']['version'] == VERSION
    assert manifest['package']['build'] is False
    assert all(manifest['package'][x] is False for x in ('autolib','autobins','autoexamples','autotests','autobenches'))
    assert manifest['lib'] == {'name':'futures_macro','path':'src/lib.rs','proc-macro':True}
    assert manifest['features'] == {} and 'build-dependencies' not in manifest
    assert set(manifest['dependencies']) == {'proc-macro2','quote','syn'}
    print(json.dumps({'archive_sha256':sha256(archive), 'archive_source':'https://static.crates.io/crates/futures-macro/futures-macro-0.3.32.crate',
      'app_lock_sha256':sha256(lock_bytes),'app_lock_package':packages[0],
      'index_sha256':sha256(index_bytes),'index_source':'https://index.crates.io/fu/tu/futures-macro','index_entry':index,
      'cache_archive_and_each_extracted_file_equal':True,'cache_only_extra':'.cargo-ok',
      'upstream_locator':'../../round8/futures-core/upstream-{commit.json,commit.raw,tree.json}',
      'upstream_commit':COMMIT,'commit_object_verified':True,'github_signature_status_at_locator_capture':commit['verification']['reason'],
      'publisher_authentication_independently_verified':False,'upstream_root_tree':commit['tree']['sha'],
      'tree_objects_recomputed':len(directories),'own_upstream_blob_files_checked':len(EXPECTED-GENERATED),
      'normalized_manifest_no_build_script':True,'normalized_manifest_library_proc_macro':True,
      'archive_files':dict(sorted(files.items()))},indent=2,sort_keys=True))

if __name__ == '__main__':
    main()
