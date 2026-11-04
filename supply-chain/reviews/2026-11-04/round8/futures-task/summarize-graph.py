#!/usr/bin/env python3
"""Summarize frozen metadata and verify selected direct-parent source files."""
import gzip
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tomllib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
SELECTED = {
    ('futures', '0.3.32'): [('Cargo.toml', '[dependencies.futures-task]')],
    ('futures-executor', '0.3.32'): [('src/local_pool.rs', 'let waker = waker_ref(thread_notify);'),
                                    ('src/thread_pool.rs', 'let waker = waker_ref(&wake_handle);')],
    ('futures-util', '0.3.32'): [('src/future/future/shared.rs', 'let waker = waker_ref(&inner.notifier);'),
                                 ('src/task/mod.rs', 'pub use futures_task::{waker_ref, WakerRef};')],
    ('glib', '0.22.10'): [('src/main_context_futures.rs', "Send(FutureObj<'static, Box<dyn Any + Send + 'static>>),")],
}
sha = lambda data: hashlib.sha256(data).hexdigest()


def main():
    raw = gzip.decompress(Path(sys.argv[1]).read_bytes())
    metadata = json.loads(raw)
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
    package = next(p for p in packages.values() if (p['name'], p['version']) == ('futures-task', '0.3.32'))
    parents = []
    for node in nodes.values():
        for edge in node['deps']:
            if edge['pkg'] == package['id']:
                parent = packages[node['id']]
                item = {'id': parent['id'], 'edge': edge}
                key = (parent['name'], parent['version'])
                if key in SELECTED:
                    assert parent['source'] == 'registry+https://github.com/rust-lang/crates.io-index'
                    directory = Path(parent['manifest_path']).parent
                    cache = directory.parent.parent.parent / 'cache' / directory.parent.name / f'{key[0]}-{key[1]}.crate'
                    archive = cache.read_bytes()
                    entry = next(p for p in lock['package'] if (p['name'], p['version']) == key
                                 and p.get('source') == parent['source'])
                    assert sha(archive) == entry['checksum']
                    files = []
                    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
                        for relative, needle in SELECTED[key]:
                            source = (directory / relative).read_bytes()
                            assert tar.extractfile(f'{key[0]}-{key[1]}/{relative}').read() == source
                            lines = [(i, line.strip()) for i, line in enumerate(source.decode().splitlines(), 1) if needle in line]
                            assert len(lines) == 1, (key, lines)
                            files.append({'path': relative, 'sha256': sha(source),
                                          'line': lines[0][0], 'text': lines[0][1]})
                    item.update({'archive_sha256': sha(archive), 'selected_files': files})
                parents.append(item)
    assert { (packages[p['id']]['name'], packages[p['id']]['version']) for p in parents
             if 'selected_files' in p } == set(SELECTED)
    print(json.dumps({'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                      'metadata_command': 'cargo +1.94.0 metadata --format-version 1 --all-features --locked --frozen',
                      'metadata_sha256': sha(raw), 'package_count': len(packages), 'package_id': package['id'],
                      'resolved_features': nodes[package['id']]['features'],
                      'resolved_dependencies': nodes[package['id']]['deps'],
                      'reverse_parents_all_targets': parents}, indent=2))


if __name__ == '__main__':
    main()
