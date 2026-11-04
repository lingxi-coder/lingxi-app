#!/usr/bin/env python3
"""Verify archive, registry, lock, complete Git tree and optional Cargo cache."""
import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import tarfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
NAME = 'futures-io'
VERSION = '0.3.32'
CHECKSUM = 'cecba35d7ad927e23624b22ad55235f2239cfa44fd10428eecbeba6d6a717718'
COMMIT = 'd9bba94c239daa1175a5bb2958f37a5c72db3f6a'
UPSTREAM = 'futures-io/'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git_hash(kind, data):
    return hashlib.sha1(f'{kind} {len(data)}\0'.encode() + data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache', type=Path)
    args = parser.parse_args()
    archive = (HERE / f'{NAME}-{VERSION}.crate').read_bytes()
    assert digest(archive) == CHECKSUM
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    matches = [p for p in lock['package'] if p['name'] == NAME and p['version'] == VERSION]
    assert len(matches) == 1
    assert matches[0]['source'] == 'registry+https://github.com/rust-lang/crates.io-index'
    assert matches[0]['checksum'] == CHECKSUM
    versions = [e for e in map(json.loads, (HERE / 'crates-index.jsonl').read_text().splitlines())
                if e['vers'] == VERSION]
    assert len(versions) == 1
    index = versions[0]
    assert index['name'] == NAME and index['cksum'] == CHECKSUM and index['deps'] == []
    assert index['features'] == {'default': ['std'], 'std': [], 'unstable': []}
    assert index['rust_version'] == '1.36'

    commit = json.loads((HERE / 'upstream-commit.json').read_text())
    raw = (HERE / 'upstream-commit.raw').read_bytes()
    tree = json.loads((HERE / 'upstream-tree.json').read_text())
    assert commit['sha'] == COMMIT and git_hash('commit', raw) == COMMIT
    assert raw.split(b'\n', 1)[0] == f"tree {commit['tree']['sha']}".encode()
    assert tree['sha'] == COMMIT and not tree['truncated']
    paths = {e['path']: e for e in tree['tree']}
    assert len(paths) == len(tree['tree'])
    directories = {'': []}
    for path, item in paths.items():
        pp = PurePosixPath(path)
        parent = str(pp.parent) if str(pp.parent) != '.' else ''
        directories.setdefault(parent, []).append((pp.name, item))
        if item['type'] == 'tree':
            directories.setdefault(path, [])
    for directory in sorted(directories, key=lambda s: (s.count('/'), len(s)), reverse=True):
        entries = sorted(directories[directory],
                         key=lambda x: x[0].encode() + (b'/' if x[1]['type'] == 'tree' else b''))
        data = b''.join(e['mode'].lstrip('0').encode() + b' ' + n.encode() + b'\0' + bytes.fromhex(e['sha'])
                        for n, e in entries)
        expected = paths[directory]['sha'] if directory else commit['tree']['sha']
        assert git_hash('tree', data) == expected, directory

    files = {}
    prefix = f'{NAME}-{VERSION}/'
    generated = {'.cargo_vcs_info.json', 'Cargo.lock', 'Cargo.toml'}
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
        for member in tar:
            assert member.isfile() and member.name.startswith(prefix)
            relative = member.name[len(prefix):]
            assert relative and not PurePosixPath(relative).is_absolute() and '..' not in PurePosixPath(relative).parts
            assert relative not in files
            data = tar.extractfile(member).read()
            if relative == '.cargo_vcs_info.json':
                vcs = json.loads(data)
                assert vcs['git']['sha1'] == COMMIT and vcs['path_in_vcs'] == UPSTREAM.rstrip('/')
            files[relative] = {'size': len(data), 'mode': oct(member.mode), 'sha256': digest(data),
                               'git_blob': git_hash('blob', data)}
            if args.cache:
                assert (args.cache / relative).read_bytes() == data, relative
            if relative not in generated:
                path = UPSTREAM + ('Cargo.toml' if relative == 'Cargo.toml.orig' else relative)
                if relative in ('LICENSE-APACHE', 'LICENSE-MIT'):
                    link = paths[path]
                    assert link['mode'] == '120000'
                    assert link['sha'] == git_hash('blob', f'../{relative}'.encode())
                    path = relative
                entry = paths[path]
                assert entry['type'] == 'blob' and entry['sha'] == git_hash('blob', data), relative
                assert int(entry['mode'], 8) & 0o777 == member.mode & 0o777, relative
    assert set(files) == {'.cargo_vcs_info.json', 'Cargo.lock', 'Cargo.toml', 'Cargo.toml.orig',
                          'LICENSE-APACHE', 'LICENSE-MIT', 'README.md', 'src/lib.rs'}
    if args.cache:
        cached = {p.relative_to(args.cache).as_posix() for p in args.cache.rglob('*') if p.is_file()}
        assert cached == set(files) | {'.cargo-ok'}
    print(json.dumps({'archive_sha256': CHECKSUM, 'lock_sha256': digest((ROOT / 'Cargo.lock').read_bytes()),
                      'index_yanked_at_capture': index['yanked'], 'upstream_commit': COMMIT,
                      'commit_object_verified': True, 'github_signature_status_at_capture': commit['verification']['reason'],
                      'signature_independently_verified': False, 'upstream_root_tree': commit['tree']['sha'],
                      'tree_objects_checked': len(directories), 'upstream_blob_files_checked': 5,
                      'cache_checked': bool(args.cache), 'archive_files': files}, indent=2, sort_keys=True))


if __name__ == '__main__':
    main()
