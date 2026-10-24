import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { copySource } from '../src/main/local-apps/store.ts';
import { LocalAppStore } from './fixtures/local-app-store.ts';

async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-local-app-store-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  return { root, store: new LocalAppStore(join(root, 'library')) };
}

test('an incomplete first publish does not hide other apps, while damaged metadata identifies the app', async t => {
  const { store } = await fixture(t);
  const row = await store.publish({ name: 'Existing', html: '<!doctype html>' });
  const incomplete = 'a11111111111111111111111111111111';
  await fs.mkdir(store.directory(incomplete));
  assert.deepEqual((await store.list()).map(app => app.id), [row.id]);
  await fs.writeFile(join(store.directory(incomplete), 'app.json'), '{broken');
  await assert.rejects(store.list(), new RegExp(`Cannot load application ${incomplete}`));
});

test('publication advances past a snapshot left by a crash before the metadata commit', async t => {
  const { store } = await fixture(t);
  const first = await store.publish({ name: 'Recoverable', html: '<!doctype html><h1>original</h1>' });
  const orphan = store.revisionPath(first.id, 2);
  await fs.mkdir(orphan);
  await fs.writeFile(join(orphan, 'index.html'), 'uncommitted snapshot');
  const updated = await store.publish({ id: first.id, html: '<!doctype html><h1>updated</h1>' });
  assert.equal(updated.revision, 3);
  assert.equal((await store.get(first.id)).revision, 3);
  assert.equal(await fs.readFile(join(orphan, 'index.html'), 'utf8'), 'uncommitted snapshot');
  assert.match(await fs.readFile(join(store.revisionPath(first.id, 1), 'index.html'), 'utf8'), /original/);
});

test('published executable scripts still run after snapshot and workspace creation', { skip: process.platform === 'win32' }, async t => {
  const { store } = await fixture(t);
  const source = join(store.root, 'imports', 'scripts');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Scripts', entry: { type: 'static', path: 'index.html' }, install: [{ command: './install.sh' }] }));
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'install.sh'), '#!/bin/sh\nprintf ready\n', { mode: 0o755 });
  const row = await store.publish({ staged_path: source });
  const workspace = await store.prepareWorkspace(row);
  assert.equal((await promisify(execFile)(join(workspace, 'install.sh'))).stdout, 'ready');
  assert.equal((await fs.stat(join(workspace, 'install.sh'))).mode & 0o777, 0o700);
});

test('a copied preparation marker cannot skip installation in a fresh workspace', async t => {
  const { store } = await fixture(t);
  const source = join(store.root, 'imports', 'prepared');
  await fs.mkdir(source, { recursive: true });
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Prepared source', entry: { type: 'static', path: 'index.html' } }));
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, '.lingxi-prepared.json'), '{"revision":1}');
  const row = await store.publish({ staged_path: source });
  await assert.rejects(fs.access(join(store.revisionPath(row.id, 1), '.lingxi-prepared.json')));
  // Also defend workspace preparation against an older source snapshot.
  await fs.writeFile(join(store.revisionPath(row.id, 1), '.lingxi-prepared.json'), '{"revision":1}');
  const workspace = await store.prepareWorkspace(row);
  await assert.rejects(fs.access(join(workspace, '.lingxi-prepared.json')));
});

test('custom Python environments are excluded using a regular marker without skipping the project root', { skip: process.platform === 'win32' }, async t => {
  const { store } = await fixture(t);
  const source = join(store.root, 'imports', 'custom-python');
  await fs.mkdir(join(source, '.python-env', 'bin'), { recursive: true });
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Python', entry: { type: 'static', path: 'index.html' }, install: [{ command: 'python -V', venv: '.python-env' }] }));
  await fs.writeFile(join(source, 'index.html'), '<!doctype html>');
  await fs.writeFile(join(source, 'pyvenv.cfg'), 'project-owned root file');
  await fs.writeFile(join(source, '.python-env', 'pyvenv.cfg'), 'home = /python');
  await fs.symlink(join(source, 'index.html'), join(source, '.python-env', 'bin', 'python'));
  const row = await store.publish({ staged_path: source });
  await assert.rejects(fs.access(join(store.revisionPath(row.id, 1), '.python-env')));
  assert.equal(await fs.readFile(join(store.revisionPath(row.id, 1), 'pyvenv.cfg'), 'utf8'), 'project-owned root file');
  await fs.unlink(join(source, '.python-env', 'pyvenv.cfg'));
  await fs.symlink(join(source, 'pyvenv.cfg'), join(source, '.python-env', 'pyvenv.cfg'));
  await assert.rejects(store.publish({ staged_path: source }), /symlink/);
});

test('replacing an enumerated source directory with a symlink aborts the snapshot', { skip: process.platform === 'win32' }, async t => {
  const { root } = await fixture(t);
  const source = join(root, 'source'), outside = join(root, 'outside'), destination = join(root, 'snapshot');
  await fs.mkdir(join(source, 'folder'), { recursive: true });
  await fs.mkdir(outside);
  await fs.writeFile(join(outside, 'private.txt'), 'not authorized source');
  const readdir = fs.readdir;
  let swapped = false;
  fs.readdir = (async (...args: Parameters<typeof fs.readdir>) => {
    const entries = await readdir(...args);
    if (String(args[0]) === source && !swapped) {
      swapped = true;
      await fs.rename(join(source, 'folder'), join(source, 'original-folder'));
      await fs.symlink(outside, join(source, 'folder'));
    }
    return entries;
  }) as typeof fs.readdir;
  try {
    await assert.rejects(copySource(source, destination), /symlink|changed/);
    await assert.rejects(fs.access(join(destination, 'folder', 'private.txt')));
  } finally { fs.readdir = readdir; }
});

test('a failed workspace copy leaves no partial workspace directory', { skip: process.platform === 'win32' }, async t => {
  const { root, store } = await fixture(t);
  const row = await store.publish({ name: 'Workspace failure', html: '<!doctype html>' });
  const entry = join(store.revisionPath(row.id, 1), 'index.html');
  await fs.writeFile(join(root, 'outside.html'), 'outside');
  await fs.unlink(entry);
  await fs.symlink(join(root, 'outside.html'), entry);
  await assert.rejects(store.prepareWorkspace(row), /symlink/);
  assert.deepEqual(await fs.readdir(join(store.directory(row.id), 'workspaces')), []);
});
