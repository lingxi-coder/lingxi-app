#!/usr/bin/env python3
"""Record the working-tree sources referenced by handbook pages (not API scope)."""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

REPOSITORIES = ('lingxi-app', 'harness-runtime', 'llm-client', 'mobile-linux-runtime')


def git(repo: Path, *args: str) -> bytes:
    return subprocess.check_output(['git', '-C', str(repo), *args], stderr=subprocess.DEVNULL)


def repository_url(repo: Path) -> str:
    remote = git(repo, 'remote', 'get-url', 'origin').decode().strip()
    match = re.search(r'github\.com[^/:]*[:/]([^/]+/[^/]+?)(?:\.git)?$', remote)
    if not match:
        raise ValueError(f'{repo.name}: GitHub origin required')
    return 'https://github.com/' + match.group(1)


def source_revision(repo: Path, paths: list[str]) -> str:
    head = git(repo, 'rev-parse', 'HEAD').decode().strip()
    if not paths:
        return head
    revision = git(repo, 'log', '-1', '--format=%H', 'HEAD', '--', *paths).decode().strip()
    return revision or head


def revision_published(repo: Path, revision: str) -> bool:
    """Use locally known origin branches without fetching or guessing from HEAD."""
    refs = git(repo, 'for-each-ref', '--format=%(refname)', '--contains=' + revision, 'refs/remotes/origin/').decode().splitlines()
    return any(ref != 'refs/remotes/origin/HEAD' for ref in refs)


def product_pins(workspace: Path) -> dict[str, str]:
    manifest = tomllib.loads((workspace / 'lingxi-app/Cargo.toml').read_text())
    result = {}
    for dependency in manifest['workspace']['dependencies'].values():
        if not isinstance(dependency, dict):
            continue
        for repo in REPOSITORIES[1:]:
            if dependency.get('git', '').removesuffix('.git').endswith('/' + repo):
                revision = dependency.get('rev')
                if not revision or not re.fullmatch(r'[0-9a-f]{40}', revision):
                    raise ValueError(f'{repo}: product pin must be a full SHA')
                if repo in result and result[repo] != revision:
                    raise ValueError(f'{repo}: conflicting product dependency revisions')
                result[repo] = revision
    return result


def build(workspace: Path, pages: list[dict]) -> dict:
    pins = product_pins(workspace)
    metadata = {}
    inputs: dict[str, dict[str, bytes]] = {repo: {} for repo in REPOSITORIES}
    page_sources = {}
    for repo in REPOSITORIES:
        root = workspace / repo
        metadata[repo] = {'repo': repo, 'revision': git(root, 'rev-parse', 'HEAD').decode().strip(), 'sourceUrl': repository_url(root)}
        if repo in pins:
            metadata[repo]['productPinnedRevision'] = pins[repo]
    for page in pages:
        refs = []
        if not page.get('sources'):
            raise ValueError(f'{page["id"]}: missing authoritative sources')
        for source in page['sources']:
            repo, path = source['repo'], source['path']
            if repo not in metadata or Path(path).is_absolute() or '..' in Path(path).parts:
                raise ValueError(f'{page["id"]}: invalid source reference {repo}/{path}')
            root = workspace / repo
            file = root / path
            if not file.resolve().is_relative_to(root.resolve()) or not file.is_file():
                raise ValueError(f'{page["id"]}: source is missing or outside repository: {repo}/{path}')
            if '/docs/technical/' in '/' + path or path in ('apps/web/src/data/docs/baseline.json', 'apps/web/public/api-index.json'):
                raise ValueError(f'{page["id"]}: generated output cannot be a source')
            content = file.read_bytes()
            inputs[repo][path] = content
            try:
                committed = git(root, 'show', 'HEAD:' + path)
            except subprocess.CalledProcessError:
                committed = None
            ref = {**source, 'sourceState': 'committed' if content == committed else 'working-tree'}
            refs.append(ref)
        page_sources[page['id']] = refs
    product_root = workspace / REPOSITORIES[0]
    metadata[REPOSITORIES[0]]['revision'] = source_revision(product_root, sorted(inputs[REPOSITORIES[0]]))
    for repo, record in metadata.items():
        record['published'] = revision_published(workspace / repo, record['revision'])
    for refs in page_sources.values():
        for ref in refs:
            repo = ref['repo']
            revision = metadata[repo]['revision']
            ref['revision'] = revision
            if ref['sourceState'] == 'committed' and not metadata[repo]['published']:
                ref['sourceState'] = 'unpublished'
            if ref['sourceState'] == 'committed':
                from urllib.parse import quote
                ref['url'] = f'{metadata[repo]["sourceUrl"]}/blob/{revision}/{quote(ref["path"], safe="/")}'
    for repo, files in inputs.items():
        digest = hashlib.sha256()
        dirty = False
        for path, content in sorted(files.items()):
            digest.update(path.encode() + b'\0' + hashlib.sha256(content).digest())
            try:
                dirty |= content != git(workspace / repo, 'show', 'HEAD:' + path)
            except subprocess.CalledProcessError:
                dirty = True
        metadata[repo].update(dirty=dirty, treeFingerprint='sha256:' + digest.hexdigest())
    return {'schemaVersion': 1, 'sourceMode': 'working-tree', 'repositories': list(metadata.values()), 'pages': page_sources}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--output', type=Path, default=Path(__file__).resolve().parents[1] / 'src/data/docs/baseline.json')
    args = parser.parse_args()
    value = build(args.workspace.resolve(), json.load(sys.stdin))
    text = json.dumps(value, ensure_ascii=False, indent=2) + '\n'
    if args.check:
        if not args.output.exists() or args.output.read_text() != text:
            raise SystemExit('Handbook source baseline is stale; run npm run docs:sync')
    else:
        args.output.write_text(text)
    print(f'Handbook baseline: {len(value["pages"])} pages, {sum(len(p) for p in value["pages"].values())} source references')


if __name__ == '__main__':
    main()
