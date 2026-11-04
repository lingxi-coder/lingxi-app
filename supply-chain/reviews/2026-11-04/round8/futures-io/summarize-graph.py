#!/usr/bin/env python3
"""Summarize frozen metadata and verify selected direct-parent source files."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import tarfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
SELECTED = {
    ('async-compat', '0.2.5'): ('src/lib.rs', 'impl<T: tokio::io::AsyncRead> futures_io::AsyncRead'),
    ('async-io', '2.6.0'): ('src/lib.rs', 'use futures_io::{AsyncRead, AsyncWrite}'),
    ('async-signal', '0.2.14'): ('src/pipe.rs', 'use futures_io::AsyncRead'),
    ('blocking', '1.6.2'): ('src/lib.rs', 'use futures_io::{AsyncRead, AsyncSeek, AsyncWrite}'),
    ('futures', '0.3.32'): ('src/lib.rs', 'pub use futures_io::{AsyncBufRead, AsyncRead, AsyncSeek, AsyncWrite}'),
    ('futures-lite', '2.6.1'): ('src/io.rs', 'pub use futures_io::{AsyncBufRead, AsyncRead, AsyncSeek, AsyncWrite}'),
    ('futures-util', '0.3.32'): ('src/io/mod.rs', 'pub use futures_io::{AsyncBufRead, AsyncRead, AsyncSeek, AsyncWrite}'),
    ('piper', '0.2.5'): ('src/lib.rs', 'use futures_io::{AsyncBufRead, AsyncRead, AsyncWrite}'),
}
sha = lambda data: hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('metadata', type=Path, help='gzip-compressed cargo metadata JSON')
    args = parser.parse_args()
    raw = gzip.decompress(args.metadata.read_bytes())
    metadata = json.loads(raw)
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    pkg = next(p for p in packages.values() if (p['name'], p['version']) == ('futures-io', '0.3.32'))
    node = nodes[pkg['id']]
    parents = []
    for candidate in nodes.values():
        for edge in candidate['deps']:
            if edge['pkg'] == pkg['id']:
                parent = packages[candidate['id']]
                key = (parent['name'], parent['version'])
                assert key in SELECTED and parent['source'] == 'registry+https://github.com/rust-lang/crates.io-index'
                relative, needle = SELECTED[key]
                package_dir = Path(parent['manifest_path']).parent
                file = (package_dir / relative).read_bytes()
                cache = package_dir.parent.parent.parent / 'cache' / package_dir.parent.name / f'{key[0]}-{key[1]}.crate'
                archive = cache.read_bytes()
                entry = next(p for p in lock['package'] if (p['name'], p['version']) == key
                             and p.get('source') == parent['source'])
                assert sha(archive) == entry['checksum']
                with tarfile.open(fileobj=__import__('io').BytesIO(archive), mode='r:gz') as tar:
                    assert tar.extractfile(f'{key[0]}-{key[1]}/{relative}').read() == file
                lines = file.decode().splitlines()
                found = [(i, line.strip()) for i, line in enumerate(lines, 1) if needle in line]
                assert len(found) == 1, (key, found)
                parents.append({'id': parent['id'], 'edge': edge, 'archive_sha256': sha(archive),
                                'selected_file': relative, 'selected_file_sha256': sha(file),
                                'selected_line': found[0][0], 'selected_text': found[0][1]})
    assert {(packages[p['id']]['name'], packages[p['id']]['version']) for p in parents} == set(SELECTED)
    print(json.dumps({'head': __import__('subprocess').check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                      'metadata_command': 'cargo +1.94.0 metadata --format-version 1 --all-features --locked --frozen',
                      'metadata_sha256': sha(raw), 'package_count': len(packages), 'package_id': pkg['id'],
                      'resolved_features': node['features'], 'resolved_dependencies': node['deps'],
                      'reverse_parents_all_targets': parents}, indent=2))


if __name__ == '__main__':
    main()
